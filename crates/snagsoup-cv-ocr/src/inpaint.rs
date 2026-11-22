use snagsoup_canvas::{PixelRect, TiledBitmap};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InpaintTierUsed {
    UiFlat,
    UiLinearGradientX,
    UiLinearGradientY,
    BoundaryDiffusion,
}

/// Two-tier smart inpainter matching `TSCV.Inpainting.dll`:
/// - Tier 1 (`UiFastInpainter`, < 2ms): detects flat or 1D linear-gradient border around `rect` and interpolates cleanly.
/// - Tier 2 (`BoundaryDiffusion` / Telea-style inward propagation): fills textured/photo backgrounds smoothly.
pub fn inpaint_rect(bmp: &mut TiledBitmap, rect: PixelRect) -> Option<InpaintTierUsed> {
    let clipped = rect.intersect(bmp.bounds())?;
    let mask = vec![true; (clipped.width as usize) * (clipped.height as usize)];
    inpaint_region(bmp, &mask, clipped)
}

/// Inpaints arbitrary masked pixels (`mask` has length `rect.width * rect.height`) in `bmp`.
pub fn inpaint_region(
    bmp: &mut TiledBitmap,
    mask: &[bool],
    rect: PixelRect,
) -> Option<InpaintTierUsed> {
    let clipped = rect.intersect(bmp.bounds())?;
    let rw = clipped.width as usize;
    let rh = clipped.height as usize;
    if rw == 0 || rh == 0 || mask.len() < rw * rh {
        return None;
    }

    // Read an expanded ring (+2px) around clipped rect for border sampling
    let pad = 2u32;
    let ex0 = clipped.x.saturating_sub(pad);
    let ey0 = clipped.y.saturating_sub(pad);
    let ex1 = (clipped.right() + pad).min(bmp.width());
    let ey1 = (clipped.bottom() + pad).min(bmp.height());
    let ew = (ex1 - ex0) as usize;
    let eh = (ey1 - ey0) as usize;
    let ox = (clipped.x - ex0) as usize;
    let oy = (clipped.y - ey0) as usize;

    let mut patch = bmp.read_region(PixelRect::new(ex0, ey0, ew as u32, eh as u32));
    let mut is_masked = vec![false; ew * eh];
    for y in 0..rh {
        for x in 0..rw {
            if mask[y * rw + x] {
                is_masked[(oy + y) * ew + (ox + x)] = true;
            }
        }
    }

    // Collect valid border pixels adjacent to the mask
    let mut border_samples: Vec<(f32, f32, [f32; 4])> = Vec::new();
    for y in 0..eh {
        for x in 0..ew {
            if is_masked[y * ew + x] {
                continue;
            }
            let touches_mask = (x > 0 && is_masked[y * ew + x - 1])
                || (x + 1 < ew && is_masked[y * ew + x + 1])
                || (y > 0 && is_masked[(y - 1) * ew + x])
                || (y + 1 < eh && is_masked[(y + 1) * ew + x]);
            if touches_mask {
                let off = (y * ew + x) * 4;
                border_samples.push((
                    x as f32,
                    y as f32,
                    [
                        patch[off] as f32,
                        patch[off + 1] as f32,
                        patch[off + 2] as f32,
                        patch[off + 3] as f32,
                    ],
                ));
            }
        }
    }

    if border_samples.is_empty() {
        return None;
    }

    // --- Tier 1a: Flat background check (std-dev < 3.5 across RGB) ---
    let n = border_samples.len() as f32;
    let mut mean = [0.0f32; 4];
    for &(_, _, c) in &border_samples {
        for k in 0..4 {
            mean[k] += c[k];
        }
    }
    for k in 0..4 {
        mean[k] /= n;
    }

    let mut var_sum = 0.0f32;
    for &(_, _, c) in &border_samples {
        for k in 0..3 {
            let d = c[k] - mean[k];
            var_sum += d * d;
        }
    }
    let rms_flat = (var_sum / (n * 3.0)).sqrt();

    if rms_flat < 3.5 {
        let fill = [
            mean[0].round().clamp(0.0, 255.0) as u8,
            mean[1].round().clamp(0.0, 255.0) as u8,
            mean[2].round().clamp(0.0, 255.0) as u8,
            mean[3].round().clamp(0.0, 255.0) as u8,
        ];
        bmp.mutate_region(clipped, |x, y, px| {
            let lx = (x - clipped.x) as usize;
            let ly = (y - clipped.y) as usize;
            if mask[ly * rw + lx] {
                *px = fill;
            }
        });
        return Some(InpaintTierUsed::UiFlat);
    }

    // --- Tier 1b: 1D Linear Gradient fit along X or Y ---
    let (slope_x, intercept_x, rms_x) = fit_1d_linear(&border_samples, true);
    let (slope_y, intercept_y, rms_y) = fit_1d_linear(&border_samples, false);

    if rms_x < 4.5 && rms_x <= rms_y {
        bmp.mutate_region(clipped, |x, y, px| {
            let lx = (x - clipped.x) as usize;
            let ly = (y - clipped.y) as usize;
            if mask[ly * rw + lx] {
                let px_x = (ox + lx) as f32;
                for c in 0..4 {
                    px[c] = (intercept_x[c] + slope_x[c] * px_x)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        });
        return Some(InpaintTierUsed::UiLinearGradientX);
    }
    if rms_y < 4.5 {
        bmp.mutate_region(clipped, |x, y, px| {
            let lx = (x - clipped.x) as usize;
            let ly = (y - clipped.y) as usize;
            if mask[ly * rw + lx] {
                let px_y = (oy + ly) as f32;
                for c in 0..4 {
                    px[c] = (intercept_y[c] + slope_y[c] * px_y)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        });
        return Some(InpaintTierUsed::UiLinearGradientY);
    }

    // --- Tier 2: Telea-style inward boundary propagation + harmonic smoothing ---
    let mut known: Vec<bool> = is_masked.iter().map(|&m| !m).collect();
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    let mut in_queue = vec![false; ew * eh];

    for y in 0..eh {
        for x in 0..ew {
            if !known[y * ew + x] {
                let has_known_neighbor = (x > 0 && known[y * ew + x - 1])
                    || (x + 1 < ew && known[y * ew + x + 1])
                    || (y > 0 && known[(y - 1) * ew + x])
                    || (y + 1 < eh && known[(y + 1) * ew + x]);
                if has_known_neighbor {
                    queue.push_back((x, y));
                    in_queue[y * ew + x] = true;
                }
            }
        }
    }

    while let Some((x, y)) = queue.pop_front() {
        let mut sum = [0.0f32; 4];
        let mut w_sum = 0.0f32;
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx >= 0 && ny >= 0 && (nx as usize) < ew && (ny as usize) < eh {
                    let nidx = (ny as usize) * ew + (nx as usize);
                    if known[nidx] {
                        let weight = if dx == 0 || dy == 0 { 1.0 } else { 0.707 };
                        for c in 0..4 {
                            sum[c] += patch[nidx * 4 + c] as f32 * weight;
                        }
                        w_sum += weight;
                    }
                }
            }
        }
        let idx = y * ew + x;
        if w_sum > 0.0 {
            for c in 0..4 {
                patch[idx * 4 + c] = (sum[c] / w_sum).round().clamp(0.0, 255.0) as u8;
            }
        }
        known[idx] = true;

        for (dx, dy) in [(-1isize, 0), (1, 0), (0, -1), (0, 1)] {
            let nx = x as isize + dx;
            let ny = y as isize + dy;
            if nx >= 0 && ny >= 0 && (nx as usize) < ew && (ny as usize) < eh {
                let nidx = (ny as usize) * ew + (nx as usize);
                if !known[nidx] && !in_queue[nidx] {
                    in_queue[nidx] = true;
                    queue.push_back((nx as usize, ny as usize));
                }
            }
        }
    }

    // 4 passes of harmonic relaxation inside masked pixels for smooth seamless blend
    for _ in 0..4 {
        let prev = patch.clone();
        for y in 1..eh.saturating_sub(1) {
            for x in 1..ew.saturating_sub(1) {
                if is_masked[y * ew + x] {
                    let u = ((y - 1) * ew + x) * 4;
                    let d = ((y + 1) * ew + x) * 4;
                    let l = (y * ew + x - 1) * 4;
                    let r = (y * ew + x + 1) * 4;
                    let dst = (y * ew + x) * 4;
                    for c in 0..4 {
                        patch[dst + c] = ((prev[u + c] as u32
                            + prev[d + c] as u32
                            + prev[l + c] as u32
                            + prev[r + c] as u32)
                            / 4) as u8;
                    }
                }
            }
        }
    }

    bmp.mutate_region(clipped, |x, y, px| {
        let lx = (x - clipped.x) as usize;
        let ly = (y - clipped.y) as usize;
        if mask[ly * rw + lx] {
            let off = ((oy + ly) * ew + (ox + lx)) * 4;
            px.copy_from_slice(&patch[off..off + 4]);
        }
    });
    Some(InpaintTierUsed::BoundaryDiffusion)
}

fn fit_1d_linear(samples: &[(f32, f32, [f32; 4])], along_x: bool) -> ([f32; 4], [f32; 4], f32) {
    let n = samples.len() as f32;
    let mut sum_t = 0.0f32;
    let mut sum_c = [0.0f32; 4];
    for &(x, y, c) in samples {
        let t = if along_x { x } else { y };
        sum_t += t;
        for k in 0..4 {
            sum_c[k] += c[k];
        }
    }
    let mean_t = sum_t / n;
    let mean_c = [
        sum_c[0] / n,
        sum_c[1] / n,
        sum_c[2] / n,
        sum_c[3] / n,
    ];

    let mut denom = 0.0f32;
    let mut numer = [0.0f32; 4];
    for &(x, y, c) in samples {
        let dt = (if along_x { x } else { y }) - mean_t;
        denom += dt * dt;
        for k in 0..4 {
            numer[k] += dt * (c[k] - mean_c[k]);
        }
    }

    let mut slope = [0.0f32; 4];
    let mut intercept = mean_c;
    if denom > 1e-4 {
        for k in 0..4 {
            slope[k] = numer[k] / denom;
            intercept[k] = mean_c[k] - slope[k] * mean_t;
        }
    }

    let mut err_sq = 0.0f32;
    for &(x, y, c) in samples {
        let t = if along_x { x } else { y };
        for k in 0..3 {
            let pred = intercept[k] + slope[k] * t;
            let d = pred - c[k];
            err_sq += d * d;
        }
    }
    let rms = (err_sq / (n * 3.0)).sqrt();
    (slope, intercept, rms)
}
