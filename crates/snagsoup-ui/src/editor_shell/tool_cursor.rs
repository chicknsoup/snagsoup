//! Native Win32 hardware cursor (`HCURSOR`) generator & cache for Editor tools.
//!
//! Renders each selected Editor tool's pure vector icon (`24×24` SVG inside a `28×28`
//! DPI-scaled frame with a crisp white contrast halo + dark `#111827` strokes so it stays
//! legible on both light and dark screenshots), with per-tool natural `(xHotspot, yHotspot)`
//! coordinates (nib/pipette/wand/drop tip for pointed tools; icon/lens center for shape &
//! stamp tools).

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

/// Tool cursor specification: `(cache_key, svg_markup, hotspot_viewbox_xy_in_0_to_24)`.
/// Returns `None` for `ToolMode::Move`, where Windows' native 4-way `IDC_SIZEALL` cursor applies.
pub fn tool_cursor_spec(mode: &ToolMode) -> Option<(&'static str, &'static str, (f32, f32))> {
    match mode {
        ToolMode::Move => None,
        // Tip-pointed tools (hotspot right on the active nib / nozzle / wand / drop / tail tip)
        ToolMode::Pen => Some(("pen", SnagIcon::Pen.svg_source(), (3.0, 20.0))),
        ToolMode::Highlighter => Some((
            "highlighter",
            SnagIcon::Highlighter.svg_source(),
            (3.0, 20.0),
        )),
        ToolMode::Eyedropper => Some(("eyedropper", SnagIcon::Pipette.svg_source(), (2.0, 22.0))),
        ToolMode::MagicWand => Some((
            "magic_wand",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8.5 8.5L21 21M6 2v2M6 8v2M2 6h2M8 6h2M3.2 3.2l1.2 1.2M8.8 3.2L7.6 4.4M3.2 8.8l1.2-1.2"/></svg>"#,
            (6.0, 6.0),
        )),
        ToolMode::Fill => Some(("fill", SnagIcon::Fill.svg_source(), (20.0, 19.5))),
        ToolMode::Eraser => Some(("eraser", SnagIcon::Eraser.svg_source(), (7.0, 19.0))),
        ToolMode::Callout => Some(("callout", SnagIcon::Callout.svg_source(), (3.0, 21.0))),

        // Lens / center-aimed tools
        ToolMode::Magnify => Some(("magnify", SnagIcon::Magnify.svg_source(), (11.0, 11.0))),
        ToolMode::Selection => Some(("selection", SnagIcon::Selection.svg_source(), (12.0, 12.0))),
        ToolMode::Arrow => Some(("arrow", SnagIcon::Arrow.svg_source(), (12.0, 12.0))),
        ToolMode::Line => Some((
            "line",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M5 19L19 5"/><circle cx="5" cy="19" r="2" fill="currentColor"/><circle cx="19" cy="5" r="2" fill="currentColor"/></svg>"#,
            (12.0, 12.0),
        )),
        ToolMode::Text => Some(("text", SnagIcon::Text.svg_source(), (12.0, 12.0))),
        ToolMode::Shape => Some(("shape", SnagIcon::Shape.svg_source(), (12.0, 12.0))),
        ToolMode::Stamp => Some(("stamp", SnagIcon::Stamp.svg_source(), (12.0, 12.0))),
        ToolMode::Blur => Some(("blur", SnagIcon::Blur.svg_source(), (12.0, 12.0))),
        ToolMode::Crop => Some(("crop", SnagIcon::Crop.svg_source(), (12.0, 12.0))),
        ToolMode::Step => Some(("step", SnagIcon::Step.svg_source(), (12.0, 12.0))),
        ToolMode::CutOut => Some((
            "cutout",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 8h16M4 16h16"/><path d="M9 4v4M15 4v4M9 16v4M15 16v4"/></svg>"#,
            (12.0, 12.0),
        )),
        ToolMode::Simplify => Some(("simplify", SnagIcon::Effects.svg_source(), (12.0, 12.0))),
        ToolMode::SmartMove => Some(("smart_move", SnagIcon::Move.svg_source(), (12.0, 12.0))),
        ToolMode::Image | ToolMode::Cursor | ToolMode::DropZone | ToolMode::Unknown(_) => None,
    }
}

/// Convenience helper returning `(cache_key, svg_source)` if `mode` has a custom tool icon cursor.
pub fn tool_cursor_svg(mode: &ToolMode) -> Option<(&'static str, &'static str)> {
    tool_cursor_spec(mode).map(|(k, svg, _)| (k, svg))
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

/// Build the DPI-scaled pure tool-icon `RGBA8` cursor bitmap + natural hotspot for `mode`.
pub fn build_tool_cursor_bitmap(mode: &ToolMode, scale_factor: f32) -> Option<ToolCursorBitmap> {
    let (_key, raw_svg, (vx, vy)) = tool_cursor_spec(mode)?;
    let scale_pct = (scale_factor.clamp(1.0, 3.0) * 100.0).round() as u32;

    let pad = ((2 * scale_pct + 50) / 100).max(2);
    let icon_sz = ((22 * scale_pct + 50) / 100).max(22);
    let canvas_sz = icon_sz + pad * 2;

    let hx = (pad as f32 + (vx / 24.0) * (icon_sz as f32))
        .round()
        .clamp(0.0, (canvas_sz - 1) as f32) as u32;
    let hy = (pad as f32 + (vy / 24.0) * (icon_sz as f32))
        .round()
        .clamp(0.0, (canvas_sz - 1) as f32) as u32;

    let mut rgba = vec![0u8; (canvas_sz as usize) * (canvas_sz as usize) * 4];

    // 1. Render pure tool icon SVG in dark charcoal ink (`#111827`)
    let styled_svg = raw_svg
        .replace("stroke-width=\"2\"", "stroke-width=\"2.2\"")
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
        let dy_canvas = pad as i32 + oy;
        if dy_canvas < 0 || dy_canvas >= csz {
            continue;
        }
        for ox in -halo_r..isz + halo_r {
            let dx_canvas = pad as i32 + ox;
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

    // 3. Composite crisp dark icon strokes over the white halo
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
                let d_off = (((pad + iy) * canvas_sz + (pad + ix)) as usize) * 4;
                let slot: &mut [u8; 4] = (&mut rgba[d_off..d_off + 4]).try_into().expect("4 bytes");
                blend_pixel_over(slot, src);
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
    let (key, _, _) = tool_cursor_spec(mode)?;
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
    fn all_editor_tools_generate_pure_icon_cursors_with_natural_hotspots() {
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
            assert_eq!((bmp.width, bmp.height), (26, 26));
            assert!(bmp.hotspot_x < bmp.width && bmp.hotspot_y < bmp.height);

            // Top-left corner `(0, 0)` must be completely transparent (no detached crosshair!)
            assert_eq!(&bmp.rgba[0..4], &[0, 0, 0, 0]);

            // Icon must contain both dark strokes and white contrast halo
            let mut dark_ink_px = 0usize;
            let mut white_halo_px = 0usize;
            for px in bmp.rgba.chunks_exact(4) {
                let [r, g, b, a] = [px[0], px[1], px[2], px[3]];
                if a > 200 && r < 40 && g < 40 && b < 50 {
                    dark_ink_px += 1;
                } else if a > 180 && r > 240 && g > 240 && b > 240 {
                    white_halo_px += 1;
                }
            }
            assert!(
                dark_ink_px >= 14,
                "tool {:?} icon cursor should have visible dark strokes (got {dark_ink_px})",
                entry.mode
            );
            assert!(
                white_halo_px >= 20,
                "tool {:?} icon cursor should have visible white halo (got {white_halo_px})",
                entry.mode
            );

            let h1 = cached_tool_hcursor(&entry.mode, 1.25)
                .unwrap_or_else(|| panic!("expected valid HCURSOR for {:?}", entry.mode));
            let h2 = cached_tool_hcursor(&entry.mode, 1.25).unwrap();
            assert_eq!(h1, h2);

            generated.push((entry.mode.clone(), bmp));
        }

        assert_eq!(generated.len(), 20);

        let get_bmp = |m: ToolMode| -> &ToolCursorBitmap {
            &generated.iter().find(|(mode, _)| *mode == m).unwrap().1
        };
        // Pointed tools have bottom-left / tip hotspots; centered tools have `(13, 13)` center hotspot
        let pen = get_bmp(ToolMode::Pen);
        assert!(pen.hotspot_x <= 6 && pen.hotspot_y >= 19, "Pen hotspot at nib tip");
        let pip = get_bmp(ToolMode::Eyedropper);
        assert!(pip.hotspot_x <= 5 && pip.hotspot_y >= 21, "Eyedropper hotspot at nozzle tip");
        let wand = get_bmp(ToolMode::MagicWand);
        assert!(wand.hotspot_x <= 9 && wand.hotspot_y <= 9, "Magic Wand hotspot at top-left star");
        let step = get_bmp(ToolMode::Step);
        assert_eq!((step.hotspot_x, step.hotspot_y), (13, 13));
    }
}
