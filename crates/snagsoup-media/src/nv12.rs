//! Fast even-dimension rounding and `RGBA8`/`BGRA8` -> `NV12` (YUV 4:2:0) color conversion
//! for H.264 Media Foundation encoding.

/// Round dimensions down to the nearest even numbers (`>= 2`) required by H.264 / NV12.
#[inline]
pub const fn even_dimensions(width: u32, height: u32) -> (u32, u32) {
    let w = width & !1;
    let h = height & !1;
    (if w < 2 { 2 } else { w }, if h < 2 { 2 } else { h })
}

/// Convert straight `RGBA8` (`src_w x src_h`) into tightly packed `NV12` (`even_w x even_h`),
/// using BT.601 studio-swing integer math (no floating-point bottleneck).
pub fn rgba_to_nv12(rgba: &[u8], src_w: u32, src_h: u32) -> Vec<u8> {
    convert_rgbx_to_nv12(rgba, src_w, src_h, false)
}

/// Convert `BGRA8` (`src_w x src_h`) into tightly packed `NV12` (`even_w x even_h`).
pub fn bgra_to_nv12(bgra: &[u8], src_w: u32, src_h: u32) -> Vec<u8> {
    convert_rgbx_to_nv12(bgra, src_w, src_h, true)
}

fn convert_rgbx_to_nv12(src: &[u8], src_w: u32, src_h: u32, is_bgra: bool) -> Vec<u8> {
    let (w, h) = even_dimensions(src_w, src_h);
    let w_usize = w as usize;
    let h_usize = h as usize;
    let src_stride = (src_w as usize) * 4;

    let y_plane_size = w_usize * h_usize;
    let uv_plane_size = y_plane_size / 2;
    let mut nv12 = vec![0u8; y_plane_size + uv_plane_size];

    let (y_plane, uv_plane) = nv12.split_at_mut(y_plane_size);

    let (r_off, b_off) = if is_bgra { (2, 0) } else { (0, 2) };

    // Y plane
    for y in 0..h_usize {
        let srow = &src[y * src_stride..y * src_stride + w_usize * 4];
        let yrow = &mut y_plane[y * w_usize..(y + 1) * w_usize];
        for x in 0..w_usize {
            let p = &srow[x * 4..x * 4 + 4];
            let r = p[r_off] as i32;
            let g = p[1] as i32;
            let b = p[b_off] as i32;
            yrow[x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(16, 235) as u8;
        }
    }

    // Interleaved UV plane (2x2 box average)
    for y in (0..h_usize).step_by(2) {
        let srow0 = &src[y * src_stride..y * src_stride + w_usize * 4];
        let srow1 = &src[(y + 1) * src_stride..(y + 1) * src_stride + w_usize * 4];
        let uvrow = &mut uv_plane[(y / 2) * w_usize..(y / 2 + 1) * w_usize];
        for x in (0..w_usize).step_by(2) {
            let i0 = x * 4;
            let i1 = (x + 1) * 4;
            let r = (srow0[i0 + r_off] as i32
                + srow0[i1 + r_off] as i32
                + srow1[i0 + r_off] as i32
                + srow1[i1 + r_off] as i32)
                >> 2;
            let g = (srow0[i0 + 1] as i32
                + srow0[i1 + 1] as i32
                + srow1[i0 + 1] as i32
                + srow1[i1 + 1] as i32)
                >> 2;
            let b = (srow0[i0 + b_off] as i32
                + srow0[i1 + b_off] as i32
                + srow1[i0 + b_off] as i32
                + srow1[i1 + b_off] as i32)
                >> 2;

            let u = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
            let v = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
            uvrow[x] = u;
            uvrow[x + 1] = v;
        }
    }

    nv12
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_dimensions_rounds_odd_values_down_to_even() {
        assert_eq!(even_dimensions(1921, 1081), (1920, 1080));
        assert_eq!(even_dimensions(1280, 720), (1280, 720));
        assert_eq!(even_dimensions(1, 3), (2, 2));
    }

    #[test]
    fn rgba_to_nv12_produces_exact_plane_size_and_neutral_white() {
        // 5x3 white image -> cropped to 4x2 even NV12 (4*2*3/2 = 12 bytes)
        let rgba = vec![255u8; 5 * 3 * 4];
        let nv12 = rgba_to_nv12(&rgba, 5, 3);
        assert_eq!(nv12.len(), 4 * 2 * 3 / 2);
        // Y plane should be 235 (studio white), UV plane should be 128 (neutral chroma)
        assert!(nv12[..8].iter().all(|&y| y == 235));
        assert!(nv12[8..].iter().all(|&uv| uv == 128));
    }
}
