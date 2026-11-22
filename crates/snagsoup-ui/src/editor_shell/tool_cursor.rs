//! Native Win32 hardware cursor (`HCURSOR`) generator & cache for Editor tools.
//!
//! Renders **Option A (Precision Crosshair `+` + Selected Tool Icon Badge)**:
//! - Top-left hotspot `(hx, hy)` marked by a high-contrast 4-arm precision crosshair
//!   with a 1-pixel open center sight so exact pixel colors remain visible underneath.
//! - Bottom-right tool icon rasterized from SnagSoup's embedded vector SVGs via `resvg`
//!   with a white sticker halo + dark `#111827` ink so it stays legible over both light
//!   and dark screenshots at any Windows DPI scale (`100%`–`300%`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use snagsoup_canvas::render_svg_to_rgba;
use snagsoup_model::ToolMode;
use snagsoup_win_platform::cursor::create_argb_hcursor;

use crate::icons::SnagIcon;

/// Rasterized cursor bitmap + hotspot (unit-testable independently of Win32 `HCURSOR`).
#[derive(Debug, Clone)]
pub struct ToolCursorBitmap {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub rgba: Vec<u8>,
}

/// Canonical cache key string for each tool that uses a custom icon cursor on the Editor canvas.
/// Returns `None` for `ToolMode::Move`, where Windows' native 4-way `IDC_SIZEALL` cursor applies.
pub fn tool_cursor_svg(mode: &ToolMode) -> Option<(&'static str, &'static str)> {
    match mode {
        ToolMode::Move => None,
        ToolMode::Selection => Some(("selection", SnagIcon::Selection.svg_source())),
        ToolMode::Arrow => Some(("arrow", SnagIcon::Arrow.svg_source())),
        ToolMode::Line => Some((
            "line",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M5 19L19 5"/><circle cx="5" cy="19" r="2" fill="currentColor"/><circle cx="19" cy="5" r="2" fill="currentColor"/></svg>"#,
        )),
        ToolMode::Text => Some(("text", SnagIcon::Text.svg_source())),
        ToolMode::Callout => Some(("callout", SnagIcon::Callout.svg_source())),
        ToolMode::Shape => Some(("shape", SnagIcon::Shape.svg_source())),
        ToolMode::Stamp => Some(("stamp", SnagIcon::Stamp.svg_source())),
        ToolMode::Fill => Some(("fill", SnagIcon::Fill.svg_source())),
        ToolMode::Blur => Some(("blur", SnagIcon::Blur.svg_source())),
        ToolMode::Crop => Some(("crop", SnagIcon::Crop.svg_source())),
        ToolMode::Step => Some(("step", SnagIcon::Step.svg_source())),
        ToolMode::Pen => Some(("pen", SnagIcon::Pen.svg_source())),
        ToolMode::Highlighter => Some(("highlighter", SnagIcon::Highlighter.svg_source())),
        ToolMode::Eraser => Some(("eraser", SnagIcon::Eraser.svg_source())),
        ToolMode::Magnify => Some(("magnify", SnagIcon::Magnify.svg_source())),
        ToolMode::CutOut => Some((
            "cutout",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 8h16M4 16h16"/><path d="M9 4v4M15 4v4M9 16v4M15 16v4"/></svg>"#,
        )),
        ToolMode::Simplify => Some(("simplify", SnagIcon::Effects.svg_source())),
        ToolMode::SmartMove => Some(("smart_move", SnagIcon::Move.svg_source())),
        ToolMode::MagicWand => Some((
            "magic_wand",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 4V2M15 16v-2M8 9h2M20 9h2M17.8 11.8L19 13M17.8 6.2L19 5M3 21l9-9M12.2 6.2L11 5"/></svg>"#,
        )),
        ToolMode::Eyedropper => Some(("eyedropper", SnagIcon::Pipette.svg_source())),
        ToolMode::Image | ToolMode::Cursor | ToolMode::DropZone | ToolMode::Unknown(_) => None,
    }
}

fn blend_pixel_over(dst: &mut [u8; 4], src: [u8; 4]) {
    let sa = src[3] as u32;
    if sa == 0 {
        return;
    }
    if sa == 255 || dst[3] == 0 {
        *dst = src;
        return;
    }
    let da = dst[3] as u32;
    let inv_sa = 255 - sa;
    let out_a = sa + (da * inv_sa + 127) / 255;
    if out_a == 0 {
        *dst = [0, 0, 0, 0];
        return;
    }
    for c in 0..3 {
        let num = (src[c] as u32) * sa * 255 + (dst[c] as u32) * da * inv_sa;
        dst[c] = ((num + (out_a * 255) / 2) / (out_a * 255)).min(255) as u8;
    }
    dst[3] = out_a.min(255) as u8;
}

/// Build the DPI-scaled `RGBA8` cursor image (`Precision Crosshair + Tool Icon Badge`) for `mode`.
pub fn build_tool_cursor_bitmap(mode: &ToolMode, scale_factor: f32) -> Option<ToolCursorBitmap> {
    let (_key, raw_svg) = tool_cursor_svg(mode)?;
    let scale_pct = (scale_factor.clamp(1.0, 3.0) * 100.0).round() as u32;

    let canvas_sz = ((32 * scale_pct + 50) / 100).max(32);
    let hx = ((6 * scale_pct + 50) / 100).max(6);
    let hy = hx;
    let icon_x = ((11 * scale_pct + 50) / 100).max(11);
    let icon_y = icon_x;
    let icon_sz = ((19 * scale_pct + 50) / 100)
        .max(18)
        .min(canvas_sz.saturating_sub(icon_x + 2));

    let mut rgba = vec![0u8; (canvas_sz as usize) * (canvas_sz as usize) * 4];

    // 1. Render tool icon SVG in dark charcoal ink (`#111827`) with slightly boosted stroke weight
    let styled_svg = raw_svg
        .replace("stroke-width=\"2\"", "stroke-width=\"2.35\"")
        .replace("currentColor", "#111827");
    let icon_rgba = render_svg_to_rgba(styled_svg.as_bytes(), icon_sz, icon_sz)?;

    // 2. Dilate icon alpha into a crisp solid white sticker halo (`~1.5px` solid + `0.85px` AA feather)
    let solid_r = (scale_pct as f32 / 100.0) * 1.48;
    let outer_r = solid_r + 0.85;
    let halo_r = outer_r.ceil().max(2.0) as i32;
    let outer_r_sq = outer_r * outer_r;
    let isz = icon_sz as i32;
    let csz = canvas_sz as i32;

    for oy in -halo_r..isz + halo_r {
        let dy_canvas = icon_y as i32 + oy;
        if dy_canvas < 0 || dy_canvas >= csz {
            continue;
        }
        for ox in -halo_r..isz + halo_r {
            let dx_canvas = icon_x as i32 + ox;
            if dx_canvas < 0 || dx_canvas >= csz {
                continue;
            }
            let mut max_halo: f32 = 0.0;
            for ky in -halo_r..=halo_r {
                let sy = oy + ky;
                if sy < 0 || sy >= isz {
                    continue;
                }
                for kx in -halo_r..=halo_r {
                    let sx = ox + kx;
                    if sx < 0 || sx >= isz {
                        continue;
                    }
                    let d2 = (kx * kx + ky * ky) as f32;
                    if d2 > outer_r_sq {
                        continue;
                    }
                    let a = icon_rgba[((sy as u32 * icon_sz + sx as u32) as usize) * 4 + 3];
                    if a > 0 {
                        let dist = d2.sqrt();
                        let falloff = if dist <= solid_r {
                            1.0
                        } else {
                            ((outer_r - dist) / (outer_r - solid_r)).clamp(0.0, 1.0)
                        };
                        max_halo = max_halo.max((a as f32 / 255.0).min(1.0) * falloff);
                    }
                }
            }
            if max_halo > 0.02 {
                let ha = (max_halo * 252.0).round().clamp(0.0, 252.0) as u8;
                let idx = ((dy_canvas as u32 * canvas_sz + dx_canvas as u32) as usize) * 4;
                let slot: &mut [u8; 4] = (&mut rgba[idx..idx + 4]).try_into().expect("4 bytes");
                blend_pixel_over(slot, [255, 255, 255, ha]);
            }
        }
    }

    // Composite crisp dark icon strokes over the white halo
    for iy in 0..icon_sz {
        for ix in 0..icon_sz {
            let s_off = ((iy * icon_sz + ix) as usize) * 4;
            let src = [
                icon_rgba[s_off],
                icon_rgba[s_off + 1],
                icon_rgba[s_off + 2],
                icon_rgba[s_off + 3],
            ];
            if src[3] > 0 {
                let d_off = (((icon_y + iy) * canvas_sz + (icon_x + ix)) as usize) * 4;
                let slot: &mut [u8; 4] = (&mut rgba[d_off..d_off + 4]).try_into().expect("4 bytes");
                blend_pixel_over(slot, src);
            }
        }
    }

    // 3. Draw Precision Crosshair `+` centered at `(hx, hy)` ON TOP so aiming is always razor-sharp
    let arm = ((5 * scale_pct + 50) / 100).max(5) as i32;
    let half_t = if scale_pct >= 175 { 1i32 } else { 0i32 };
    let hxi = hx as i32;
    let hyi = hy as i32;

    let is_crosshair_core = |px: i32, py: i32| -> bool {
        let dx = (px - hxi).abs();
        let dy = (py - hyi).abs();
        // Leave exact 1px center sight `(0, 0)` open so target pixel color remains visible!
        if dx == 0 && dy == 0 {
            return false;
        }
        (dy <= half_t && dx >= 1 && dx <= arm) || (dx <= half_t && dy >= 1 && dy <= arm)
    };

    // White crosshair halo (1px border around arms)
    for py in (hyi - arm - 1).max(0)..=(hyi + arm + 1).min(csz - 1) {
        for px in (hxi - arm - 1).max(0)..=(hxi + arm + 1).min(csz - 1) {
            if is_crosshair_core(px, py) || (px == hxi && py == hyi) {
                continue;
            }
            let mut near_core = false;
            'nbr: for ny in -1..=1 {
                for nx in -1..=1 {
                    if is_crosshair_core(px + nx, py + ny) {
                        near_core = true;
                        break 'nbr;
                    }
                }
            }
            if near_core {
                let idx = ((py as u32 * canvas_sz + px as u32) as usize) * 4;
                let slot: &mut [u8; 4] = (&mut rgba[idx..idx + 4]).try_into().expect("4 bytes");
                blend_pixel_over(slot, [255, 255, 255, 245]);
            }
        }
    }

    // Dark charcoal crosshair core
    for py in (hyi - arm).max(0)..=(hyi + arm).min(csz - 1) {
        for px in (hxi - arm).max(0)..=(hxi + arm).min(csz - 1) {
            if is_crosshair_core(px, py) {
                let idx = ((py as u32 * canvas_sz + px as u32) as usize) * 4;
                rgba[idx..idx + 4].copy_from_slice(&[17, 24, 39, 255]);
            }
        }
    }

    Some(ToolCursorBitmap {
        width: canvas_sz,
        height: canvas_sz,
        hotspot_x: hx,
        hotspot_y: hy,
        rgba,
    })
}

/// Obtain a cached Win32 `HCURSOR` (as `isize`) for `(mode, scale_factor)`.
pub fn cached_tool_hcursor(mode: &ToolMode, scale_factor: f32) -> Option<isize> {
    let (key, _) = tool_cursor_svg(mode)?;
    // Quantize scale factor to 25% steps (`100`, `125`, `150`, `175`, `200`, ...)
    let scale_bucket = (((scale_factor.clamp(1.0, 3.0) * 4.0).round() as u32) * 25).max(100);

    static CACHE: OnceLock<Mutex<HashMap<(&'static str, u32), isize>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(map) = cache.lock() {
        if let Some(&hcur) = map.get(&(key, scale_bucket)) {
            return (hcur != 0).then_some(hcur);
        }
    }

    let bmp = build_tool_cursor_bitmap(mode, scale_bucket as f32 / 100.0)?;
    let hcur = create_argb_hcursor(
        bmp.width,
        bmp.height,
        bmp.hotspot_x,
        bmp.hotspot_y,
        &bmp.rgba,
    )
    .unwrap_or(0);

    if let Ok(mut map) = cache.lock() {
        map.insert((key, scale_bucket), hcur);
    }
    (hcur != 0).then_some(hcur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_shell::shell::default_all_21_tools;

    #[test]
    fn all_editor_drawing_tools_generate_distinct_crosshair_badge_cursors() {
        let tools = default_all_21_tools();
        assert_eq!(tools.len(), 21);

        let mut generated = Vec::new();
        for entry in &tools {
            if entry.mode == ToolMode::Move {
                assert!(
                    build_tool_cursor_bitmap(&entry.mode, 1.0).is_none(),
                    "Move uses native 4-way IDC_SIZEALL cursor"
                );
                continue;
            }
            let bmp = build_tool_cursor_bitmap(&entry.mode, 1.0)
                .unwrap_or_else(|| panic!("expected custom cursor bitmap for {:?}", entry.mode));
            assert_eq!((bmp.width, bmp.height), (32, 32));
            assert_eq!((bmp.hotspot_x, bmp.hotspot_y), (6, 6));

            // Center sight `(6, 6)` stays open (`alpha == 0`) while its 4 crosshair neighbors are dark `#111827`
            let center_off = ((bmp.hotspot_y * bmp.width + bmp.hotspot_x) as usize) * 4;
            assert_eq!(bmp.rgba[center_off + 3], 0, "1px center sight must stay transparent");
            for (dx, dy) in [(-2i32, 0i32), (2, 0), (0, -2), (0, 2)] {
                let x = (bmp.hotspot_x as i32 + dx) as u32;
                let y = (bmp.hotspot_y as i32 + dy) as u32;
                let off = ((y * bmp.width + x) as usize) * 4;
                assert_eq!(&bmp.rgba[off..off + 4], &[17, 24, 39, 255]);
            }

            // Icon badge region `(11..30, 11..30)` must contain both dark ink and white halo pixels
            let mut dark_ink_px = 0usize;
            let mut white_halo_px = 0usize;
            for y in 11..31u32 {
                for x in 11..31u32 {
                    let off = ((y * bmp.width + x) as usize) * 4;
                    let [r, g, b, a] = [
                        bmp.rgba[off],
                        bmp.rgba[off + 1],
                        bmp.rgba[off + 2],
                        bmp.rgba[off + 3],
                    ];
                    if a > 200 && r < 40 && g < 40 && b < 50 {
                        dark_ink_px += 1;
                    } else if a > 180 && r > 240 && g > 240 && b > 240 {
                        white_halo_px += 1;
                    }
                }
            }
            assert!(
                dark_ink_px >= 12,
                "tool {:?} badge should have visible dark icon strokes (got {dark_ink_px})",
                entry.mode
            );
            assert!(
                white_halo_px >= 20,
                "tool {:?} badge should have visible white contrast halo (got {white_halo_px})",
                entry.mode
            );

            // Verify Win32 HCURSOR creation & caching succeeds
            let h1 = cached_tool_hcursor(&entry.mode, 1.25)
                .unwrap_or_else(|| panic!("expected valid HCURSOR for {:?}", entry.mode));
            let h2 = cached_tool_hcursor(&entry.mode, 1.25).unwrap();
            assert_eq!(h1, h2, "HCURSOR must be cached and reused across frames");

            generated.push((entry.label, bmp.rgba));
        }

        assert_eq!(generated.len(), 20);
        // Spot-check that Step, Arrow, Line, Shape, Text, Eraser, and Eyedropper bitmaps are all distinct
        let find_rgba = |lbl: &str| -> &[u8] {
            &generated.iter().find(|(l, _)| *l == lbl).unwrap().1
        };
        assert_ne!(find_rgba("Step"), find_rgba("Arrow"));
        assert_ne!(find_rgba("Arrow"), find_rgba("Line"));
        assert_ne!(find_rgba("Shape"), find_rgba("Text"));
        assert_ne!(find_rgba("Eraser"), find_rgba("Eyedropper"));
    }
}
