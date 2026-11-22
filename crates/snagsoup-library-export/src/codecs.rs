use crate::error::{ExportError, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use color_quant::NeuQuant;
use image::{ImageFormat, RgbaImage};
use pdf_writer::{Content, Finish, Name, Pdf, Rect as PdfRect, Ref};
use snagsoup_canvas::{downscale_rgba_bilinear, encode_rgba_to_png, TiledBitmap};
use snagsoup_model::{
    decode_rtf_base64, extract_plain_text, ColorRgba, GifExportPreset, SnagxPage, ToolMode,
};
use std::io::Cursor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterExportFormat {
    Png,
    Jpeg { quality: u8 },
    WebP,
    Bmp,
    Tiff,
    Ico,
}

/// Encodes an RGBA8 image into PNG, JPEG, WebP, BMP, TIFF, or ICO (Zero-CC pure Rust!).
pub fn export_raster_bytes(
    width: u32,
    height: u32,
    rgba: &[u8],
    format: RasterExportFormat,
) -> Result<Vec<u8>> {
    let img = RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| ExportError::InvalidInput("RGBA buffer size mismatch".into()))?;
    let mut out = Cursor::new(Vec::new());
    match format {
        RasterExportFormat::Png => {
            return Ok(encode_rgba_to_png(width, height, rgba)?);
        }
        RasterExportFormat::Jpeg { quality } => {
            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
            let mut enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
            enc.encode_image(&rgb)?;
        }
        RasterExportFormat::WebP => {
            img.write_to(&mut out, ImageFormat::WebP)?;
        }
        RasterExportFormat::Bmp => {
            img.write_to(&mut out, ImageFormat::Bmp)?;
        }
        RasterExportFormat::Tiff => {
            img.write_to(&mut out, ImageFormat::Tiff)?;
        }
        RasterExportFormat::Ico => {
            // ICO requires <= 256x256
            let (iw, ih, ibuf) = if width > 256 || height > 256 {
                let scale = 256.0 / width.max(height) as f32;
                let nw = ((width as f32 * scale).round() as u32).clamp(1, 256);
                let nh = ((height as f32 * scale).round() as u32).clamp(1, 256);
                (nw, nh, downscale_rgba_bilinear(rgba, width, height, nw, nh))
            } else {
                (width, height, rgba.to_vec())
            };
            let ico_img = RgbaImage::from_raw(iw, ih, ibuf)
                .ok_or_else(|| ExportError::InvalidInput("ICO resize failed".into()))?;
            ico_img.write_to(&mut out, ImageFormat::Ico)?;
        }
    }
    Ok(out.into_inner())
}

/// Decodes PNG, JPEG, WebP, BMP, TIFF, or ICO bytes into a `TiledBitmap`.
pub fn import_raster_bytes(bytes: &[u8]) -> Result<TiledBitmap> {
    let dyn_img = image::load_from_memory(bytes)?;
    let rgba = dyn_img.to_rgba8();
    Ok(TiledBitmap::from_rgba(rgba.width(), rgba.height(), rgba.as_raw())?)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PdfPageSize {
    AutoFit { dpi: f32 },
    A4,
    Letter,
}

/// Pure-Rust single- or multi-page PDF exporter using `pdf-writer` (0 MB C/Adobe deps!).
/// Each page is `(width, height, rgba8)` embedded as a JPEG image stream sized to `page_size`.
pub fn export_pdf_bytes(pages: &[(u32, u32, Vec<u8>)], page_size: PdfPageSize) -> Result<Vec<u8>> {
    if pages.is_empty() {
        return Err(ExportError::InvalidInput("PDF requires at least 1 page".into()));
    }

    let mut pdf = Pdf::new();
    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let mut next_id = 3i32;
    let mut alloc = || {
        let r = Ref::new(next_id);
        next_id += 1;
        r
    };

    let mut page_ids = Vec::with_capacity(pages.len());

    for (w, h, rgba) in pages {
        let page_id = alloc();
        let content_id = alloc();
        let img_id = alloc();
        page_ids.push(page_id);

        // Encode RGB JPEG stream for compact, universal PDF embedding
        let jpeg_bytes = export_raster_bytes(*w, *h, rgba, RasterExportFormat::Jpeg { quality: 92 })?;

        let (pw_pt, ph_pt, draw_x, draw_y, draw_w, draw_h) = match page_size {
            PdfPageSize::AutoFit { dpi } => {
                let d = dpi.max(72.0);
                let pt_w = (*w as f32) * 72.0 / d;
                let pt_h = (*h as f32) * 72.0 / d;
                (pt_w, pt_h, 0.0, 0.0, pt_w, pt_h)
            }
            PdfPageSize::A4 => fit_in_page(*w, *h, 595.28, 841.89, 24.0),
            PdfPageSize::Letter => fit_in_page(*w, *h, 612.0, 792.0, 24.0),
        };

        let mut img_xobj = pdf.image_xobject(img_id, &jpeg_bytes);
        img_xobj.filter(pdf_writer::Filter::DctDecode);
        img_xobj.width(*w as i32);
        img_xobj.height(*h as i32);
        img_xobj.color_space().device_rgb();
        img_xobj.bits_per_component(8);
        img_xobj.finish();

        let mut content = Content::new();
        content.save_state();
        content.transform([draw_w, 0.0, 0.0, draw_h, draw_x, draw_y]);
        content.x_object(Name(b"Im0"));
        content.restore_state();
        pdf.stream(content_id, &content.finish());

        let mut page = pdf.page(page_id);
        page.parent(page_tree_id);
        page.media_box(PdfRect::new(0.0, 0.0, pw_pt, ph_pt));
        page.contents(content_id);
        page.resources().x_objects().pair(Name(b"Im0"), img_id);
        page.finish();
    }

    pdf.pages(page_tree_id)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);
    pdf.catalog(catalog_id).pages(page_tree_id);

    Ok(pdf.finish())
}

fn fit_in_page(w: u32, h: u32, pw: f32, ph: f32, margin: f32) -> (f32, f32, f32, f32, f32, f32) {
    let avail_w = (pw - margin * 2.0).max(1.0);
    let avail_h = (ph - margin * 2.0).max(1.0);
    let scale = (avail_w / w.max(1) as f32).min(avail_h / h.max(1) as f32);
    let dw = w as f32 * scale;
    let dh = h as f32 * scale;
    let dx = (pw - dw) * 0.5;
    let dy = (ph - dh) * 0.5;
    (pw, ph, dx, dy, dw, dh)
}

/// Pure-Rust SVG exporter preserving vector `CaptureObjects` (`Arrow`, `Line`, `Shape`, `Callout`, `Text`, `Step`)
/// layered over an embedded `<image href="data:image/png;base64,..."/>` raster background.
pub fn export_svg_string(page: &SnagxPage, bg_bitmap: Option<&TiledBitmap>) -> Result<String> {
    let w = page
        .capture_canvas_width
        .map(|v| v.round() as u32)
        .or_else(|| bg_bitmap.map(|b| b.width()))
        .unwrap_or(800);
    let h = page
        .capture_canvas_height
        .map(|v| v.round() as u32)
        .or_else(|| bg_bitmap.map(|b| b.height()))
        .unwrap_or(600);

    let mut svg = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
"#
    );

    if let Some(bg) = page.capture_background_color {
        if bg.a > 0 {
            svg.push_str(&format!(
                "  <rect width=\"{w}\" height=\"{h}\" fill=\"{}\" fill-opacity=\"{:.3}\"/>\n",
                svg_hex(bg),
                bg.a as f32 / 255.0
            ));
        }
    }

    if let Some(bmp) = bg_bitmap {
        if bmp.width() > 0 && bmp.height() > 0 {
            let png = bmp.to_png_bytes()?;
            let b64 = BASE64.encode(&png);
            svg.push_str(&format!(
                "  <image x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" href=\"data:image/png;base64,{b64}\"/>\n",
                bmp.width(),
                bmp.height()
            ));
        }
    }

    for obj in &page.capture_objects {
        let op = (obj.opacity.unwrap_or(100.0) / 100.0).clamp(0.0, 1.0);
        let fg = obj.foreground_color.unwrap_or(ColorRgba::rgb(252, 66, 66));
        let bg = obj.background_color.unwrap_or(ColorRgba::TRANSPARENT);
        let sw = obj.stroke_width.unwrap_or(2.0);
        let mode = obj.tool_mode.clone().unwrap_or(ToolMode::Shape);

        match mode {
            ToolMode::Arrow | ToolMode::Line => {
                if let (Some(s), Some(e)) = (obj.start_point, obj.end_point) {
                    svg.push_str(&format!(
                        "  <line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"{}\" stroke-width=\"{:.2}\" stroke-linecap=\"round\" opacity=\"{:.3}\"/>\n",
                        s.x, s.y, e.x, e.y, svg_hex(fg), sw, op
                    ));
                }
            }
            ToolMode::Step => {
                if let (Some(s), Some(e)) = (obj.start_point, obj.end_point) {
                    let cx = (s.x + e.x) * 0.5;
                    let cy = (s.y + e.y) * 0.5;
                    let r = ((e.x - s.x).abs().min((e.y - s.y).abs()) * 0.5).max(8.0);
                    let fill = if bg.a > 0 { bg } else { fg };
                    let label = xml_escape(obj.step_value.as_deref().unwrap_or("1"));
                    svg.push_str(&format!(
                        "  <circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" fill=\"{}\" opacity=\"{op:.3}\"/>\n",
                        svg_hex(fill)
                    ));
                    svg.push_str(&format!(
                        "  <text x=\"{cx:.2}\" y=\"{:.2}\" text-anchor=\"middle\" font-family=\"Arial\" font-size=\"{:.1}\" font-weight=\"bold\" fill=\"#FFFFFF\">{label}</text>\n",
                        cy + r * 0.35,
                        r * 1.1
                    ));
                }
            }
            ToolMode::Text | ToolMode::Callout => {
                if let (Some(s), Some(e)) = (obj.start_point, obj.end_point) {
                    let x = s.x.min(e.x);
                    let y = s.y.min(e.y);
                    let rw = (e.x - s.x).abs().max(1.0);
                    let rh = (e.y - s.y).abs().max(1.0);
                    if mode == ToolMode::Callout || bg.a > 0 {
                        svg.push_str(&format!(
                            "  <rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{rw:.2}\" height=\"{rh:.2}\" rx=\"8\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{sw:.2}\" opacity=\"{op:.3}\"/>\n",
                            svg_hex(bg),
                            svg_hex(fg)
                        ));
                    }
                    let txt = obj
                        .rtf_encoded_text
                        .as_deref()
                        .and_then(|b| decode_rtf_base64(b).ok())
                        .map(|r| extract_plain_text(&r))
                        .filter(|t| !t.trim().is_empty())
                        .or_else(|| obj.placeholder_text.clone())
                        .unwrap_or_default();
                    if !txt.is_empty() {
                        let fs = obj.font_size.unwrap_or(16.0);
                        svg.push_str(&format!(
                            "  <text x=\"{:.2}\" y=\"{:.2}\" text-anchor=\"middle\" font-family=\"{}\" font-size=\"{fs:.1}\" fill=\"{}\">{}</text>\n",
                            x + rw * 0.5,
                            y + rh * 0.5 + fs * 0.35,
                            xml_escape(obj.font_family.as_deref().unwrap_or("Arial")),
                            svg_hex(fg),
                            xml_escape(&txt)
                        ));
                    }
                }
            }
            _ => {
                if let (Some(s), Some(e)) = (obj.start_point, obj.end_point) {
                    let x = s.x.min(e.x);
                    let y = s.y.min(e.y);
                    let rw = (e.x - s.x).abs().max(1.0);
                    let rh = (e.y - s.y).abs().max(1.0);
                    let fill_attr = if bg.a > 0 {
                        svg_hex(bg)
                    } else {
                        "none".to_string()
                    };
                    svg.push_str(&format!(
                        "  <rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{rw:.2}\" height=\"{rh:.2}\" fill=\"{fill_attr}\" stroke=\"{}\" stroke-width=\"{sw:.2}\" opacity=\"{op:.3}\"/>\n",
                        svg_hex(fg)
                    ));
                }
            }
        }
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

fn svg_hex(c: ColorRgba) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Input frame for `AnimatedGifExporter`.
#[derive(Debug, Clone)]
pub struct GifFrameInput {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Frame duration in centiseconds (1/100 s, e.g. 10 = 100ms = 10 fps).
    pub delay_cs: u16,
}

#[derive(Debug, Clone)]
pub struct GifExportReport {
    pub bytes: Vec<u8>,
    pub input_frames: usize,
    pub stored_frames: usize,
    pub total_duration_cs: u32,
}

/// Pure-Rust Animated GIF exporter (`gif` + `color_quant` NeuQuant + Floyd-Steinberg dithering +
/// `SkipSimilarFrames` delay-merging + optional fade-to-black `InsertTransitionFrames`).
pub struct AnimatedGifExporter;

impl AnimatedGifExporter {
    pub fn export(frames: &[GifFrameInput], preset: &GifExportPreset) -> Result<GifExportReport> {
        if frames.is_empty() {
            return Err(ExportError::InvalidInput("GIF requires at least 1 frame".into()));
        }

        // Resolve target dimensions respecting preset max_width / max_height
        let src_w = frames[0].width;
        let src_h = frames[0].height;
        let mut scale = 1.0f32;
        if let Some(mw) = preset.max_width {
            if src_w > mw {
                scale = scale.min(mw as f32 / src_w as f32);
            }
        }
        if let Some(mh) = preset.max_height {
            if src_h > mh {
                scale = scale.min(mh as f32 / src_h as f32);
            }
        }
        let dst_w = ((src_w as f32 * scale).round() as u32).max(1) as u16;
        let dst_h = ((src_h as f32 * scale).round() as u32).max(1) as u16;

        // 1. Scale & apply SkipSimilarFrames (merging delays when diff < similarity_threshold)
        let mut merged: Vec<(Vec<u8>, u16)> = Vec::new();
        for f in frames {
            let scaled = if f.width == dst_w as u32 && f.height == dst_h as u32 {
                f.rgba.clone()
            } else {
                downscale_rgba_bilinear(&f.rgba, f.width, f.height, dst_w as u32, dst_h as u32)
            };

            if preset.skip_similar_frames {
                if let Some((prev_rgba, prev_delay)) = merged.last_mut() {
                    let diff_ratio = frame_diff_ratio(prev_rgba, &scaled);
                    if diff_ratio <= preset.similarity_threshold {
                        *prev_delay = prev_delay.saturating_add(f.delay_cs.max(1));
                        continue;
                    }
                }
            }
            merged.push((scaled, f.delay_cs.max(1)));
        }

        // 2. Optional fade-to-black transition frames at end of loop
        if preset.insert_transition_frames {
            if let Some((last_rgba, _)) = merged.last().cloned() {
                for step in [0.66f32, 0.33, 0.0] {
                    let faded: Vec<u8> = last_rgba
                        .chunks_exact(4)
                        .flat_map(|px| {
                            [
                                (px[0] as f32 * step).round() as u8,
                                (px[1] as f32 * step).round() as u8,
                                (px[2] as f32 * step).round() as u8,
                                255,
                            ]
                        })
                        .collect();
                    merged.push((faded, 8));
                }
            }
        }

        // 3. Encode with `gif` + `color_quant` NeuQuant + Floyd-Steinberg dithering
        let mut out_bytes = Vec::new();
        let total_duration_cs: u32 = merged.iter().map(|(_, d)| *d as u32).sum();
        let stored_frames = merged.len();

        {
            let mut encoder = gif::Encoder::new(&mut out_bytes, dst_w, dst_h, &[])?;
            if preset.loop_forever {
                encoder.set_repeat(gif::Repeat::Infinite)?;
            }

            let dither = preset.dithering_factor.clamp(0.0, 1.0);
            for (rgba, delay_cs) in &merged {
                let nq = NeuQuant::new(10, 256, rgba);
                let palette = nq.color_map_rgb();
                let indices = quantize_with_dither(rgba, dst_w as usize, dst_h as usize, &nq, &palette, dither);

                let mut frame = gif::Frame::from_palette_pixels(
                    dst_w,
                    dst_h,
                    indices,
                    palette,
                    None,
                );
                frame.delay = *delay_cs;
                encoder.write_frame(&frame)?;
            }
        }

        Ok(GifExportReport {
            bytes: out_bytes,
            input_frames: frames.len(),
            stored_frames,
            total_duration_cs,
        })
    }
}

fn frame_diff_ratio(a: &[u8], b: &[u8]) -> f32 {
    let pixels = (a.len().min(b.len())) / 4;
    if pixels == 0 {
        return 0.0;
    }
    let mut diff_px = 0usize;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        if (pa[0] as i16 - pb[0] as i16).abs() > 2
            || (pa[1] as i16 - pb[1] as i16).abs() > 2
            || (pa[2] as i16 - pb[2] as i16).abs() > 2
        {
            diff_px += 1;
        }
    }
    diff_px as f32 / pixels as f32
}

fn quantize_with_dither(
    rgba: &[u8],
    w: usize,
    h: usize,
    nq: &NeuQuant,
    palette: &[u8],
    dither_factor: f32,
) -> Vec<u8> {
    let mut indices = vec![0u8; w * h];
    if dither_factor <= 0.01 {
        for (i, px) in rgba.chunks_exact(4).enumerate() {
            indices[i] = nq.index_of(px) as u8;
        }
        return indices;
    }

    // Floyd-Steinberg error diffusion scaled by `dither_factor`
    let mut err = vec![0.0f32; w * h * 3];
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        err[i * 3] = px[0] as f32;
        err[i * 3 + 1] = px[1] as f32;
        err[i * 3 + 2] = px[2] as f32;
    }

    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let r = err[i * 3].clamp(0.0, 255.0);
            let g = err[i * 3 + 1].clamp(0.0, 255.0);
            let b = err[i * 3 + 2].clamp(0.0, 255.0);
            let idx = nq.index_of(&[r as u8, g as u8, b as u8, 255]);
            indices[i] = idx as u8;

            let pr = palette[idx * 3] as f32;
            let pg = palette[idx * 3 + 1] as f32;
            let pb = palette[idx * 3 + 2] as f32;

            let er = (r - pr) * dither_factor;
            let eg = (g - pg) * dither_factor;
            let eb = (b - pb) * dither_factor;

            let mut spread = |nx: isize, ny: isize, wt: f32| {
                if nx >= 0 && (nx as usize) < w && ny >= 0 && (ny as usize) < h {
                    let ni = (ny as usize) * w + (nx as usize);
                    err[ni * 3] += er * wt;
                    err[ni * 3 + 1] += eg * wt;
                    err[ni * 3 + 2] += eb * wt;
                }
            };
            spread(x as isize + 1, y as isize, 7.0 / 16.0);
            spread(x as isize - 1, y as isize + 1, 3.0 / 16.0);
            spread(x as isize, y as isize + 1, 5.0 / 16.0);
            spread(x as isize + 1, y as isize + 1, 1.0 / 16.0);
        }
    }
    indices
}
