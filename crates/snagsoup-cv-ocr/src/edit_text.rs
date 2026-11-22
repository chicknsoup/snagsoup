use crate::error::Result;
use crate::inpaint::inpaint_rect;
use crate::ocr::recognize_bitmap;
use snagsoup_canvas::{DocumentSession, HistoryCommand, PixelRect};
use snagsoup_model::{
    encode_rtf_base64, generate_rtf, CaptureObject, ColorRgba, PointF, RichTextSpan,
    RichTextStyle, ToolMode,
};

/// Snagit "Edit Text" implementation:
/// 1. Recognizes text lines via `Windows.Media.Ocr`.
/// 2. Estimates foreground text color & font size per line.
/// 3. Inpaints the original text pixels on `session.bitmap` (single undoable raster snapshot).
/// 4. Spawns editable `ToolMode::Text` `CaptureObject`s at the exact recognized coordinates.
pub fn edit_text_in_session(
    session: &mut DocumentSession,
    language_tag: Option<&str>,
) -> Result<Vec<usize>> {
    let doc = recognize_bitmap(&session.bitmap, language_tag)?;
    if doc.lines.is_empty() {
        return Ok(Vec::new());
    }

    let before_bitmap = session.bitmap.clone();
    let mut dirty_union: Option<PixelRect> = None;
    let mut new_objects: Vec<CaptureObject> = Vec::with_capacity(doc.lines.len());

    for (idx, line) in doc.lines.iter().enumerate() {
        if line.text.trim().is_empty() {
            continue;
        }
        let pad = 2.0;
        let x0 = (line.bounds.x - pad).floor().max(0.0) as u32;
        let y0 = (line.bounds.y - pad).floor().max(0.0) as u32;
        let x1 = (line.bounds.right() + pad)
            .ceil()
            .min(session.bitmap.width() as f64) as u32;
        let y1 = (line.bounds.bottom() + pad)
            .ceil()
            .min(session.bitmap.height() as f64) as u32;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let rect = PixelRect::new(x0, y0, x1 - x0, y1 - y0);

        // Estimate text color before inpainting
        let text_color = estimate_text_color(&session.bitmap, rect);
        let font_size = (line.bounds.height * 0.85).clamp(8.0, 200.0);

        // Erase original text cleanly from bitmap
        inpaint_rect(&mut session.bitmap, rect);
        dirty_union = Some(match dirty_union {
            Some(prev) => prev.union(rect),
            None => rect,
        });

        let rtf = generate_rtf(&[RichTextSpan {
            text: line.text.trim().to_string(),
            style: RichTextStyle {
                font_family: Some("Segoe UI".to_string()),
                font_size_pt: Some(font_size as f32),
                color: Some(text_color),
                bold: false,
                italic: false,
                underline: false,
            },
        }]);

        let obj = CaptureObject {
            object_id: Some(format!("ocr-text-{idx}")),
            tool_mode: Some(ToolMode::Text),
            start_point: Some(PointF::new(line.bounds.x, line.bounds.y)),
            end_point: Some(PointF::new(line.bounds.right(), line.bounds.bottom())),
            foreground_color: Some(text_color),
            background_color: Some(ColorRgba::TRANSPARENT),
            font_family: Some("Segoe UI".to_string()),
            font_size: Some(font_size),
            rtf_encoded_text: Some(encode_rtf_base64(&rtf)),
            placeholder_text: Some(line.text.trim().to_string()),
            opacity: Some(100.0),
            ..CaptureObject::default()
        };
        new_objects.push(obj);
    }

    if let Some(dirty_rect) = dirty_union {
        let after_bitmap = session.bitmap.clone();
        session.history.push(HistoryCommand::RasterSnapshot {
            before: before_bitmap,
            after: after_bitmap,
            dirty_rect,
        });
        session.dirty.mark_rect(dirty_rect);
    }

    let mut indices = Vec::with_capacity(new_objects.len());
    for obj in new_objects {
        indices.push(session.add_object(obj));
    }
    Ok(indices)
}

/// Estimates foreground text color by picking the pixel inside `rect` with maximum contrast
/// against the mean border color around `rect`.
fn estimate_text_color(bmp: &snagsoup_canvas::TiledBitmap, rect: PixelRect) -> ColorRgba {
    let buf = bmp.read_region(rect);
    let rw = rect.width as usize;
    let rh = rect.height as usize;
    if rw == 0 || rh == 0 || buf.is_empty() {
        return ColorRgba::BLACK;
    }

    // Average border pixels of rect as background estimate
    let mut bg = [0.0f32; 3];
    let mut count = 0.0f32;
    for x in 0..rw {
        for &y in &[0usize, rh - 1] {
            let off = (y * rw + x) * 4;
            bg[0] += buf[off] as f32;
            bg[1] += buf[off + 1] as f32;
            bg[2] += buf[off + 2] as f32;
            count += 1.0;
        }
    }
    if count > 0.0 {
        bg[0] /= count;
        bg[1] /= count;
        bg[2] /= count;
    }

    // Find top-contrast pixels (stroke core)
    let mut best_dist = -1.0f32;
    let mut best_px = [0u8, 0, 0];
    for px in buf.chunks_exact(4) {
        if px[3] < 128 {
            continue;
        }
        let dr = px[0] as f32 - bg[0];
        let dg = px[1] as f32 - bg[1];
        let db = px[2] as f32 - bg[2];
        let d2 = dr * dr + dg * dg + db * db;
        if d2 > best_dist {
            best_dist = d2;
            best_px = [px[0], px[1], px[2]];
        }
    }
    ColorRgba::rgb(best_px[0], best_px[1], best_px[2])
}
