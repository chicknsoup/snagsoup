//! HDR `DXGI_FORMAT_R16G16B16A16_FLOAT` (scRGB linear) -> sRGB `RGBA8` tonemapping.

/// Decode an IEEE 754 binary16 (half-precision float) into `f32`.
#[inline]
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 1) as u32;
    let exp = ((bits >> 10) & 0x1F) as u32;
    let frac = (bits & 0x03FF) as u32;

    if exp == 0 {
        if frac == 0 {
            return f32::from_bits(sign << 31);
        }
        // Subnormal half-float
        let mut f = frac;
        let mut e: i32 = -14;
        while (f & 0x0400) == 0 {
            f <<= 1;
            e -= 1;
        }
        f &= 0x03FF;
        let f32_exp = (e + 127) as u32;
        return f32::from_bits((sign << 31) | (f32_exp << 23) | (f << 13));
    }
    if exp == 31 {
        let f32_bits = (sign << 31) | (0xFF << 23) | (frac << 13);
        return f32::from_bits(f32_bits);
    }

    let f32_exp = exp + (127 - 15);
    f32::from_bits((sign << 31) | (f32_exp << 23) | (frac << 13))
}

#[inline]
fn linear_to_srgb(v: f32) -> u8 {
    let clamped = v.clamp(0.0, 1.0);
    let srgb = if clamped <= 0.003_130_8 {
        clamped * 12.92
    } else {
        1.055 * clamped.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Soft Reinhard highlight roll-off for HDR values > 1.0 while preserving SDR `[0, 1]` contrast.
#[inline]
fn tonemap_channel(linear_val: f32, sdr_scale: f32) -> u8 {
    let v = (linear_val * sdr_scale).max(0.0);
    // Preserve SDR range smoothly; compress HDR peaks > 1.0 via Reinhard shoulder.
    let mapped = if v <= 0.8 {
        v
    } else {
        let excess = v - 0.8;
        0.8 + (excess / (1.0 + excess * 2.5)) * 0.2
    };
    linear_to_srgb(mapped)
}

/// Convert an `R16G16B16A16_FLOAT` (8 bytes/pixel) surface with `row_pitch` into tightly
/// packed sRGB `RGBA8` (`width * height * 4` bytes).
///
/// In Windows scRGB HDR, `1.0` equals 80 nits; `sdr_white_nits` is typically 80.0–240.0
/// (pass `80.0` for 1:1 SDR white or higher if normalizing brighter HDR desktops).
pub fn tonemap_scrgb_f16_to_rgba8(
    src: &[u8],
    width: u32,
    height: u32,
    row_pitch: usize,
    sdr_white_nits: f32,
) -> Vec<u8> {
    let sdr_scale = if sdr_white_nits > 0.0 {
        80.0 / sdr_white_nits
    } else {
        1.0
    };
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h * 4];

    for y in 0..h {
        let row_start = y * row_pitch;
        let dst_row = &mut out[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            let off = row_start + x * 8;
            let r_bits = u16::from_le_bytes([src[off], src[off + 1]]);
            let g_bits = u16::from_le_bytes([src[off + 2], src[off + 3]]);
            let b_bits = u16::from_le_bytes([src[off + 4], src[off + 5]]);

            let r = f16_to_f32(r_bits);
            let g = f16_to_f32(g_bits);
            let b = f16_to_f32(b_bits);

            let d = &mut dst_row[x * 4..x * 4 + 4];
            d[0] = tonemap_channel(r, sdr_scale);
            d[1] = tonemap_channel(g, sdr_scale);
            d[2] = tonemap_channel(b, sdr_scale);
            d[3] = 255;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_decode_basic_values() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0x4000), 2.0);
        assert_eq!(f16_to_f32(0xBC00), -1.0);
        assert!((f16_to_f32(0x3800) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hdr_scrgb_tonemap_produces_valid_srgb_without_clipping() {
        // 2x1 HDR pixels: (0.0, 0.5, 1.0, 1.0) and (4.0, 0.0, 0.25, 1.0)
        let mut src = Vec::new();
        for bits in [0x0000u16, 0x3800, 0x3C00, 0x3C00, 0x4400, 0x0000, 0x3400, 0x3C00] {
            src.extend_from_slice(&bits.to_le_bytes());
        }
        let rgba = tonemap_scrgb_f16_to_rgba8(&src, 2, 1, 16, 80.0);
        assert_eq!(rgba.len(), 8);
        assert_eq!(rgba[0], 0); // linear 0.0 -> 0
        assert!(rgba[1] > 180 && rgba[1] < 200); // linear 0.5 -> ~188 sRGB
        assert!(rgba[2] >= 220); // linear 1.0 -> near-white
        assert_eq!(rgba[3], 255);
        assert!(rgba[4] >= 230); // linear 4.0 HDR peak compressed cleanly <= 255
        assert_eq!(rgba[7], 255);
    }
}
