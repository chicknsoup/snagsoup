use crate::error::{CvOcrError, Result};
use crate::inpaint::{inpaint_rect, InpaintTierUsed};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use snagsoup_canvas::{encode_rgba_to_png, DocumentSession, HistoryCommand, PixelRect, TiledBitmap};
use snagsoup_model::{CaptureObject, PointF, ToolMode};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartElement {
    pub bounds: PixelRect,
    pub is_text_like: bool,
}

/// Detects movable UI elements (buttons, icons, badges, labels) via edge connected components
/// + stroke-density heuristic (Smart Move parity with `TSCV.Recognition.dll`).
pub fn detect_smart_objects(bmp: &TiledBitmap) -> Vec<SmartElement> {
    let w = bmp.width() as usize;
    let h = bmp.height() as usize;
    if w < 8 || h < 8 {
        return Vec::new();
    }
    let rgba = bmp.to_rgba();

    // Color-gradient magnitude map (catches colored buttons on similarly-bright backgrounds!)
    let mut edges = vec![0u8; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let l = (y * w + x - 1) * 4;
            let r = (y * w + x + 1) * 4;
            let u = ((y - 1) * w + x) * 4;
            let d = ((y + 1) * w + x) * 4;
            let mut grad = 0i32;
            for c in 0..3 {
                grad += (rgba[r + c] as i32 - rgba[l + c] as i32).abs();
                grad += (rgba[d + c] as i32 - rgba[u + c] as i32).abs();
            }
            if grad >= 64 {
                edges[y * w + x] = 1;
            }
        }
    }

    // 2px dilation to bridge anti-aliased gaps around UI controls
    let mut mask = vec![0u8; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            if edges[y * w + x] != 0 {
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        mask[((y as isize + dy) as usize) * w + (x as isize + dx) as usize] = 1;
                    }
                }
            }
        }
    }

    let mut visited = vec![false; w * h];
    let mut queue = VecDeque::new();
    let mut out = Vec::new();
    let max_area = (w * h) * 60 / 100;

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if mask[idx] == 0 || visited[idx] {
                continue;
            }
            visited[idx] = true;
            queue.push_back((x, y));
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;
            let mut edge_px = 0usize;

            while let Some((cx, cy)) = queue.pop_front() {
                if edges[cy * w + cx] != 0 {
                    edge_px += 1;
                }
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);

                for (dx, dy) in [(-1isize, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = cx as isize + dx;
                    let ny = cy as isize + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        let nidx = (ny as usize) * w + (nx as usize);
                        if mask[nidx] != 0 && !visited[nidx] {
                            visited[nidx] = true;
                            queue.push_back((nx as usize, ny as usize));
                        }
                    }
                }
            }

            let rw = max_x - min_x + 1;
            let rh = max_y - min_y + 1;
            let area = rw * rh;
            if rw >= 10 && rh >= 8 && area <= max_area {
                let density = edge_px as f32 / area as f32;
                let is_text_like = density > 0.18 && rw > rh * 2;
                out.push(SmartElement {
                    bounds: PixelRect::new(min_x as u32, min_y as u32, rw as u32, rh as u32),
                    is_text_like,
                });
            }
        }
    }
    out
}

/// Lifts a detected UI element at `rect` out of `session.bitmap` **without** pushing history yet,
/// returning `(obj_idx, before_bitmap, clipped_rect, inpaint_tier)` so an interactive drag can
/// commit a single `HistoryCommand::Batch` at `mouse_up`.
pub fn lift_smart_object_unrecorded(
    session: &mut DocumentSession,
    rect: PixelRect,
) -> Result<(usize, TiledBitmap, PixelRect, Option<InpaintTierUsed>)> {
    let clipped = rect
        .intersect(session.bitmap.bounds())
        .ok_or_else(|| CvOcrError::InvalidInput("Lift rect out of bounds".to_string()))?;

    let elem_rgba = session.bitmap.read_region(clipped);
    let png_bytes = encode_rgba_to_png(clipped.width, clipped.height, &elem_rgba)?;
    let b64 = BASE64.encode(&png_bytes);

    let before_bitmap = session.bitmap.clone();
    let tier = inpaint_rect(&mut session.bitmap, clipped);
    session.dirty.mark_rect(clipped);

    let obj = CaptureObject {
        object_id: Some(format!("smart-move-{}-{}", clipped.x, clipped.y)),
        tool_mode: Some(ToolMode::Image),
        start_point: Some(PointF::new(clipped.x as f64, clipped.y as f64)),
        end_point: Some(PointF::new(clipped.right() as f64, clipped.bottom() as f64)),
        image: Some(b64),
        opacity: Some(100.0),
        ..CaptureObject::default()
    };
    let idx = session.add_object_unrecorded(obj);
    Ok((idx, before_bitmap, clipped, tier))
}

/// Lifts a detected UI element at `rect` out of `session.bitmap`:
/// 1. Extracts `rect` as a floating PNG `CaptureObject` (`ToolMode::Image`).
/// 2. Inpaints `rect` cleanly on `session.bitmap` using our two-tier inpainter.
/// 3. Records both the raster snapshot and the new floating object as **one** atomic Undo step.
pub fn lift_smart_object(
    session: &mut DocumentSession,
    rect: PixelRect,
) -> Result<(usize, Option<InpaintTierUsed>)> {
    let (idx, before_bitmap, clipped, tier) = lift_smart_object_unrecorded(session, rect)?;
    let after_bitmap = session.bitmap.clone();
    let object = session.page.capture_objects[idx].clone();
    session.history.push(HistoryCommand::Batch(vec![
        HistoryCommand::RasterSnapshot {
            before: before_bitmap,
            after: after_bitmap,
            dirty_rect: clipped,
        },
        HistoryCommand::AddObject { index: idx, object },
    ]));
    Ok((idx, tier))
}
