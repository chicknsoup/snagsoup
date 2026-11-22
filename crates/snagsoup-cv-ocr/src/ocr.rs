use crate::error::{CvOcrError, Result};
use crate::neural_ocr::{
    is_neural_ocr_available, recognize_rgba_neural_with_progress, resize_gray_catmull_rom,
    sort_ocr_lines_transitively,
};
use snagsoup_canvas::TiledBitmap;
use snagsoup_model::RectF;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::core::HSTRING;

#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub bounds: RectF,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub bounds: RectF,
    pub words: Vec<OcrWord>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OcrDocument {
    pub text: String,
    pub lines: Vec<OcrLine>,
    pub text_angle: Option<f64>,
}

/// Lists OCR language tags available on this machine (including `"neural-latin-vi"` when the
/// bundled Offline Neural OCR engine is active, plus any installed `Windows.Media.Ocr` packs).
pub fn available_ocr_languages() -> Vec<String> {
    let mut out = Vec::new();
    if is_neural_ocr_available() {
        out.push("neural-latin-vi".to_string());
    }
    if let Ok(langs) = OcrEngine::AvailableRecognizerLanguages() {
        for lang in langs {
            if let Ok(tag) = lang.LanguageTag() {
                out.push(tag.to_string());
            }
        }
    }
    out
}

/// Recognizes text from a `TiledBitmap` using the Offline Multilingual Neural OCR engine
/// (with automatic fallback to 3× super-resolved `Windows.Media.Ocr`).
pub fn recognize_bitmap(bmp: &TiledBitmap, language_tag: Option<&str>) -> Result<OcrDocument> {
    recognize_rgba(bmp.width(), bmp.height(), &bmp.to_rgba(), language_tag)
}

/// Recognizes text from an in-memory RGBA8 buffer:
/// 1. Primary: Offline Neural OCR (`PP-OCRv4` DBNet + `EasyOCR` 64px Vietnamese/Latin BiLSTM-CTC)
///    when `language_tag` is `None` or a Latin/Vietnamese/English tag.
/// 2. Fallback: Enhanced 3× Catmull-Rom Bicubic + Dark-Mode-Normalized `Windows.Media.Ocr`.
pub fn recognize_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    language_tag: Option<&str>,
) -> Result<OcrDocument> {
    recognize_rgba_with_progress(width, height, rgba, language_tag, |_, _| true)
}

/// Recognizes text from an RGBA8 buffer while reporting incremental `(fraction_0_to_1, stage)`
/// progress updates. Returning `false` from `on_progress` cancels recognition immediately with
/// [`CvOcrError::Cancelled`].
pub fn recognize_rgba_with_progress<F>(
    width: u32,
    height: u32,
    rgba: &[u8],
    language_tag: Option<&str>,
    mut on_progress: F,
) -> Result<OcrDocument>
where
    F: FnMut(f32, &str) -> bool,
{
    if width == 0 || height == 0 || rgba.len() < (width as usize) * (height as usize) * 4 {
        return Err(CvOcrError::InvalidInput(
            "Invalid RGBA dimensions or buffer length".to_string(),
        ));
    }

    let prefer_neural = match language_tag {
        None => true,
        Some(tag) => {
            let t = tag.to_ascii_lowercase();
            t.starts_with("neural")
                || t.starts_with("vi")
                || t.starts_with("en")
                || t.starts_with("fr")
                || t.starts_with("de")
                || t.starts_with("es")
                || t.starts_with("it")
                || t.starts_with("pt")
        }
    };

    if prefer_neural {
        match recognize_rgba_neural_with_progress(width, height, rgba, &mut on_progress) {
            Ok(doc) => return Ok(doc),
            Err(CvOcrError::Cancelled) => return Err(CvOcrError::Cancelled),
            Err(_) => {}
        }
    }

    if !on_progress(0.20, "Running Windows OCR fallback...") {
        return Err(CvOcrError::Cancelled);
    }
    let winrt_tag = language_tag.filter(|t| !t.eq_ignore_ascii_case("neural-latin-vi"));
    let doc = recognize_rgba_winrt_enhanced(width, height, rgba, winrt_tag)?;
    let _ = on_progress(1.0, "Completed");
    Ok(doc)
}

/// Enhanced `Windows.Media.Ocr` fallback: normalizes dark-mode polarity, upscales small screen
/// captures 2×–3× with Catmull-Rom Bicubic, and adds a clean 24px white margin before recognition.
fn recognize_rgba_winrt_enhanced(
    width: u32,
    height: u32,
    rgba: &[u8],
    language_tag: Option<&str>,
) -> Result<OcrDocument> {
    let engine = if let Some(tag) = language_tag {
        let lang = Language::CreateLanguage(&HSTRING::from(tag))?;
        OcrEngine::TryCreateFromLanguage(&lang).map_err(|_| CvOcrError::OcrUnavailable)?
    } else {
        OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| CvOcrError::OcrUnavailable)?
    };

    let w = width as usize;
    let h = height as usize;
    let mut gray = vec![255u8; w * h];
    let mut lum_sum = 0u64;
    for (i, src) in rgba.chunks_exact(4).take(w * h).enumerate() {
        let a = src[3] as u32;
        let inv = 255 - a;
        let r = (src[0] as u32 * a + 255 * inv + 127) / 255;
        let g = (src[1] as u32 * a + 255 * inv + 127) / 255;
        let b = (src[2] as u32 * a + 255 * inv + 127) / 255;
        let lum = ((77 * r + 150 * g + 29 * b + 128) >> 8) as u8;
        gray[i] = lum;
        lum_sum += lum as u64;
    }

    // Invert global dark-mode captures so Windows.Media.Ocr sees dark text on white background
    if (lum_sum / ((w * h).max(1) as u64)) < 118 {
        for px in &mut gray {
            *px = 255 - *px;
        }
    }

    let scale: usize = if width.max(height) <= 900 {
        3
    } else if width.max(height) <= 1800 {
        2
    } else {
        1
    };
    let scaled_w = w * scale;
    let scaled_h = h * scale;
    let upscaled = if scale > 1 {
        resize_gray_catmull_rom(&gray, w, h, scaled_w, scaled_h)
    } else {
        gray
    };

    let pad = 24usize;
    let out_w = scaled_w + pad * 2;
    let out_h = scaled_h + pad * 2;
    let mut bgra = vec![255u8; out_w * out_h * 4];
    for y in 0..scaled_h {
        let src_row = y * scaled_w;
        let dst_row = (y + pad) * out_w + pad;
        for x in 0..scaled_w {
            let v = upscaled[src_row + x];
            let d = (dst_row + x) * 4;
            bgra[d] = v;
            bgra[d + 1] = v;
            bgra[d + 2] = v;
            bgra[d + 3] = 255;
        }
    }

    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let ibuf = writer.DetachBuffer()?;
    let soft_bmp = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &ibuf,
        BitmapPixelFormat::Bgra8,
        out_w as i32,
        out_h as i32,
        BitmapAlphaMode::Premultiplied,
    )?;

    let result = engine.RecognizeAsync(&soft_bmp)?.get()?;
    let text_angle = result.TextAngle().ok().and_then(|r| r.GetDouble().ok());
    let inv_scale = 1.0 / (scale as f64);
    let pad_f = pad as f64;

    let mut lines = Vec::new();
    for line in result.Lines()? {
        let mut words = Vec::new();
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for word in line.Words()? {
            let w_text = word.Text()?.to_string();
            let r = word.BoundingRect()?;
            let ox = ((r.X as f64 - pad_f) * inv_scale).clamp(0.0, width as f64);
            let oy = ((r.Y as f64 - pad_f) * inv_scale).clamp(0.0, height as f64);
            let ow = ((r.Width as f64) * inv_scale).max(1.0);
            let oh = ((r.Height as f64) * inv_scale).max(1.0);
            let rect = RectF::new(ox, oy, ow, oh);
            min_x = min_x.min(rect.x);
            min_y = min_y.min(rect.y);
            max_x = max_x.max(rect.right());
            max_y = max_y.max(rect.bottom());
            words.push(OcrWord {
                text: w_text,
                bounds: rect,
            });
        }

        // Reconstruct line text using real inter-word pixel gaps so URLs/paths don't get spurious spaces
        let mut line_text = String::new();
        for (wi, w_item) in words.iter().enumerate() {
            if wi > 0 {
                let prev_w = &words[wi - 1];
                let gap = w_item.bounds.x - prev_w.bounds.right();
                let tight_punct = prev_w.text.ends_with('/')
                    || prev_w.text.ends_with('.')
                    || prev_w.text.ends_with(':')
                    || prev_w.text.ends_with('-')
                    || prev_w.text.ends_with('_')
                    || w_item.text.starts_with('/')
                    || w_item.text.starts_with('.')
                    || w_item.text.starts_with(':')
                    || w_item.text.starts_with('-')
                    || w_item.text.starts_with('_');
                if !(tight_punct && gap < prev_w.bounds.height * 0.35) {
                    line_text.push(' ');
                }
            }
            line_text.push_str(&w_item.text);
        }
        if line_text.is_empty() {
            line_text = line.Text()?.to_string();
        }

        let bounds = if min_x.is_finite() {
            RectF::new(
                min_x,
                min_y,
                (max_x - min_x).max(1.0),
                (max_y - min_y).max(1.0),
            )
        } else {
            RectF::default()
        };

        lines.push(OcrLine {
            text: line_text,
            bounds,
            words,
        });
    }

    sort_ocr_lines_transitively(&mut lines);
    let formatted = format_grab_text(&lines);
    Ok(OcrDocument {
        text: formatted,
        lines,
        text_angle,
    })
}

/// Formats OCR lines into clean structured plain text preserving line breaks and table column tabs.
pub fn format_grab_text(lines: &[OcrLine]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        let txt = line.text.trim();
        if txt.is_empty() {
            continue;
        }
        if !out.is_empty() && idx > 0 {
            let prev = &lines[idx - 1];
            let cy_cur = line.bounds.y + line.bounds.height * 0.5;
            let cy_prev = prev.bounds.y + prev.bounds.height * 0.5;
            let same_row =
                (cy_cur - cy_prev).abs() <= prev.bounds.height.min(line.bounds.height) * 0.48;
            if same_row {
                let gap = line.bounds.x - prev.bounds.right();
                if gap > prev.bounds.height * 1.4 {
                    out.push('\t');
                } else {
                    out.push(' ');
                }
            } else {
                out.push('\n');
            }
        }
        out.push_str(txt);
    }
    out
}

/// Convenience wrapper for Snagit's "Grab Text" feature.
pub fn grab_text(bmp: &TiledBitmap, language_tag: Option<&str>) -> Result<String> {
    Ok(recognize_bitmap(bmp, language_tag)?.text)
}
