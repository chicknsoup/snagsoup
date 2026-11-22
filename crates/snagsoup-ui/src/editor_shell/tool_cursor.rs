//! Native Win32 32-bpp `BI_RGB` hardware cursor (`HCURSOR`) generator & cache for Editor tools.
//!
//! - Uses standard Windows OS cursors for `Move` (`IDC_SIZEALL`), `Selection` (`IDC_CROSS`),
//!   `Text` (`IDC_IBEAM`), and `Callout` (`IDC_CROSS` / `IDC_IBEAM`).
//! - Renders the remaining 17 Editor tools at the exact OS `SM_CXCURSOR × SM_CYCURSOR`
//!   hardware dimension (`32×32` at 100%, `40×40` at 125%, `48×48` at 150%, `64×64` at 200%)
//!   using `4×` supersampled two-pass vector paths (`#121426` dark Aero outer border + crisp
//!   `#FFFFFF` inner stroke — matching Windows 11's native cursor aesthetic with zero pixel
//!   stretching and zero patchy halo speckles).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use snagsoup_canvas::render_dual_layer_cursor_svg_rgba;
use snagsoup_model::ToolMode;
use snagsoup_win_platform::cursor::{create_argb_hcursor, query_system_cursor_size_for_scale};

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
///
/// Returns `None` for tools that use standard Windows OS cursors:
/// - `Move` (`IDC_SIZEALL`)
/// - `Selection` (`IDC_CROSS`)
/// - `Text` (`IDC_IBEAM`)
/// - `Callout` (`IDC_CROSS` while placing, `IDC_IBEAM` while typing)
pub fn tool_cursor_spec(mode: &ToolMode) -> Option<(&'static str, &'static str, (f32, f32))> {
    match mode {
        ToolMode::Move | ToolMode::Selection | ToolMode::Text | ToolMode::Callout => None,

        // Tip-pointed tools (hotspot right on the active nib / nozzle / wand / drop / eraser edge)
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

        // Lens / center-aimed tools
        ToolMode::Magnify => Some(("magnify", SnagIcon::Magnify.svg_source(), (11.0, 11.0))),
        ToolMode::Arrow => Some(("arrow", SnagIcon::Arrow.svg_source(), (12.0, 12.0))),
        ToolMode::Line => Some((
            "line",
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 19L19 5"/><circle cx="5" cy="19" r="1.8" fill="currentColor"/><circle cx="19" cy="5" r="1.8" fill="currentColor"/></svg>"#,
            (12.0, 12.0),
        )),
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

/// Build the `4×`-supersampled pure vector cursor bitmap at exact OS `SM_CXCURSOR` size.
pub fn build_tool_cursor_bitmap(mode: &ToolMode, scale_factor: f32) -> Option<ToolCursorBitmap> {
    let (_key, raw_svg, (vx, vy)) = tool_cursor_spec(mode)?;
    let canvas_sz = query_system_cursor_size_for_scale(scale_factor);

    // Keep comfortable Windows 11 Aero cursor proportion (`22px` inside `32px`, `28px` inside `40px`)
    // with generous integer margin so outer strokes never clip against the cursor box boundary.
    let mut icon_px = ((canvas_sz as f32) * 0.6875).round() as u32;
    if (canvas_sz - icon_px) % 2 == 1 {
        icon_px = icon_px.saturating_sub(1);
    }
    let icon_sz = icon_px.max(18) as f32;
    let pad = ((canvas_sz - icon_px) / 2) as f32;

    let hx = (pad + (vx / 24.0) * icon_sz)
        .round()
        .clamp(0.0, (canvas_sz - 1) as f32) as u32;
    let hy = (pad + (vy / 24.0) * icon_sz)
        .round()
        .clamp(0.0, (canvas_sz - 1) as f32) as u32;

    // Pass 1 (Aero dark slate-navy outer border `#121426`) + Pass 2 (crisp white core `#FFFFFF`)
    let outer_svg = raw_svg
        .replace("stroke-width=\"2\"", "stroke-width=\"4.3\"")
        .replace("currentColor", "#121426");
    let inner_svg = raw_svg
        .replace("stroke-width=\"2\"", "stroke-width=\"1.85\"")
        .replace("currentColor", "#FFFFFF");

    let rgba = render_dual_layer_cursor_svg_rgba(
        outer_svg.as_bytes(),
        inner_svg.as_bytes(),
        canvas_sz,
        pad,
        icon_sz,
    )?;

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
    let cursor_sz = query_system_cursor_size_for_scale(scale_factor);

    static CACHE: OnceLock<Mutex<HashMap<(&'static str, u32), isize>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(map) = cache.lock() {
        if let Some(&hcur) = map.get(&(key, cursor_sz)) {
            return (hcur != 0).then_some(hcur);
        }
    }

    let bmp = build_tool_cursor_bitmap(mode, scale_factor)?;
    let hcur = create_argb_hcursor(
        bmp.width,
        bmp.height,
        bmp.hotspot_x,
        bmp.hotspot_y,
        &bmp.rgba,
    )
    .unwrap_or(0);

    if let Ok(mut map) = cache.lock() {
        map.insert((key, cursor_sz), hcur);
    }
    (hcur != 0).then_some(hcur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_shell::shell::default_all_21_tools;

    #[test]
    fn custom_icon_cursors_are_supersampled_at_os_size_and_standard_tools_excluded() {
        let tools = default_all_21_tools();
        assert_eq!(tools.len(), 21);

        let sys_sz_100 = query_system_cursor_size_for_scale(1.0);
        let mut generated = Vec::new();

        for entry in &tools {
            if matches!(
                entry.mode,
                ToolMode::Move | ToolMode::Selection | ToolMode::Text | ToolMode::Callout
            ) {
                assert!(
                    build_tool_cursor_bitmap(&entry.mode, 1.0).is_none(),
                    "{:?} must use standard OS cursor",
                    entry.mode
                );
                continue;
            }

            let bmp = build_tool_cursor_bitmap(&entry.mode, 1.0)
                .unwrap_or_else(|| panic!("expected custom cursor bitmap for {:?}", entry.mode));
            assert_eq!(
                (bmp.width, bmp.height),
                (sys_sz_100, sys_sz_100),
                "cursor bitmap must match exact OS SM_CXCURSOR size to prevent nearest-neighbor stretching"
            );
            assert!(bmp.hotspot_x < bmp.width && bmp.hotspot_y < bmp.height);

            // Outer border pixels (`#121426`), crisp white core pixels (`#FFFFFF`), and smooth AA pixels (`0 < a < 255`)
            let mut dark_border_px = 0usize;
            let mut white_core_px = 0usize;
            let mut smooth_aa_px = 0usize;
            for px in bmp.rgba.chunks_exact(4) {
                let [r, g, b, a] = [px[0], px[1], px[2], px[3]];
                if a > 0 && a < 255 {
                    smooth_aa_px += 1;
                }
                if a > 180 && r < 40 && g < 40 && b < 60 {
                    dark_border_px += 1;
                } else if a > 220 && r > 235 && g > 235 && b > 235 {
                    white_core_px += 1;
                }
            }
            assert!(
                dark_border_px >= 16,
                "tool {:?} should have crisp dark Aero border (got {dark_border_px})",
                entry.mode
            );
            assert!(
                white_core_px >= 10,
                "tool {:?} should have crisp white core strokes (got {white_core_px})",
                entry.mode
            );
            assert!(
                smooth_aa_px >= 20,
                "tool {:?} should have smooth 4x-supersampled AA edge coverage (got {smooth_aa_px})",
                entry.mode
            );

            let h1 = cached_tool_hcursor(&entry.mode, 1.25).unwrap();
            let h2 = cached_tool_hcursor(&entry.mode, 1.25).unwrap();
            assert_eq!(h1, h2);

            generated.push((entry.mode.clone(), bmp));
        }

        // 21 total tools - 4 standard OS cursor tools (`Move`, `Selection`, `Text`, `Callout`) = 17 custom icon cursors
        assert_eq!(generated.len(), 17);

        let get_bmp = |m: ToolMode| -> &ToolCursorBitmap {
            &generated.iter().find(|(mode, _)| *mode == m).unwrap().1
        };
        let pen = get_bmp(ToolMode::Pen);
        assert!(pen.hotspot_x < pen.width / 2 && pen.hotspot_y > pen.height / 2);
        let step = get_bmp(ToolMode::Step);
        assert_eq!((step.hotspot_x, step.hotspot_y), (sys_sz_100 / 2, sys_sz_100 / 2));
    }
}
