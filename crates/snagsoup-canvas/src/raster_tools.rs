use crate::history::{DocumentSession, HistoryCommand};
use crate::tile_bitmap::{PixelRect, TileCoord, TiledBitmap};
use crate::vector_g1::{build_rounded_rect_path, object_bounds_rectf, to_skia_color};
use snagsoup_model::{CaptureObject, ColorRgba, PointF, RectF};
use std::collections::VecDeque;
use tiny_skia::{
    FillRule, LineCap, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

/// 1. Eraser: interpolates a continuous circular brush along `points` on `TiledBitmap` (CoW).
pub fn eraser_stroke(
    bmp: &mut TiledBitmap,
    points: &[PointF],
    radius: f32,
    erase_color: ColorRgba,
) -> Vec<TileCoord> {
    if points.is_empty() || radius <= 0.0 {
        return Vec::new();
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in points {
        min_x = min_x.min(p.x - radius as f64);
        min_y = min_y.min(p.y - radius as f64);
        max_x = max_x.max(p.x + radius as f64);
        max_y = max_y.max(p.y + radius as f64);
    }
    let Some(rect) = PixelRect::from_rect_f(
        RectF::new(min_x, min_y, (max_x - min_x).max(1.0), (max_y - min_y).max(1.0)),
        bmp.width(),
        bmp.height(),
    ) else {
        return Vec::new();
    };

    // Sample interpolated dab centers every ~radius/2 pixels
    let step = (radius * 0.4).max(1.0) as f64;
    let mut dabs: Vec<(f64, f64)> = Vec::new();
    dabs.push((points[0].x, points[0].y));
    for win in points.windows(2) {
        let dx = win[1].x - win[0].x;
        let dy = win[1].y - win[0].y;
        let dist = dx.hypot(dy);
        let n = (dist / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            dabs.push((win[0].x + dx * t, win[0].y + dy * t));
        }
    }

    let r2 = (radius as f64) * (radius as f64);
    let dst_px = [erase_color.r, erase_color.g, erase_color.b, erase_color.a];
    bmp.mutate_region(rect, |x, y, px| {
        let xf = x as f64 + 0.5;
        let yf = y as f64 + 0.5;
        for &(cx, cy) in &dabs {
            let dx = xf - cx;
            let dy = yf - cy;
            if dx * dx + dy * dy <= r2 {
                *px = dst_px;
                break;
            }
        }
    })
}

/// 2. Flood Fill: scanline queue flood-fill with `tolerance` (0..100) and optional 1px edge antialiasing.
pub fn flood_fill(
    bmp: &mut TiledBitmap,
    seed_x: u32,
    seed_y: u32,
    fill_color: ColorRgba,
    tolerance: f32,
    antialiasing: bool,
) -> Option<PixelRect> {
    let w = bmp.width();
    let h = bmp.height();
    if seed_x >= w || seed_y >= h {
        return None;
    }
    let target = bmp.get_pixel(seed_x, seed_y)?;
    let replacement = [fill_color.r, fill_color.g, fill_color.b, fill_color.a];
    if target == replacement {
        return None;
    }

    let rgba = bmp.to_rgba();
    let tol_dist = (tolerance.clamp(0.0, 100.0) / 100.0) * 441.67294; // sqrt(255^2 * 3)
    let tol_sq = tol_dist * tol_dist;

    let color_match = |idx: usize| -> bool {
        let dr = rgba[idx] as f32 - target[0] as f32;
        let dg = rgba[idx + 1] as f32 - target[1] as f32;
        let db = rgba[idx + 2] as f32 - target[2] as f32;
        let da = rgba[idx + 3] as f32 - target[3] as f32;
        (dr * dr + dg * dg + db * db + da * da) <= tol_sq
    };

    let mut mask = vec![0u8; (w as usize) * (h as usize)];
    let mut queue = VecDeque::new();
    queue.push_back((seed_x, seed_y));

    let mut min_x = seed_x;
    let mut min_y = seed_y;
    let mut max_x = seed_x;
    let mut max_y = seed_y;

    while let Some((x, y)) = queue.pop_front() {
        let row_off = (y as usize) * (w as usize);
        if mask[row_off + x as usize] != 0 || !color_match((row_off + x as usize) * 4) {
            continue;
        }
        // Scan left
        let mut lx = x;
        while lx > 0
            && mask[row_off + (lx - 1) as usize] == 0
            && color_match((row_off + (lx - 1) as usize) * 4)
        {
            lx -= 1;
        }
        // Scan right
        let mut rx = x;
        while rx + 1 < w
            && mask[row_off + (rx + 1) as usize] == 0
            && color_match((row_off + (rx + 1) as usize) * 4)
        {
            rx += 1;
        }

        min_x = min_x.min(lx);
        max_x = max_x.max(rx);
        min_y = min_y.min(y);
        max_y = max_y.max(y);

        for cx in lx..=rx {
            mask[row_off + cx as usize] = 255;
            if y > 0 && mask[((y - 1) as usize) * (w as usize) + cx as usize] == 0 {
                queue.push_back((cx, y - 1));
            }
            if y + 1 < h && mask[((y + 1) as usize) * (w as usize) + cx as usize] == 0 {
                queue.push_back((cx, y + 1));
            }
        }
    }

    // Optional 1px edge antialiasing feathering
    if antialiasing {
        let mut edge_pixels = Vec::new();
        let ex0 = min_x.saturating_sub(1);
        let ey0 = min_y.saturating_sub(1);
        let ex1 = (max_x + 1).min(w - 1);
        let ey1 = (max_y + 1).min(h - 1);
        for y in ey0..=ey1 {
            for x in ex0..=ex1 {
                let idx = (y as usize) * (w as usize) + x as usize;
                if mask[idx] == 0 {
                    let mut filled_neighbors = 0u32;
                    if x > 0 && mask[idx - 1] == 255 {
                        filled_neighbors += 1;
                    }
                    if x + 1 < w && mask[idx + 1] == 255 {
                        filled_neighbors += 1;
                    }
                    if y > 0 && mask[idx - w as usize] == 255 {
                        filled_neighbors += 1;
                    }
                    if y + 1 < h && mask[idx + w as usize] == 255 {
                        filled_neighbors += 1;
                    }
                    if filled_neighbors > 0 {
                        edge_pixels.push((idx, (filled_neighbors * 40).min(160) as u8));
                    }
                }
            }
        }
        for (idx, alpha) in edge_pixels {
            mask[idx] = alpha;
        }
        min_x = ex0;
        min_y = ey0;
        max_x = ex1;
        max_y = ey1;
    }

    let dirty = PixelRect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1);
    bmp.mutate_region(dirty, |x, y, px| {
        let m = mask[(y as usize) * (w as usize) + x as usize];
        if m == 255 {
            *px = replacement;
        } else if m > 0 {
            let t = m as f32 / 255.0;
            for c in 0..4 {
                px[c] = (px[c] as f32 * (1.0 - t) + replacement[c] as f32 * t)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    });
    Some(dirty)
}

/// 3. Non-destructive Blur (3-pass O(1) separable Box Blur) or Pixelate (`BlurType == 1`) on `Pixmap`.
pub fn render_blur_or_pixelate(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let Some(r) = object_bounds_rectf(obj) else { return };
    let x0 = (r.x as f32 * transform.sx + transform.tx).floor().max(0.0) as u32;
    let y0 = (r.y as f32 * transform.sy + transform.ty).floor().max(0.0) as u32;
    let x1 = ((r.right() as f32) * transform.sx + transform.tx)
        .ceil()
        .clamp(0.0, pixmap.width() as f32) as u32;
    let y1 = ((r.bottom() as f32) * transform.sy + transform.ty)
        .ceil()
        .clamp(0.0, pixmap.height() as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let rect = PixelRect::new(x0, y0, x1 - x0, y1 - y0);
    let blur_type = obj.extra.get("BlurType").and_then(|v| v.as_i64()).unwrap_or(0);
    let intensity = obj
        .extra
        .get("BlurIntensity")
        .and_then(|v| v.as_f64())
        .unwrap_or(15.0) as f32;

    if blur_type == 1 {
        let block = ((intensity * 0.4).round() as u32).clamp(2, 64);
        apply_pixelate_region(pixmap, rect, block);
    } else {
        let radius = ((intensity * 0.35).round() as usize).clamp(1, 48);
        apply_box_blur_3pass_region(pixmap, rect, radius);
    }
}

/// Fast O(1)-per-pixel 3-pass separable Box Blur (approximating Gaussian blur) on a `Pixmap` sub-rect.
pub fn apply_box_blur_3pass_region(pixmap: &mut Pixmap, rect: PixelRect, radius: usize) {
    if rect.is_empty() || radius == 0 {
        return;
    }
    let pw = pixmap.width() as usize;
    let rw = rect.width as usize;
    let rh = rect.height as usize;
    let rx = rect.x as usize;
    let ry = rect.y as usize;

    let mut buf = vec![0u8; rw * rh * 4];
    let mut tmp = vec![0u8; rw * rh * 4];
    {
        let data = pixmap.data();
        for y in 0..rh {
            let src = ((ry + y) * pw + rx) * 4;
            let dst = y * rw * 4;
            buf[dst..dst + rw * 4].copy_from_slice(&data[src..src + rw * 4]);
        }
    }

    let window = (radius * 2 + 1) as u32;
    for _pass in 0..3 {
        // Horizontal pass: buf -> tmp
        for y in 0..rh {
            let row = y * rw * 4;
            let mut sum = [0u32; 4];
            for dx in -(radius as isize)..=(radius as isize) {
                let sx = dx.clamp(0, (rw - 1) as isize) as usize;
                let off = row + sx * 4;
                for c in 0..4 {
                    sum[c] += buf[off + c] as u32;
                }
            }
            for x in 0..rw {
                let dst = row + x * 4;
                for c in 0..4 {
                    tmp[dst + c] = (sum[c] / window) as u8;
                }
                let old_x = (x as isize - radius as isize).clamp(0, (rw - 1) as isize) as usize;
                let new_x = (x as isize + radius as isize + 1).clamp(0, (rw - 1) as isize) as usize;
                let old_off = row + old_x * 4;
                let new_off = row + new_x * 4;
                for c in 0..4 {
                    sum[c] = sum[c] + buf[new_off + c] as u32 - buf[old_off + c] as u32;
                }
            }
        }
        // Vertical pass: tmp -> buf
        for x in 0..rw {
            let mut sum = [0u32; 4];
            for dy in -(radius as isize)..=(radius as isize) {
                let sy = dy.clamp(0, (rh - 1) as isize) as usize;
                let off = (sy * rw + x) * 4;
                for c in 0..4 {
                    sum[c] += tmp[off + c] as u32;
                }
            }
            for y in 0..rh {
                let dst = (y * rw + x) * 4;
                for c in 0..4 {
                    buf[dst + c] = (sum[c] / window) as u8;
                }
                let old_y = (y as isize - radius as isize).clamp(0, (rh - 1) as isize) as usize;
                let new_y = (y as isize + radius as isize + 1).clamp(0, (rh - 1) as isize) as usize;
                let old_off = (old_y * rw + x) * 4;
                let new_off = (new_y * rw + x) * 4;
                for c in 0..4 {
                    sum[c] = sum[c] + tmp[new_off + c] as u32 - tmp[old_off + c] as u32;
                }
            }
        }
    }

    let data = pixmap.data_mut();
    for y in 0..rh {
        let dst = ((ry + y) * pw + rx) * 4;
        let src = y * rw * 4;
        data[dst..dst + rw * 4].copy_from_slice(&buf[src..src + rw * 4]);
    }
}

/// Grid pixelation on a `Pixmap` sub-rect.
pub fn apply_pixelate_region(pixmap: &mut Pixmap, rect: PixelRect, block_size: u32) {
    if rect.is_empty() || block_size <= 1 {
        return;
    }
    let pw = pixmap.width() as usize;
    let data = pixmap.data_mut();
    let bs = block_size;

    let mut by = rect.y;
    while by < rect.bottom() {
        let bh = bs.min(rect.bottom() - by);
        let mut bx = rect.x;
        while bx < rect.right() {
            let bw = bs.min(rect.right() - bx);
            let mut sum = [0u32; 4];
            let count = (bw * bh).max(1);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let off = ((y as usize) * pw + x as usize) * 4;
                    for c in 0..4 {
                        sum[c] += data[off + c] as u32;
                    }
                }
            }
            let avg = [
                (sum[0] / count) as u8,
                (sum[1] / count) as u8,
                (sum[2] / count) as u8,
                (sum[3] / count) as u8,
            ];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let off = ((y as usize) * pw + x as usize) * 4;
                    data[off..off + 4].copy_from_slice(&avg);
                }
            }
            bx += bs;
        }
        by += bs;
    }
}

/// 4. Magnify: zoomed loupe sampling underlying `Pixmap` by `MagnifyScale` (default 200%) with circle/rounded-rect mask.
pub fn render_magnify_object(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let Some(r) = object_bounds_rectf(obj) else { return };
    let scale = (obj
        .extra
        .get("MagnifyScale")
        .and_then(|v| v.as_f64())
        .unwrap_or(200.0) as f32
        / 100.0)
        .clamp(1.1, 8.0);
    let Some(rect) = Rect::from_xywh(r.x as f32, r.y as f32, (r.width as f32).max(8.0), (r.height as f32).max(8.0)) else {
        return;
    };

    let dst_w = (rect.width() * transform.sx).round().max(1.0) as u32;
    let dst_h = (rect.height() * transform.sy).round().max(1.0) as u32;
    let dst_x = (rect.x() * transform.sx + transform.tx).round() as i32;
    let dst_y = (rect.y() * transform.sy + transform.ty).round() as i32;

    let cx = dst_x as f32 + dst_w as f32 * 0.5;
    let cy = dst_y as f32 + dst_h as f32 * 0.5;

    let Some(mut zoomed) = Pixmap::new(dst_w, dst_h) else { return };
    let src = pixmap.data();
    let sw = pixmap.width() as i32;
    let sh = pixmap.height() as i32;
    let zd = zoomed.data_mut();

    for y in 0..dst_h {
        let sy = (cy + (y as f32 - dst_h as f32 * 0.5) / scale).round() as i32;
        let sy = sy.clamp(0, sh - 1) as usize;
        for x in 0..dst_w {
            let sx = (cx + (x as f32 - dst_w as f32 * 0.5) / scale).round() as i32;
            let sx = sx.clamp(0, sw - 1) as usize;
            let s_off = (sy * sw as usize + sx) * 4;
            let d_off = ((y as usize) * dst_w as usize + x as usize) * 4;
            zd[d_off..d_off + 4].copy_from_slice(&src[s_off..s_off + 4]);
        }
    }

    let shape_kind = obj.tool_shape.as_deref().unwrap_or("Ellipse");
    let clip_path = if shape_kind == "Ellipse" || shape_kind == "Circle" {
        PathBuilder::from_oval(rect)
    } else {
        build_rounded_rect_path(rect, rect.width().min(rect.height()) * 0.12)
    };
    let Some(clip_path) = clip_path else { return };

    let mut mask = Mask::new(pixmap.width(), pixmap.height());
    if let Some(ref mut m) = mask {
        m.fill_path(&clip_path, FillRule::Winding, true, transform);
    }
    pixmap.draw_pixmap(
        dst_x,
        dst_y,
        zoomed.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        mask.as_ref(),
    );

    let fg = obj.foreground_color.unwrap_or(ColorRgba::rgb(40, 40, 40));
    let stroke_w = obj.stroke_width.unwrap_or(3.0).max(1.0) as f32;
    let mut border_paint = Paint::default();
    border_paint.anti_alias = true;
    border_paint.set_color(to_skia_color(fg, 1.0));
    let stroke = Stroke {
        width: stroke_w,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&clip_path, &border_paint, &stroke, transform, None);
}

/// 5. Cut Out: removes a horizontal `[start..end]` or vertical `[start..end]` strip with
/// straight (`0`), sawtooth (`1..=5`), or seeded torn-edge (`6..=15`) seam + subtle shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutOutOrientation {
    Horizontal,
    Vertical,
}

pub fn cut_out_strip(
    bmp: &TiledBitmap,
    orientation: CutOutOrientation,
    start: u32,
    end: u32,
    style: i32,
    seed: u64,
) -> TiledBitmap {
    let w = bmp.width();
    let h = bmp.height();
    let (lo, hi) = if start <= end { (start, end) } else { (end, start) };
    let src = bmp.to_rgba();

    let seam_offset = |pos: u32| -> i32 {
        match style {
            0 => 0, // Straight
            1..=5 => {
                // Sawtooth zigzag
                let period = 16;
                let m = (pos % period) as i32;
                let half = (period / 2) as i32;
                if m < half { m - half / 2 } else { (period as i32 - m) - half / 2 }
            }
            _ => {
                // Seeded deterministic pseudo-random torn paper edge
                let mut x = (pos as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ seed;
                x ^= x >> 30;
                x = x.wrapping_mul(0xBF58476D1CE4E5B9);
                x ^= x >> 27;
                ((x % 9) as i32) - 4
            }
        }
    };

    match orientation {
        CutOutOrientation::Horizontal => {
            let hi = hi.min(h);
            let lo = lo.min(hi);
            let cut_h = hi - lo;
            if cut_h == 0 || cut_h >= h {
                return bmp.clone();
            }
            let new_h = h - cut_h;
            let mut out = vec![0u8; (w as usize) * (new_h as usize) * 4];
            for y in 0..new_h {
                for x in 0..w {
                    let seam = (lo as i32 + seam_offset(x)).clamp(0, new_h as i32 - 1) as u32;
                    let sy = if y < seam { y } else { (y + cut_h).min(h - 1) };
                    let s_off = ((sy as usize) * (w as usize) + x as usize) * 4;
                    let d_off = ((y as usize) * (w as usize) + x as usize) * 4;
                    out[d_off..d_off + 4].copy_from_slice(&src[s_off..s_off + 4]);
                    // Subtle seam shadow on 2px right below seam
                    if y >= seam && y < seam + 2 {
                        for c in 0..3 {
                            out[d_off + c] = (out[d_off + c] as f32 * 0.82) as u8;
                        }
                    }
                }
            }
            TiledBitmap::from_rgba(w, new_h, &out).expect("valid cutout dims")
        }
        CutOutOrientation::Vertical => {
            let hi = hi.min(w);
            let lo = lo.min(hi);
            let cut_w = hi - lo;
            if cut_w == 0 || cut_w >= w {
                return bmp.clone();
            }
            let new_w = w - cut_w;
            let mut out = vec![0u8; (new_w as usize) * (h as usize) * 4];
            for y in 0..h {
                let seam = (lo as i32 + seam_offset(y)).clamp(0, new_w as i32 - 1) as u32;
                for x in 0..new_w {
                    let sx = if x < seam { x } else { (x + cut_w).min(w - 1) };
                    let s_off = ((y as usize) * (w as usize) + sx as usize) * 4;
                    let d_off = ((y as usize) * (new_w as usize) + x as usize) * 4;
                    out[d_off..d_off + 4].copy_from_slice(&src[s_off..s_off + 4]);
                    if x >= seam && x < seam + 2 {
                        for c in 0..3 {
                            out[d_off + c] = (out[d_off + c] as f32 * 0.82) as u8;
                        }
                    }
                }
            }
            TiledBitmap::from_rgba(new_w, h, &out).expect("valid cutout dims")
        }
    }
}

/// 6a. Auto-Trim: finds the tight bounding rect excluding outer borders matching top-left pixel within `tolerance`.
pub fn auto_trim_bounds(bmp: &TiledBitmap, tolerance: u8) -> Option<PixelRect> {
    let w = bmp.width();
    let h = bmp.height();
    if w == 0 || h == 0 {
        return None;
    }
    let bg = bmp.get_pixel(0, 0)?;
    let rgba = bmp.to_rgba();
    let tol = tolerance as i32;

    let is_bg = |x: u32, y: u32| -> bool {
        let off = ((y as usize) * (w as usize) + x as usize) * 4;
        (rgba[off] as i32 - bg[0] as i32).abs() <= tol
            && (rgba[off + 1] as i32 - bg[1] as i32).abs() <= tol
            && (rgba[off + 2] as i32 - bg[2] as i32).abs() <= tol
            && (rgba[off + 3] as i32 - bg[3] as i32).abs() <= tol
    };

    let mut top = 0;
    while top < h && (0..w).all(|x| is_bg(x, top)) {
        top += 1;
    }
    if top == h {
        return None; // Entire image is uniform background
    }
    let mut bottom = h - 1;
    while bottom > top && (0..w).all(|x| is_bg(x, bottom)) {
        bottom -= 1;
    }
    let mut left = 0;
    while left < w && (top..=bottom).all(|y| is_bg(left, y)) {
        left += 1;
    }
    let mut right = w - 1;
    while right > left && (top..=bottom).all(|y| is_bg(right, y)) {
        right -= 1;
    }

    Some(PixelRect::new(left, top, right - left + 1, bottom - top + 1))
}

fn translate_object(obj: &mut CaptureObject, dx: f64, dy: f64) {
    if let Some(ref mut p) = obj.start_point {
        p.x += dx;
        p.y += dy;
    }
    if let Some(ref mut p) = obj.end_point {
        p.x += dx;
        p.y += dy;
    }
    if let Some(ref mut pts) = obj.points_array {
        for p in pts {
            p.x += dx;
            p.y += dy;
        }
    }
    if let Some(ref mut tails) = obj.tail_points {
        for p in tails {
            p.x += dx;
            p.y += dy;
        }
    }
}

/// 6b. Crop Document: crops `session.bitmap` to `rect` AND translates all `CaptureObjects` by `(-rect.x, -rect.y)`
/// in a single undoable `CanvasResize` history step!
pub fn crop_document(session: &mut DocumentSession, rect: PixelRect) -> bool {
    let Some(clipped) = rect.intersect(session.bitmap.bounds()) else {
        return false;
    };
    let before_bitmap = session.bitmap.clone();
    let before_objects = session.page.capture_objects.clone();

    let after_bitmap = session.bitmap.crop(clipped);
    let dx = -(clipped.x as f64);
    let dy = -(clipped.y as f64);

    for obj in &mut session.page.capture_objects {
        translate_object(obj, dx, dy);
    }
    if let Some(ref mut sections) = session.page.sections {
        for sec in sections {
            if let Some(ref mut graphics) = sec.section_graphics {
                for obj in graphics {
                    translate_object(obj, dx, dy);
                }
            }
        }
    }

    session.page.capture_canvas_width = Some(after_bitmap.width() as f64);
    session.page.capture_canvas_height = Some(after_bitmap.height() as f64);
    let after_objects = session.page.capture_objects.clone();
    session.bitmap = after_bitmap.clone();

    session.history.push(HistoryCommand::CanvasResize {
        before_bitmap,
        after_bitmap,
        before_objects,
        after_objects,
    });
    session
        .dirty
        .mark_all(session.bitmap.width(), session.bitmap.height());
    true
}
