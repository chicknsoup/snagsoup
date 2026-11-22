use snagsoup_canvas::{PixelRect, TiledBitmap};
use snagsoup_model::{CaptureObject, ColorRgba, PointF, SnagPalette, ToolMode};
use std::collections::VecDeque;

/// Auto-Simplify SUI (`SuiMorphologyBasedTextBlockDetector` parity from `TSCV.Recognition.dll`):
/// Sobel gradient -> threshold -> horizontal morphological close scaled by `sui_detail` (1..=10) ->
/// connected components -> palette color assignment -> `ToolMode::Simplify` `CaptureObject`s.
pub fn auto_simplify(
    bmp: &TiledBitmap,
    palette: Option<&SnagPalette>,
    sui_detail: u8,
) -> Vec<CaptureObject> {
    let w = bmp.width() as usize;
    let h = bmp.height() as usize;
    if w < 8 || h < 8 {
        return Vec::new();
    }

    let detail = sui_detail.clamp(1, 10);
    let rgba = bmp.to_rgba();

    // 1. Grayscale luminance
    let mut gray = vec![0u8; w * h];
    for i in 0..w * h {
        let off = i * 4;
        gray[i] = ((rgba[off] as u32 * 77 + rgba[off + 1] as u32 * 150 + rgba[off + 2] as u32 * 29)
            >> 8) as u8;
    }

    // 2. Sobel edge magnitude -> binary mask
    let mut edges = vec![0u8; w * h];
    let edge_thresh = 48i32;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let tl = gray[(y - 1) * w + (x - 1)] as i32;
            let tc = gray[(y - 1) * w + x] as i32;
            let tr = gray[(y - 1) * w + (x + 1)] as i32;
            let ml = gray[y * w + (x - 1)] as i32;
            let mr = gray[y * w + (x + 1)] as i32;
            let bl = gray[(y + 1) * w + (x - 1)] as i32;
            let bc = gray[(y + 1) * w + x] as i32;
            let br = gray[(y + 1) * w + (x + 1)] as i32;

            let gx = -tl + tr - 2 * ml + 2 * mr - bl + br;
            let gy = -tl - 2 * tc - tr + bl + 2 * bc + br;
            if gx.abs() + gy.abs() >= edge_thresh {
                edges[y * w + x] = 1;
            }
        }
    }

    // 3. Horizontal morphological close (dilate then erode) scaled by detail (1=coarse, 10=fine)
    let horiz_r = (13usize.saturating_sub(detail as usize)).clamp(2, 12);
    let mut dilated = vec![0u8; w * h];
    for y in 0..h {
        let row = y * w;
        let mut last_one: Option<usize> = None;
        for x in 0..w {
            if edges[row + x] != 0 {
                last_one = Some(x);
            }
            if let Some(lx) = last_one {
                if x - lx <= horiz_r {
                    dilated[row + x] = 1;
                }
            }
        }
        let mut next_one: Option<usize> = None;
        for x in (0..w).rev() {
            if edges[row + x] != 0 {
                next_one = Some(x);
            }
            if let Some(nx) = next_one {
                if nx - x <= horiz_r {
                    dilated[row + x] = 1;
                }
            }
        }
    }

    // Light vertical 1px dilation to merge multi-pixel strokes
    let mut closed = dilated.clone();
    for y in 1..h - 1 {
        for x in 0..w {
            if dilated[(y - 1) * w + x] != 0 || dilated[(y + 1) * w + x] != 0 {
                closed[y * w + x] = 1;
            }
        }
    }

    // 4. Connected components -> bounding rectangles
    let mut visited = vec![false; w * h];
    let mut rects: Vec<PixelRect> = Vec::new();
    let mut queue = VecDeque::new();
    let max_area = (w * h) * 65 / 100;

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if closed[idx] == 0 || visited[idx] {
                continue;
            }
            visited[idx] = true;
            queue.push_back((x, y));
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;
            let mut count = 0usize;

            while let Some((cx, cy)) = queue.pop_front() {
                count += 1;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);

                for (dx, dy) in [(-1isize, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = cx as isize + dx;
                    let ny = cy as isize + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        let nidx = (ny as usize) * w + (nx as usize);
                        if closed[nidx] != 0 && !visited[nidx] {
                            visited[nidx] = true;
                            queue.push_back((nx as usize, ny as usize));
                        }
                    }
                }
            }

            let rw = max_x - min_x + 1;
            let rh = max_y - min_y + 1;
            if rw >= 8 && rh >= 6 && rw * rh <= max_area && count >= 16 {
                rects.push(PixelRect::new(min_x as u32, min_y as u32, rw as u32, rh as u32));
            }
        }
    }

    // 5. Assign harmonious colors from SnagPalette (or default 4-color UI palette)
    let default_colors = [
        ColorRgba::rgb(200, 206, 214),
        ColorRgba::rgb(160, 170, 182),
        ColorRgba::rgb(112, 124, 138),
        ColorRgba::rgb(68, 78, 92),
    ];
    let pal_colors: &[ColorRgba] = match palette {
        Some(p) if !p.palette_colors.is_empty() => &p.palette_colors,
        _ => &default_colors,
    };

    let mut objects = Vec::with_capacity(rects.len());
    for (i, r) in rects.into_iter().enumerate() {
        let sample = sample_rect_mean_color(&rgba, w, r);
        let assigned = nearest_palette_color(sample, pal_colors);
        let corner_ratio = if r.height <= 24 { 0.45 } else { 0.12 };
        objects.push(CaptureObject {
            object_id: Some(format!("sui-{i}")),
            tool_mode: Some(ToolMode::Simplify),
            tool_shape: Some("RoundedRectangle".to_string()),
            start_point: Some(PointF::new(r.x as f64, r.y as f64)),
            end_point: Some(PointF::new(r.right() as f64, r.bottom() as f64)),
            background_color: Some(assigned),
            foreground_color: Some(ColorRgba::TRANSPARENT),
            stroke_width: Some(0.0),
            corner_radius_ratio: Some(corner_ratio),
            opacity: Some(100.0),
            ..CaptureObject::default()
        });
    }
    objects
}

fn sample_rect_mean_color(rgba: &[u8], w: usize, r: PixelRect) -> ColorRgba {
    let mut sum = [0u64; 3];
    let mut count = 0u64;
    let step_x = (r.width / 8).max(1) as usize;
    let step_y = (r.height / 8).max(1) as usize;
    let mut y = r.y as usize;
    while y < r.bottom() as usize {
        let mut x = r.x as usize;
        while x < r.right() as usize {
            let off = (y * w + x) * 4;
            sum[0] += rgba[off] as u64;
            sum[1] += rgba[off + 1] as u64;
            sum[2] += rgba[off + 2] as u64;
            count += 1;
            x += step_x;
        }
        y += step_y;
    }
    if count == 0 {
        return ColorRgba::rgb(128, 128, 128);
    }
    ColorRgba::rgb(
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    )
}

fn nearest_palette_color(sample: ColorRgba, palette: &[ColorRgba]) -> ColorRgba {
    let s_luma = 0.2126 * sample.r as f32 + 0.7152 * sample.g as f32 + 0.0722 * sample.b as f32;
    let mut best = palette[0];
    let mut best_d = f32::INFINITY;
    for &c in palette {
        let c_luma = 0.2126 * c.r as f32 + 0.7152 * c.g as f32 + 0.0722 * c.b as f32;
        let d = (c_luma - s_luma).abs();
        if d < best_d {
            best_d = d;
            best = c;
        }
    }
    best
}
