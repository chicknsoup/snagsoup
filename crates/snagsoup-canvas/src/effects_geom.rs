use crate::compositor::downscale_rgba_bilinear;
use crate::effects_color::{
    apply_color_adjustment, apply_color_replacement, apply_filter, apply_spotlight_and_magnify,
    box_blur_rgba_in_place,
};
use crate::raster_tools::auto_trim_bounds;
use crate::text_render::{render_text_box, HorizontalAlign, TextRenderOptions, VerticalAlign};
use crate::tile_bitmap::TiledBitmap;
use snagsoup_model::{ColorRgba, EffectConfig};
use std::f32::consts::PI;
use tiny_skia::{Pixmap, Transform};

/// Applies a single `EffectConfig` to an RGBA8 image buffer, returning `(new_w, new_h, new_rgba)`.
pub fn apply_effect(w: u32, h: u32, rgba: Vec<u8>, effect: &EffectConfig) -> (u32, u32, Vec<u8>) {
    if w == 0 || h == 0 || rgba.len() < (w as usize) * (h as usize) * 4 {
        return (w, h, rgba);
    }
    match effect {
        EffectConfig::Filters { kind, intensity } => {
            let mut out = rgba;
            apply_filter(w, h, &mut out, kind, *intensity);
            (w, h, out)
        }
        EffectConfig::ColorAdjustment {
            brightness,
            contrast,
            hue,
            saturation,
            gamma,
        } => {
            let mut out = rgba;
            apply_color_adjustment(&mut out, *brightness, *contrast, *hue, *saturation, *gamma);
            (w, h, out)
        }
        EffectConfig::ColorReplacement {
            source,
            target,
            tolerance,
            preserve_luminance,
        } => {
            let mut out = rgba;
            apply_color_replacement(&mut out, *source, *target, *tolerance, *preserve_luminance);
            (w, h, out)
        }
        EffectConfig::SpotlightAndMagnify {
            x,
            y,
            width,
            height,
            dim_opacity,
            blur_radius,
            magnify_scale,
        } => {
            let mut out = rgba;
            apply_spotlight_and_magnify(
                w,
                h,
                &mut out,
                *x,
                *y,
                *width,
                *height,
                *dim_opacity,
                *blur_radius,
                *magnify_scale,
            );
            (w, h, out)
        }
        EffectConfig::Border {
            width,
            color,
            inside,
        } => apply_border(w, h, &rgba, *width, *color, *inside),
        EffectConfig::Shadow {
            offset_x,
            offset_y,
            blur_radius,
            opacity,
            color,
        } => apply_shadow(w, h, &rgba, *offset_x, *offset_y, *blur_radius, *opacity, *color),
        EffectConfig::Edges {
            style,
            size,
            edges_mask,
            seed,
        } => apply_edges(w, h, &rgba, style, *size, *edges_mask, *seed),
        EffectConfig::PageCurl {
            corner,
            radius,
            background_color,
        } => apply_page_curl(w, h, &rgba, corner, *radius, *background_color),
        EffectConfig::Perspective {
            angle_x,
            angle_y,
            reflection,
            reflection_opacity,
        } => apply_perspective(w, h, &rgba, *angle_x, *angle_y, *reflection, *reflection_opacity),
        EffectConfig::Watermark {
            image_path,
            anchor,
            scale,
            opacity,
            emboss,
        } => {
            if let Ok(bytes) = std::fs::read(image_path) {
                if let Ok(wm_bmp) = TiledBitmap::from_png_bytes(&bytes) {
                    return apply_watermark(
                        w,
                        h,
                        &rgba,
                        &wm_bmp.to_rgba(),
                        wm_bmp.width(),
                        wm_bmp.height(),
                        anchor,
                        *scale,
                        *opacity,
                        *emboss,
                    );
                }
            }
            (w, h, rgba)
        }
        EffectConfig::Caption {
            text,
            position,
            font_family,
            font_size,
            text_color,
            background_color,
        } => apply_caption(
            w,
            h,
            &rgba,
            text,
            position,
            font_family,
            *font_size,
            *text_color,
            *background_color,
        ),
        EffectConfig::Trim {
            tolerance,
            auto_trim,
        } => {
            if *auto_trim {
                apply_trim(w, h, &rgba, *tolerance)
            } else {
                (w, h, rgba)
            }
        }
        EffectConfig::Rotate {
            angle_degrees,
            background_color,
        } => apply_rotate(w, h, &rgba, *angle_degrees, *background_color),
        EffectConfig::Flip {
            horizontal,
            vertical,
        } => apply_flip(w, h, &rgba, *horizontal, *vertical),
        EffectConfig::Scale {
            width,
            height,
            keep_aspect_ratio,
            ..
        } => apply_scale(w, h, &rgba, *width, *height, *keep_aspect_ratio),
    }
}

/// Applies a sequence of `EffectConfig`s in order.
pub fn apply_effects(
    mut w: u32,
    mut h: u32,
    mut rgba: Vec<u8>,
    effects: &[EffectConfig],
) -> (u32, u32, Vec<u8>) {
    for eff in effects {
        let (nw, nh, next) = apply_effect(w, h, rgba, eff);
        w = nw;
        h = nh;
        rgba = next;
    }
    (w, h, rgba)
}

/// Fast thumbnail preview helper for the UI Effects Gallery (downscales first to `max_preview_dim`).
pub fn preview_effect_on_rgba(
    w: u32,
    h: u32,
    rgba: &[u8],
    effect: &EffectConfig,
    max_preview_dim: u32,
) -> (u32, u32, Vec<u8>) {
    if w == 0 || h == 0 || max_preview_dim == 0 {
        return (w, h, rgba.to_vec());
    }
    let scale = (max_preview_dim as f32 / w.max(h) as f32).min(1.0);
    let pw = ((w as f32 * scale).round() as u32).max(1);
    let ph = ((h as f32 * scale).round() as u32).max(1);
    let small = downscale_rgba_bilinear(rgba, w, h, pw, ph);
    apply_effect(pw, ph, small, effect)
}

/// 5. Border: inner or outer solid border.
pub fn apply_border(
    w: u32,
    h: u32,
    rgba: &[u8],
    border_w: u32,
    color: ColorRgba,
    inside: bool,
) -> (u32, u32, Vec<u8>) {
    if border_w == 0 {
        return (w, h, rgba.to_vec());
    }
    let px = [color.r, color.g, color.b, color.a];
    if inside {
        let mut out = rgba.to_vec();
        for y in 0..h {
            for x in 0..w {
                if x < border_w
                    || y < border_w
                    || x + border_w >= w
                    || y + border_w >= h
                {
                    let off = ((y as usize) * (w as usize) + x as usize) * 4;
                    out[off..off + 4].copy_from_slice(&px);
                }
            }
        }
        (w, h, out)
    } else {
        let nw = w + border_w * 2;
        let nh = h + border_w * 2;
        let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
        for chunk in out.chunks_exact_mut(4) {
            chunk.copy_from_slice(&px);
        }
        for y in 0..h {
            let src_off = ((y as usize) * (w as usize)) * 4;
            let dst_off = (((y + border_w) as usize) * (nw as usize) + border_w as usize) * 4;
            out[dst_off..dst_off + (w as usize) * 4]
                .copy_from_slice(&rgba[src_off..src_off + (w as usize) * 4]);
        }
        (nw, nh, out)
    }
}

/// 6. Shadow: 9-direction canvas drop shadow with blur & opacity.
#[allow(clippy::too_many_arguments)]
pub fn apply_shadow(
    w: u32,
    h: u32,
    rgba: &[u8],
    offset_x: i32,
    offset_y: i32,
    blur_radius: u32,
    opacity: f32,
    color: ColorRgba,
) -> (u32, u32, Vec<u8>) {
    let br = blur_radius.min(64) as i32;
    let pad_l = (-offset_x + br).max(0) as u32;
    let pad_t = (-offset_y + br).max(0) as u32;
    let pad_r = (offset_x + br).max(0) as u32;
    let pad_b = (offset_y + br).max(0) as u32;
    let nw = w + pad_l + pad_r;
    let nh = h + pad_t + pad_b;

    let mut shadow = vec![0u8; (nw as usize) * (nh as usize) * 4];
    let op = opacity.clamp(0.0, 1.0);
    let sx0 = (pad_l as i32 + offset_x).max(0) as u32;
    let sy0 = (pad_t as i32 + offset_y).max(0) as u32;

    for y in 0..h {
        for x in 0..w {
            let s_a = rgba[((y as usize) * (w as usize) + x as usize) * 4 + 3];
            if s_a > 0 {
                let dx = sx0 + x;
                let dy = sy0 + y;
                if dx < nw && dy < nh {
                    let off = ((dy as usize) * (nw as usize) + dx as usize) * 4;
                    shadow[off] = color.r;
                    shadow[off + 1] = color.g;
                    shadow[off + 2] = color.b;
                    shadow[off + 3] = ((s_a as f32) * (color.a as f32 / 255.0) * op).round() as u8;
                }
            }
        }
    }

    if blur_radius > 0 {
        box_blur_rgba_in_place(nw, nh, &mut shadow, (blur_radius as usize).min(32));
    }

    // Alpha-composite original image over blurred shadow
    for y in 0..h {
        for x in 0..w {
            let s_off = ((y as usize) * (w as usize) + x as usize) * 4;
            let d_off = (((y + pad_t) as usize) * (nw as usize) + (x + pad_l) as usize) * 4;
            alpha_over(&mut shadow[d_off..d_off + 4], &rgba[s_off..s_off + 4]);
        }
    }
    (nw, nh, shadow)
}

/// 7. Edges: `Torn`, `Wave`, `Sawtooth`, `Sharktooth`, `Bevel`, `Fade` on selectable 1..4 sides.
/// `edges_mask` bits: `1=Top, 2=Right, 4=Bottom, 8=Left` (`0` = all 4 sides).
pub fn apply_edges(
    w: u32,
    h: u32,
    rgba: &[u8],
    style: &str,
    size: u32,
    edges_mask: u8,
    seed: u64,
) -> (u32, u32, Vec<u8>) {
    let mut out = rgba.to_vec();
    let mask = if edges_mask == 0 { 0x0F } else { edges_mask };
    let depth = size.max(1).min(w.min(h) / 2).max(1);

    let noise1d = |pos: u32, side: u64| -> f32 {
        let period = (depth / 2).max(4);
        let i0 = pos / period;
        let frac = (pos % period) as f32 / period as f32;
        let hash = |n: u32| -> f32 {
            let mut x = (n as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ seed ^ (side * 0x517CC1B7);
            x ^= x >> 30;
            x = x.wrapping_mul(0xBF58476D1CE4E5B9);
            x ^= x >> 27;
            (x & 0xFFFF) as f32 / 65535.0
        };
        let v0 = hash(i0);
        let v1 = hash(i0 + 1);
        v0 * (1.0 - frac) + v1 * frac
    };

    let edge_cut = |pos: u32, side: u64| -> f32 {
        match style {
            "Wave" => {
                let wave = ((pos as f32 / (depth as f32 * 0.6).max(2.0)) * PI).sin() * 0.5 + 0.5;
                wave * depth as f32
            }
            "Sawtooth" => {
                let p = (depth * 2).max(4);
                let m = pos % p;
                let half = p / 2;
                let t = if m < half {
                    m as f32 / half as f32
                } else {
                    (p - m) as f32 / half as f32
                };
                t * depth as f32
            }
            "Sharktooth" => {
                let p = (depth * 2).max(4);
                ((pos % p) as f32 / p as f32) * depth as f32
            }
            _ => noise1d(pos, side) * depth as f32, // Torn
        }
    };

    for y in 0..h {
        for x in 0..w {
            let off = ((y as usize) * (w as usize) + x as usize) * 4;
            match style {
                "Fade" => {
                    let mut alpha_mul = 1.0f32;
                    if (mask & 1) != 0 && y < depth {
                        alpha_mul = alpha_mul.min(y as f32 / depth as f32);
                    }
                    if (mask & 2) != 0 && w - 1 - x < depth {
                        alpha_mul = alpha_mul.min((w - 1 - x) as f32 / depth as f32);
                    }
                    if (mask & 4) != 0 && h - 1 - y < depth {
                        alpha_mul = alpha_mul.min((h - 1 - y) as f32 / depth as f32);
                    }
                    if (mask & 8) != 0 && x < depth {
                        alpha_mul = alpha_mul.min(x as f32 / depth as f32);
                    }
                    out[off + 3] = (out[off + 3] as f32 * alpha_mul).round() as u8;
                }
                "Bevel" => {
                    let mut delta = 0.0f32;
                    if (mask & 1) != 0 && y < depth {
                        delta += (1.0 - y as f32 / depth as f32) * 70.0;
                    }
                    if (mask & 8) != 0 && x < depth {
                        delta += (1.0 - x as f32 / depth as f32) * 55.0;
                    }
                    if (mask & 4) != 0 && h - 1 - y < depth {
                        delta -= (1.0 - (h - 1 - y) as f32 / depth as f32) * 70.0;
                    }
                    if (mask & 2) != 0 && w - 1 - x < depth {
                        delta -= (1.0 - (w - 1 - x) as f32 / depth as f32) * 55.0;
                    }
                    if delta.abs() > 0.1 {
                        for c in 0..3 {
                            out[off + c] =
                                (out[off + c] as f32 + delta).round().clamp(0.0, 255.0) as u8;
                        }
                    }
                }
                _ => {
                    // Torn / Wave / Sawtooth / Sharktooth
                    let cut = ((mask & 1) != 0 && (y as f32) < edge_cut(x, 1))
                        || ((mask & 2) != 0 && ((w - 1 - x) as f32) < edge_cut(y, 2))
                        || ((mask & 4) != 0 && ((h - 1 - y) as f32) < edge_cut(x, 4))
                        || ((mask & 8) != 0 && (x as f32) < edge_cut(y, 8));
                    if cut {
                        out[off..off + 4].copy_from_slice(&[0, 0, 0, 0]);
                    }
                }
            }
        }
    }
    (w, h, out)
}

/// 8. Page Curl: 3D corner page curl on `TopLeft`, `TopRight`, `BottomLeft`, `BottomRight`.
pub fn apply_page_curl(
    w: u32,
    h: u32,
    rgba: &[u8],
    corner: &str,
    radius: f32,
    bg_color: ColorRgba,
) -> (u32, u32, Vec<u8>) {
    let mut out = rgba.to_vec();
    let max_fold = ((w.min(h) as f32) * 0.75).max(1.0);
    let fold = radius.clamp(1.0, max_fold);
    let bg = [bg_color.r, bg_color.g, bg_color.b, bg_color.a];

    for y in 0..h {
        for x in 0..w {
            // Map coordinates so the curled corner is always at (0, 0) in local space
            let (lx, ly) = match corner {
                "TopLeft" => (x as f32, y as f32),
                "TopRight" => ((w - 1 - x) as f32, y as f32),
                "BottomLeft" => (x as f32, (h - 1 - y) as f32),
                _ => ((w - 1 - x) as f32, (h - 1 - y) as f32), // BottomRight
            };
            let d = lx + ly;
            let off = ((y as usize) * (w as usize) + x as usize) * 4;
            if d < fold {
                // Behind the folded corner -> reveal background
                out[off..off + 4].copy_from_slice(&bg);
            } else if lx < fold && ly < fold {
                // Curled back-flap triangle (lx + ly >= fold AND lx < fold AND ly < fold)
                let t = ((d - fold) / fold).clamp(0.0, 1.0);
                // Shiny paper back gradient
                let shade = (245.0 - t * 65.0).round().clamp(140.0, 250.0) as u8;
                out[off] = shade;
                out[off + 1] = shade;
                out[off + 2] = shade;
                out[off + 3] = 255;
            } else if d < fold * 1.18 && lx < fold * 1.18 && ly < fold * 1.18 {
                // Soft shadow cast onto the page near the curl edge
                let s = 1.0 - 0.25 * (1.0 - ((d - fold) / (fold * 0.18)).clamp(0.0, 1.0));
                for c in 0..3 {
                    out[off + c] = (out[off + c] as f32 * s).round() as u8;
                }
            }
        }
    }
    (w, h, out)
}

/// 9. Perspective / Shear: 3D tilt projection + optional floor reflection fade.
pub fn apply_perspective(
    w: u32,
    h: u32,
    rgba: &[u8],
    angle_x: f32,
    angle_y: f32,
    reflection: bool,
    reflection_opacity: f32,
) -> (u32, u32, Vec<u8>) {
    let tilt_x = (angle_x.clamp(-45.0, 45.0) / 45.0) * 0.28;
    let tilt_y = (angle_y.clamp(-45.0, 45.0) / 45.0) * 0.28;
    let refl_h = if reflection { (h as f32 * 0.35).round() as u32 } else { 0 };
    let nh = h + refl_h;
    let mut out = vec![0u8; (w as usize) * (nh as usize) * 4];

    for y in 0..h {
        let v = if h > 1 { y as f32 / (h - 1) as f32 } else { 0.5 };
        for x in 0..w {
            let u = if w > 1 { x as f32 / (w - 1) as f32 } else { 0.5 };
            // Trapezoidal vertical pinch on left/right edge for 3D perspective tilt
            let pinch = 1.0 + tilt_x * (u - 0.5) * 2.0;
            let sv = 0.5 + (v - 0.5) / pinch.max(0.25);
            let pinch_h = 1.0 + tilt_y * (v - 0.5) * 2.0;
            let su = 0.5 + (u - 0.5) / pinch_h.max(0.25);

            if su >= 0.0 && su <= 1.0 && sv >= 0.0 && sv <= 1.0 {
                let px = sample_bilinear(rgba, w, h, su * (w - 1) as f32, sv * (h - 1) as f32);
                let off = ((y as usize) * (w as usize) + x as usize) * 4;
                out[off..off + 4].copy_from_slice(&px);
            }
        }
    }

    if reflection && refl_h > 0 {
        let op = reflection_opacity.clamp(0.0, 1.0);
        for ry in 0..refl_h {
            let src_y = h.saturating_sub(1 + ry.min(h - 1));
            let fade = (1.0 - (ry as f32 / refl_h as f32)) * op;
            for x in 0..w {
                let s_off = ((src_y as usize) * (w as usize) + x as usize) * 4;
                let d_off = (((h + ry) as usize) * (w as usize) + x as usize) * 4;
                out[d_off] = out[s_off];
                out[d_off + 1] = out[s_off + 1];
                out[d_off + 2] = out[s_off + 2];
                out[d_off + 3] = (out[s_off + 3] as f32 * fade).round() as u8;
            }
        }
    }
    (w, nh, out)
}

/// 10. Watermark: 9-grid anchor overlay with scale, opacity, and optional emboss.
#[allow(clippy::too_many_arguments)]
pub fn apply_watermark(
    w: u32,
    h: u32,
    rgba: &[u8],
    wm_rgba: &[u8],
    wm_w: u32,
    wm_h: u32,
    anchor: &str,
    scale: f32,
    opacity: f32,
    emboss: bool,
) -> (u32, u32, Vec<u8>) {
    if wm_w == 0 || wm_h == 0 || opacity <= 0.0 {
        return (w, h, rgba.to_vec());
    }
    let s = scale.clamp(0.05, 4.0);
    let dw = ((wm_w as f32 * s).round() as u32).max(1).min(w);
    let dh = ((wm_h as f32 * s).round() as u32).max(1).min(h);
    let mut scaled = downscale_rgba_bilinear(wm_rgba, wm_w, wm_h, dw, dh);

    if emboss && dw >= 3 && dh >= 3 {
        let src = scaled.clone();
        for y in 1..dh - 1 {
            for x in 1..dw - 1 {
                let tl = (((y - 1) * dw + (x - 1)) as usize) * 4;
                let br = (((y + 1) * dw + (x + 1)) as usize) * 4;
                let off = ((y * dw + x) as usize) * 4;
                for c in 0..3 {
                    let v = 128 + src[br + c] as i32 - src[tl + c] as i32;
                    scaled[off + c] = v.clamp(0, 255) as u8;
                }
            }
        }
    }

    let pad = 12u32.min(w / 10).min(h / 10);
    let (ox, oy) = match anchor {
        "TopLeft" => (pad, pad),
        "TopCenter" => ((w - dw) / 2, pad),
        "TopRight" => (w.saturating_sub(dw + pad), pad),
        "MiddleLeft" => (pad, (h - dh) / 2),
        "Center" => ((w - dw) / 2, (h - dh) / 2),
        "MiddleRight" => (w.saturating_sub(dw + pad), (h - dh) / 2),
        "BottomLeft" => (pad, h.saturating_sub(dh + pad)),
        "BottomCenter" => ((w - dw) / 2, h.saturating_sub(dh + pad)),
        _ => (w.saturating_sub(dw + pad), h.saturating_sub(dh + pad)), // BottomRight
    };

    let mut out = rgba.to_vec();
    let op = opacity.clamp(0.0, 1.0);
    for y in 0..dh {
        for x in 0..dw {
            let s_off = ((y * dw + x) as usize) * 4;
            let d_off = (((oy + y) * w + (ox + x)) as usize) * 4;
            let mut src_px = [
                scaled[s_off],
                scaled[s_off + 1],
                scaled[s_off + 2],
                (scaled[s_off + 3] as f32 * op).round() as u8,
            ];
            alpha_over(&mut out[d_off..d_off + 4], &src_px);
            let _ = &mut src_px;
        }
    }
    (w, h, out)
}

/// 11. Caption: outside top or bottom band with rendered text.
#[allow(clippy::too_many_arguments)]
pub fn apply_caption(
    w: u32,
    h: u32,
    rgba: &[u8],
    text: &str,
    position: &str,
    font_family: &str,
    font_size: f32,
    text_color: ColorRgba,
    bg_color: ColorRgba,
) -> (u32, u32, Vec<u8>) {
    let band_h = ((font_size.max(12.0) * 1.9).round() as u32).max(24);
    let nh = h + band_h;
    let is_top = position.eq_ignore_ascii_case("Top");

    let mut out = vec![0u8; (w as usize) * (nh as usize) * 4];
    let bg = [bg_color.r, bg_color.g, bg_color.b, bg_color.a];
    for chunk in out.chunks_exact_mut(4) {
        chunk.copy_from_slice(&bg);
    }

    let img_y0 = if is_top { band_h } else { 0 };
    for y in 0..h {
        let s_off = ((y as usize) * (w as usize)) * 4;
        let d_off = (((img_y0 + y) as usize) * (w as usize)) * 4;
        out[d_off..d_off + (w as usize) * 4]
            .copy_from_slice(&rgba[s_off..s_off + (w as usize) * 4]);
    }

    if !text.is_empty() {
        if let Some(mut pm) = Pixmap::new(w, band_h) {
            let opts = TextRenderOptions {
                font_family: font_family.to_string(),
                font_size: font_size.max(10.0),
                color: text_color,
                h_align: HorizontalAlign::Center,
                v_align: VerticalAlign::Center,
                ..TextRenderOptions::default()
            };
            render_text_box(
                &mut pm,
                text,
                4.0,
                0.0,
                (w as f32 - 8.0).max(1.0),
                band_h as f32,
                &opts,
                Transform::identity(),
            );
            let band_y0 = if is_top { 0 } else { h };
            let pm_data = pm.data();
            for y in 0..band_h {
                for x in 0..w {
                    let s_off = ((y * w + x) as usize) * 4;
                    let d_off = (((band_y0 + y) * w + x) as usize) * 4;
                    let sa = pm_data[s_off + 3];
                    if sa > 0 {
                        // Unpremultiply pm pixel for alpha_over
                        let af = sa as f32 / 255.0;
                        let straight = [
                            (pm_data[s_off] as f32 / af).round().clamp(0.0, 255.0) as u8,
                            (pm_data[s_off + 1] as f32 / af).round().clamp(0.0, 255.0) as u8,
                            (pm_data[s_off + 2] as f32 / af).round().clamp(0.0, 255.0) as u8,
                            sa,
                        ];
                        alpha_over(&mut out[d_off..d_off + 4], &straight);
                    }
                }
            }
        }
    }
    (w, nh, out)
}

/// 12. Trim: auto-crop uniform outer border within `tolerance`.
pub fn apply_trim(w: u32, h: u32, rgba: &[u8], tolerance: u8) -> (u32, u32, Vec<u8>) {
    let Ok(bmp) = TiledBitmap::from_rgba(w, h, rgba) else {
        return (w, h, rgba.to_vec());
    };
    let Some(rect) = auto_trim_bounds(&bmp, tolerance) else {
        return (w, h, rgba.to_vec());
    };
    let cropped = bmp.crop(rect);
    (cropped.width(), cropped.height(), cropped.to_rgba())
}

/// 13. Rotate: exact 90°/180°/270° fast paths + arbitrary-angle bilinear rotation.
pub fn apply_rotate(
    w: u32,
    h: u32,
    rgba: &[u8],
    angle_degrees: f32,
    bg_color: ColorRgba,
) -> (u32, u32, Vec<u8>) {
    let norm = angle_degrees.rem_euclid(360.0);
    if norm.abs() < 1e-3 || (norm - 360.0).abs() < 1e-3 {
        return (w, h, rgba.to_vec());
    }
    if (norm - 90.0).abs() < 1e-3 {
        let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
        for y in 0..h {
            for x in 0..w {
                let s = ((y * w + x) as usize) * 4;
                let d = ((x * h + (h - 1 - y)) as usize) * 4;
                out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
            }
        }
        return (h, w, out);
    }
    if (norm - 180.0).abs() < 1e-3 {
        let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
        for y in 0..h {
            for x in 0..w {
                let s = ((y * w + x) as usize) * 4;
                let d = (((h - 1 - y) * w + (w - 1 - x)) as usize) * 4;
                out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
            }
        }
        return (w, h, out);
    }
    if (norm - 270.0).abs() < 1e-3 {
        let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
        for y in 0..h {
            for x in 0..w {
                let s = ((y * w + x) as usize) * 4;
                let d = (((w - 1 - x) * h + y) as usize) * 4;
                out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
            }
        }
        return (h, w, out);
    }

    let rad = norm.to_radians();
    let cos = rad.cos();
    let sin = rad.sin();
    let nw = ((w as f32 * cos.abs() + h as f32 * sin.abs()).ceil() as u32).max(1);
    let nh = ((w as f32 * sin.abs() + h as f32 * cos.abs()).ceil() as u32).max(1);
    let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
    let bg = [bg_color.r, bg_color.g, bg_color.b, bg_color.a];

    let cx = (w as f32 - 1.0) * 0.5;
    let cy = (h as f32 - 1.0) * 0.5;
    let ncx = (nw as f32 - 1.0) * 0.5;
    let ncy = (nh as f32 - 1.0) * 0.5;

    for y in 0..nh {
        for x in 0..nw {
            let dx = x as f32 - ncx;
            let dy = y as f32 - ncy;
            let sx = dx * cos + dy * sin + cx;
            let sy = -dx * sin + dy * cos + cy;
            let off = ((y * nw + x) as usize) * 4;
            if sx >= 0.0 && sx <= (w - 1) as f32 && sy >= 0.0 && sy <= (h - 1) as f32 {
                let px = sample_bilinear(rgba, w, h, sx, sy);
                out[off..off + 4].copy_from_slice(&px);
            } else {
                out[off..off + 4].copy_from_slice(&bg);
            }
        }
    }
    (nw, nh, out)
}

/// 14. Flip: horizontal and/or vertical mirror.
pub fn apply_flip(
    w: u32,
    h: u32,
    rgba: &[u8],
    horizontal: bool,
    vertical: bool,
) -> (u32, u32, Vec<u8>) {
    if !horizontal && !vertical {
        return (w, h, rgba.to_vec());
    }
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h {
        let sy = if vertical { h - 1 - y } else { y };
        for x in 0..w {
            let sx = if horizontal { w - 1 - x } else { x };
            let s = ((sy * w + sx) as usize) * 4;
            let d = ((y * w + x) as usize) * 4;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    (w, h, out)
}

/// 15. Scale Image: bilinear resize with optional aspect ratio preservation.
pub fn apply_scale(
    w: u32,
    h: u32,
    rgba: &[u8],
    target_w: u32,
    target_h: u32,
    keep_aspect_ratio: bool,
) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = if keep_aspect_ratio && target_w > 0 && target_h > 0 {
        let s = (target_w as f32 / w as f32).min(target_h as f32 / h as f32);
        (
            ((w as f32 * s).round() as u32).max(1),
            ((h as f32 * s).round() as u32).max(1),
        )
    } else {
        (target_w.max(1), target_h.max(1))
    };
    let scaled = downscale_rgba_bilinear(rgba, w, h, nw, nh);
    (nw, nh, scaled)
}

#[inline]
fn sample_bilinear(src: &[u8], w: u32, h: u32, sx: f32, sy: f32) -> [u8; 4] {
    let x0 = (sx.floor() as u32).min(w - 1);
    let y0 = (sy.floor() as u32).min(h - 1);
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let wx = sx - x0 as f32;
    let wy = sy - y0 as f32;

    let i00 = ((y0 * w + x0) as usize) * 4;
    let i10 = ((y0 * w + x1) as usize) * 4;
    let i01 = ((y1 * w + x0) as usize) * 4;
    let i11 = ((y1 * w + x1) as usize) * 4;

    let mut out = [0u8; 4];
    for c in 0..4 {
        let top = src[i00 + c] as f32 * (1.0 - wx) + src[i10 + c] as f32 * wx;
        let bot = src[i01 + c] as f32 * (1.0 - wx) + src[i11 + c] as f32 * wx;
        out[c] = (top * (1.0 - wy) + bot * wy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

#[inline]
fn alpha_over(dst: &mut [u8], src: &[u8]) {
    let sa = src[3] as f32 / 255.0;
    if sa <= 0.0 {
        return;
    }
    if sa >= 1.0 {
        dst.copy_from_slice(src);
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 1e-6 {
        dst.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    for c in 0..3 {
        let val = (src[c] as f32 * sa + dst[c] as f32 * da * (1.0 - sa)) / out_a;
        dst[c] = val.round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}
