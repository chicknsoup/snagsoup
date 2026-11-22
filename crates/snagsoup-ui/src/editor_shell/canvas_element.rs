//! High-performance GPUI Interactive Canvas (`W3-06`):
//! - Viewport culling & `DirtyRegion`-aware tile/viewport `RenderImage` caching
//!   (smooth 120fps pan/zoom even on `1920x20000` scrolling captures)
//! - Cursor-anchored `Ctrl + Scroll` Zoom (`10%–800%`), `Space + Drag` Pan
//! - Pixel grid overlay at `>= 400%` zoom
//! - 8 outer Canvas resize handles
//! - Pink Smart Alignment Guides snapping to canvas center & sibling object edges

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    IntoElement, ParentElement, RenderImage, Styled, div, img, prelude::FluentBuilder, px, rgb,
    rgba,
};
use snagsoup_canvas::{DocumentSession, SceneCompositor, TILE_SIZE, TileCoord};
use snagsoup_model::{PointF, RectF, ToolMode};

use crate::theme::SnagTheme;

/// Live interactive tool overlays painted on top of the canvas bitmap
/// (marquee drag box, persistent `Selection`, pending `Crop` veil+handles, and inline text focus).
#[derive(Debug, Clone, Default)]
pub struct CanvasInteractiveOverlays {
    /// Active drag rectangle for `Selection`, `Blur`, `Magnify`, `CutOut`, or `Crop`.
    pub active_region_drag: Option<(ToolMode, RectF)>,
    /// Persistent marquee rectangle after dragging with `ToolMode::Selection`.
    pub active_selection_rect: Option<RectF>,
    /// Pending crop rectangle awaiting user confirmation (`✓ Crop` button or `Enter`).
    pub pending_crop_rect: Option<RectF>,
    /// Active inline `Text` / `Callout` bounding box + live text with caret.
    pub editing_text_box: Option<(RectF, String)>,
}

/// Orientation of a pink Smart Alignment Guide line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlignmentGuide {
    Vertical { x: f64 },
    Horizontal { y: f64 },
}

/// 8 outer canvas resize handle positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasResizeHandle {
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
}

impl CanvasResizeHandle {
    pub const ALL: &'static [Self] = &[
        Self::NorthWest,
        Self::North,
        Self::NorthEast,
        Self::East,
        Self::SouthEast,
        Self::South,
        Self::SouthWest,
        Self::West,
    ];
}

/// Viewport transform, zoom/pan state, viewport culling, and smart alignment snapping.
#[derive(Debug, Clone)]
pub struct CanvasViewportState {
    /// Zoom scale factor (`0.10..=8.00`, i.e., `10%..=800%` where `100%` = 1:1 physical pixels).
    pub zoom: f32,
    /// Active monitor Per-Monitor DPI scale factor (`window.scale_factor()`, e.g. `1.0`, `1.25`, `1.5`, `2.0`).
    pub device_scale: f32,
    /// Pan offset in viewport logical screen pixels (canvas origin `(0, 0)` screen position).
    pub pan_x: f32,
    pub pan_y: f32,
    pub viewport_w: f32,
    pub viewport_h: f32,
    pub space_panning: bool,
    pub active_guides: Vec<AlignmentGuide>,
}

impl Default for CanvasViewportState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            device_scale: 1.0,
            pan_x: 32.0,
            pan_y: 32.0,
            viewport_w: 960.0,
            viewport_h: 640.0,
            space_panning: false,
            active_guides: Vec::new(),
        }
    }
}

impl CanvasViewportState {
    pub fn zoom_percent(&self) -> f32 {
        self.zoom * 100.0
    }

    /// Logical DIPs per 1 canvas pixel (`zoom / device_scale`), ensuring `100%` zoom maps
    /// 1 image pixel to 1 physical screen pixel with zero HiDPI upscaling blur.
    #[inline]
    pub fn effective_logical_zoom(&self) -> f32 {
        self.zoom.max(0.10) / self.device_scale.max(0.5)
    }

    /// Convert a screen-space logical point inside the viewport to canvas coordinates.
    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> PointF {
        let z = self.effective_logical_zoom() as f64;
        PointF::new(
            (screen_x - self.pan_x) as f64 / z,
            (screen_y - self.pan_y) as f64 / z,
        )
    }

    /// Convert a canvas coordinate to viewport logical screen-space coordinates.
    pub fn canvas_to_screen(&self, pt: PointF) -> (f32, f32) {
        let z = self.effective_logical_zoom();
        (pt.x as f32 * z + self.pan_x, pt.y as f32 * z + self.pan_y)
    }

    /// Cursor-anchored zoom (`Ctrl + Scroll`): keeps the canvas point under `(cursor_x, cursor_y)` fixed.
    pub fn zoom_anchored_at(&mut self, new_zoom: f32, cursor_x: f32, cursor_y: f32) {
        let clamped = new_zoom.clamp(0.10, 8.00);
        let anchor_canvas = self.screen_to_canvas(cursor_x, cursor_y);
        self.zoom = clamped;
        let eff = self.effective_logical_zoom();
        let dev = self.device_scale.max(0.5);
        let raw_px = cursor_x - (anchor_canvas.x as f32) * eff;
        let raw_py = cursor_y - (anchor_canvas.y as f32) * eff;
        self.pan_x = (raw_px * dev).round() / dev;
        self.pan_y = (raw_py * dev).round() / dev;
    }

    /// Pan viewport by screen pixel delta (`Space + Drag`).
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        let dev = self.device_scale.max(0.5);
        self.pan_x = ((self.pan_x + dx) * dev).round() / dev;
        self.pan_y = ((self.pan_y + dy) * dev).round() / dev;
    }

    /// Fit the canvas cleanly inside the viewport with margin.
    /// Caps initial auto-fit at `100%` (`1.0`) so captures that fit on screen stay at razor-sharp 1:1 physical pixels.
    pub fn fit_canvas(&mut self, canvas_w: u32, canvas_h: u32) {
        let cw = canvas_w.max(1) as f32;
        let ch = canvas_h.max(1) as f32;
        let dev = self.device_scale.max(0.5);
        let margin = 36.0;
        let avail_phys_w = ((self.viewport_w - margin).max(64.0)) * dev;
        let avail_phys_h = ((self.viewport_h - margin).max(64.0)) * dev;
        let scale = (avail_phys_w / cw)
            .min(avail_phys_h / ch)
            .min(1.0)
            .clamp(0.10, 8.00);
        self.zoom = scale;
        let eff = self.effective_logical_zoom();
        let raw_px = ((self.viewport_w - cw * eff) * 0.5).max(12.0);
        let raw_py = ((self.viewport_h - ch * eff) * 0.5).max(12.0);
        self.pan_x = (raw_px * dev).round() / dev;
        self.pan_y = (raw_py * dev).round() / dev;
    }

    /// Whether the pixel grid should be drawn (`>= 400%` zoom).
    pub fn should_show_pixel_grid(&self) -> bool {
        self.zoom >= 4.0
    }

    /// Compute the culled visible rectangle in integer-snapped canvas coordinates intersecting `[0..canvas_w, 0..canvas_h]`.
    pub fn visible_canvas_rect(&self, canvas_w: u32, canvas_h: u32) -> RectF {
        let tl = self.screen_to_canvas(0.0, 0.0);
        let br = self.screen_to_canvas(self.viewport_w, self.viewport_h);
        let x0 = tl.x.floor().clamp(0.0, canvas_w as f64);
        let y0 = tl.y.floor().clamp(0.0, canvas_h as f64);
        let x1 = br.x.ceil().clamp(0.0, canvas_w as f64);
        let y1 = br.y.ceil().clamp(0.0, canvas_h as f64);
        RectF::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    /// Enumerate ONLY the 256x256 tile coordinates intersecting the visible viewport (Viewport Culling).
    pub fn visible_tile_coords(&self, canvas_w: u32, canvas_h: u32) -> Vec<TileCoord> {
        let vis = self.visible_canvas_rect(canvas_w, canvas_h);
        if vis.width <= 0.0 || vis.height <= 0.0 {
            return Vec::new();
        }
        let ts = TILE_SIZE as f64;
        let tx0 = (vis.x / ts).floor().max(0.0) as u32;
        let ty0 = (vis.y / ts).floor().max(0.0) as u32;
        let tx1 = ((vis.x + vis.width) / ts).ceil().max(1.0) as u32;
        let ty1 = ((vis.y + vis.height) / ts).ceil().max(1.0) as u32;

        let max_tx = canvas_w.div_ceil(TILE_SIZE);
        let max_ty = canvas_h.div_ceil(TILE_SIZE);

        let mut out = Vec::new();
        for ty in ty0..ty1.min(max_ty) {
            for tx in tx0..tx1.min(max_tx) {
                out.push(TileCoord::new(tx, ty));
            }
        }
        out
    }

    /// Compute pink Smart Alignment Guides snapping `moving` to canvas center and sibling object edges.
    ///
    /// Returns `(snapped_dx, snapped_dy, guides)`.
    pub fn compute_smart_guides(
        moving: RectF,
        canvas_w: f64,
        canvas_h: f64,
        siblings: &[RectF],
        tolerance: f64,
    ) -> (f64, f64, Vec<AlignmentGuide>) {
        let m_cx = moving.x + moving.width * 0.5;
        let m_cy = moving.y + moving.height * 0.5;
        let m_left = moving.x;
        let m_right = moving.x + moving.width;
        let m_top = moving.y;
        let m_bottom = moving.y + moving.height;

        let mut target_xs = vec![canvas_w * 0.5, 0.0, canvas_w];
        let mut target_ys = vec![canvas_h * 0.5, 0.0, canvas_h];
        for s in siblings {
            target_xs.push(s.x);
            target_xs.push(s.x + s.width * 0.5);
            target_xs.push(s.x + s.width);
            target_ys.push(s.y);
            target_ys.push(s.y + s.height * 0.5);
            target_ys.push(s.y + s.height);
        }

        let mut best_dx: Option<(f64, f64)> = None; // (delta, guide_x)
        for &tx in &target_xs {
            for &mx in &[m_cx, m_left, m_right] {
                let d = tx - mx;
                if d.abs() <= tolerance {
                    if best_dx.map_or(true, |(prev, _)| d.abs() < prev.abs()) {
                        best_dx = Some((d, tx));
                    }
                }
            }
        }

        let mut best_dy: Option<(f64, f64)> = None; // (delta, guide_y)
        for &ty in &target_ys {
            for &my in &[m_cy, m_top, m_bottom] {
                let d = ty - my;
                if d.abs() <= tolerance {
                    if best_dy.map_or(true, |(prev, _)| d.abs() < prev.abs()) {
                        best_dy = Some((d, ty));
                    }
                }
            }
        }

        let mut guides = Vec::new();
        let dx = if let Some((d, gx)) = best_dx {
            guides.push(AlignmentGuide::Vertical { x: gx });
            d
        } else {
            0.0
        };
        let dy = if let Some((d, gy)) = best_dy {
            guides.push(AlignmentGuide::Horizontal { y: gy });
            d
        } else {
            0.0
        };

        (dx, dy, guides)
    }
}

/// GPU tile/viewport texture cache keyed by [`TileCoord`], invalidated incrementally via `DirtyRegion`.
#[derive(Default)]
pub struct CanvasGpuCache {
    /// Cached per-tile `RenderImage`s (for 256x256 incremental uploads).
    pub tile_textures: HashMap<TileCoord, Arc<RenderImage>>,
    /// Last rendered visible viewport image (`SceneCompositor::render_viewport`).
    pub viewport_image: Option<Arc<RenderImage>>,
    pub viewport_rect: RectF,
    pub cached_zoom: f32,
    pub cached_image_px: (u32, u32),
    pub dirty_rebuilds: u64,
}

impl CanvasGpuCache {
    /// Synchronize dirty tiles from `session` and refresh the culled viewport image when needed.
    pub fn sync_from_session(
        &mut self,
        session: &mut DocumentSession,
        viewport: &CanvasViewportState,
    ) {
        let dirty = session.take_dirty();
        if dirty.is_dirty() {
            for tile in &dirty.tiles {
                self.tile_textures.remove(tile);
            }
            if dirty.tiles.is_empty() {
                self.tile_textures.clear();
            }
            self.viewport_image = None;
        }

        let cw = session.bitmap.width();
        let ch = session.bitmap.height();
        let vis = viewport.visible_canvas_rect(cw, ch);
        let render_zoom = viewport.zoom.clamp(0.10, 8.00);

        if self.viewport_image.is_none()
            || self.viewport_rect != vis
            || (self.cached_zoom - render_zoom).abs() > 1e-4
        {
            if vis.width >= 1.0 && vis.height >= 1.0 {
                let comp = SceneCompositor::new(&session.page, Some(&session.bitmap));
                if let Ok((vw, vh, rgba)) = comp.render_viewport(vis, render_zoom) {
                    if let Some(render_img) = crate::rgba_to_bgra_render_image(vw, vh, &rgba) {
                        self.viewport_image = Some(render_img);
                        self.viewport_rect = vis;
                        self.cached_zoom = render_zoom;
                        self.cached_image_px = (vw, vh);
                        self.dirty_rebuilds += 1;
                    }
                }
            }
        }
    }
}

/// Render the interactive canvas well with culled GPU image, 8 outer canvas handles,
/// optional `>= 400%` pixel grid indicator, pink Smart Alignment Guides, and live tool overlays.
pub fn render_canvas_surface(
    viewport: &CanvasViewportState,
    cache: &CanvasGpuCache,
    canvas_w: u32,
    canvas_h: u32,
    overlays: &CanvasInteractiveOverlays,
    theme: SnagTheme,
) -> impl IntoElement {
    let dev_scale = viewport.device_scale.max(0.5);
    let eff_zoom = viewport.effective_logical_zoom();
    let pan_x = (viewport.pan_x * dev_scale).round() / dev_scale;
    let pan_y = (viewport.pan_y * dev_scale).round() / dev_scale;
    let scaled_w = canvas_w as f32 * eff_zoom;
    let scaled_h = canvas_h as f32 * eff_zoom;
    let show_grid = viewport.should_show_pixel_grid();

    let mut surface = div()
        .relative()
        .flex_1()
        .size_full()
        .overflow_hidden()
        .bg(theme.canvas_well_bg);

    // Canvas backing frame positioned on an exact integer physical pixel `(pan_x, pan_y)`
    // with ZERO inner border on `canvas_box` itself so `img` never shifts by fractional subpixels (`1.25px`)!
    let mut canvas_box = div()
        .absolute()
        .left(px(pan_x))
        .top(px(pan_y))
        .w(px(scaled_w))
        .h(px(scaled_h))
        .bg(rgb(0xFFFFFF))
        .shadow_md()
        .child(
            div()
                .absolute()
                .inset(px(-1.0))
                .border_1()
                .border_color(theme.border_strong),
        );

    if let Some(ref render_img) = cache.viewport_image {
        let vis = cache.viewport_rect;
        let (vw, vh) = cache.cached_image_px;
        let img_left = ((vis.x as f32 * viewport.zoom).round()) / dev_scale;
        let img_top = ((vis.y as f32 * viewport.zoom).round()) / dev_scale;
        let img_w = if vw > 0 {
            vw as f32 / dev_scale
        } else {
            vis.width as f32 * eff_zoom
        };
        let img_h = if vh > 0 {
            vh as f32 / dev_scale
        } else {
            vis.height as f32 * eff_zoom
        };
        canvas_box = canvas_box.child(
            img(render_img.clone())
                .absolute()
                .left(px(img_left))
                .top(px(img_top))
                .w(px(img_w))
                .h(px(img_h)),
        );
    }

    // 8 Outer Canvas Resize Handles
    let handle_positions: &[(f32, f32)] = &[
        (-4.0, -4.0),
        (scaled_w * 0.5 - 4.0, -4.0),
        (scaled_w - 4.0, -4.0),
        (scaled_w - 4.0, scaled_h * 0.5 - 4.0),
        (scaled_w - 4.0, scaled_h - 4.0),
        (scaled_w * 0.5 - 4.0, scaled_h - 4.0),
        (-4.0, scaled_h - 4.0),
        (-4.0, scaled_h * 0.5 - 4.0),
    ];
    for &(hx, hy) in handle_positions {
        canvas_box = canvas_box.child(
            div()
                .absolute()
                .left(px(hx))
                .top(px(hy))
                .size(px(8.0))
                .bg(theme.surface_elevated)
                .border_1()
                .border_color(theme.accent_primary),
        );
    }

    // Pink Smart Alignment Guides
    for &guide in &viewport.active_guides {
        match guide {
            AlignmentGuide::Vertical { x } => {
                canvas_box = canvas_box.child(
                    div()
                        .absolute()
                        .left(px(x as f32 * eff_zoom))
                        .top(px(0.0))
                        .w(px(1.0))
                        .h_full()
                        .bg(rgb(0xEC4899)),
                );
            }
            AlignmentGuide::Horizontal { y } => {
                canvas_box = canvas_box.child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(y as f32 * eff_zoom))
                        .w_full()
                        .h(px(1.0))
                        .bg(rgb(0xEC4899)),
                );
            }
        }
    }

    // 1. Persistent Selection Marquee (`ToolMode::Selection`)
    if let Some(sel) = overlays.active_selection_rect.filter(|r| r.width >= 4.0 && r.height >= 4.0) {
        let rx = sel.x as f32 * eff_zoom;
        let ry = sel.y as f32 * eff_zoom;
        let rw = (sel.width as f32 * eff_zoom).max(2.0);
        let rh = (sel.height as f32 * eff_zoom).max(2.0);
        let badge_text = format!("{:.0} × {:.0} px", sel.width, sel.height);
        let (bx, by) = compute_marquee_badge_origin(rx, ry, rw, rh, scaled_w, scaled_h, 96.0);

        canvas_box = canvas_box
            .child(
                div()
                    .absolute()
                    .left(px(rx))
                    .top(px(ry))
                    .w(px(rw))
                    .h(px(rh))
                    .border_2()
                    .border_dashed()
                    .border_color(rgb(0x00A4E4))
                    .bg(rgba(0x00A4E418)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(bx))
                    .top(px(by))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .bg(rgba(0x111827E6))
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(rgb(0xFFFFFF))
                    .child(badge_text),
            );
    }

    // 2. Pending Crop Veil + 8 Handles (`ToolMode::Crop` awaiting Enter / ✓ Crop)
    if let Some(crop) = overlays.pending_crop_rect {
        let cx_px = (crop.x as f32 * eff_zoom).clamp(0.0, scaled_w);
        let cy_px = (crop.y as f32 * eff_zoom).clamp(0.0, scaled_h);
        let cw_px = (crop.width as f32 * eff_zoom).clamp(2.0, (scaled_w - cx_px).max(2.0));
        let ch_px = (crop.height as f32 * eff_zoom).clamp(2.0, (scaled_h - cy_px).max(2.0));
        let veil = rgba(0x00000070);

        // 4-band darkened exterior veil outside crop box
        canvas_box = canvas_box
            .child(div().absolute().left(px(0.0)).top(px(0.0)).w(px(scaled_w)).h(px(cy_px)).bg(veil))
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(cy_px + ch_px))
                    .w(px(scaled_w))
                    .h(px((scaled_h - (cy_px + ch_px)).max(0.0)))
                    .bg(veil),
            )
            .child(div().absolute().left(px(0.0)).top(px(cy_px)).w(px(cx_px)).h(px(ch_px)).bg(veil))
            .child(
                div()
                    .absolute()
                    .left(px(cx_px + cw_px))
                    .top(px(cy_px))
                    .w(px((scaled_w - (cx_px + cw_px)).max(0.0)))
                    .h(px(ch_px))
                    .bg(veil),
            );

        let mut crop_frame = div()
            .absolute()
            .left(px(cx_px))
            .top(px(cy_px))
            .w(px(cw_px))
            .h(px(ch_px))
            .border_2()
            .border_dashed()
            .border_color(rgb(0xF5A623));

        let crop_handles: &[(f32, f32)] = &[
            (-5.0, -5.0),
            (cw_px * 0.5 - 5.0, -5.0),
            (cw_px - 5.0, -5.0),
            (cw_px - 5.0, ch_px * 0.5 - 5.0),
            (cw_px - 5.0, ch_px - 5.0),
            (cw_px * 0.5 - 5.0, ch_px - 5.0),
            (-5.0, ch_px - 5.0),
            (-5.0, ch_px * 0.5 - 5.0),
        ];
        for &(hx, hy) in crop_handles {
            crop_frame = crop_frame.child(
                div()
                    .absolute()
                    .left(px(hx))
                    .top(px(hy))
                    .size(px(10.0))
                    .bg(rgb(0x111827))
                    .border_2()
                    .border_color(rgb(0xF5A623)),
            );
        }
        canvas_box = canvas_box.child(crop_frame);
    }

    // 3. Live Region Drag Marquee (`Selection`, `Blur`, `Magnify`, `CutOut`, `Crop` while mouse button held)
    if let Some((tool, drag_r_ref)) = overlays
        .active_region_drag
        .as_ref()
        .filter(|(_, r)| r.width >= 4.0 || r.height >= 4.0)
    {
        let drag_r = *drag_r_ref;
        let dx = drag_r.x as f32 * eff_zoom;
        let dy = drag_r.y as f32 * eff_zoom;
        let dw = (drag_r.width as f32 * eff_zoom).max(2.0);
        let dh = (drag_r.height as f32 * eff_zoom).max(2.0);
        let (border_col, tint_col) = match tool {
            ToolMode::Crop => (rgb(0xF5A623), rgba(0xF5A62318)),
            ToolMode::Blur => (rgb(0x8B5CF6), rgba(0x8B5CF622)),
            ToolMode::CutOut => (rgb(0xEF4444), rgba(0xEF444422)),
            _ => (rgb(0x00A4E4), rgba(0x00A4E41E)),
        };
        let badge_text = format!(
            "{} · {:.0} × {:.0} px",
            tool.as_str(),
            drag_r.width,
            drag_r.height
        );
        let (bx, by) = compute_marquee_badge_origin(dx, dy, dw, dh, scaled_w, scaled_h, 152.0);

        canvas_box = canvas_box
            .child(
                div()
                    .absolute()
                    .left(px(dx))
                    .top(px(dy))
                    .w(px(dw))
                    .h(px(dh))
                    .border_2()
                    .border_dashed()
                    .border_color(border_col)
                    .bg(tint_col),
            )
            .child(
                div()
                    .absolute()
                    .left(px(bx))
                    .top(px(by))
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .bg(rgba(0x111827E6))
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(rgb(0xFFFFFF))
                    .child(badge_text),
            );
    }

    // 4. Active Inline Text / Callout Editing Focus Frame + Caret Indicator
    if let Some((text_r, ref text_preview)) = overlays.editing_text_box {
        let tx = text_r.x as f32 * eff_zoom;
        let ty = text_r.y as f32 * eff_zoom;
        let tw = (text_r.width as f32 * eff_zoom).max(40.0);
        let th = (text_r.height as f32 * eff_zoom).max(28.0);
        let label = if text_preview.is_empty() {
            "Type text... | (Esc to finish)".to_string()
        } else {
            format!("{text_preview}|")
        };
        let (bx, by) = compute_marquee_badge_origin(tx, ty, tw, th, scaled_w, scaled_h, 220.0);
        canvas_box = canvas_box
            .child(
                div()
                    .absolute()
                    .left(px(tx))
                    .top(px(ty))
                    .w(px(tw))
                    .h(px(th))
                    .border_2()
                    .border_dashed()
                    .border_color(rgb(0x2563EB)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(bx))
                    .top(px(by))
                    .max_w(px(360.0))
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(rgb(0x1D4ED8))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(0xFFFFFF))
                    .child(label),
            );
    }

    surface = surface.child(canvas_box).when(show_grid, |el| {
        el.child(
            div()
                .absolute()
                .top(px(8.0))
                .left(px(8.0))
                .px(px(6.0))
                .py(px(2.0))
                .rounded(theme.radius_sm)
                .bg(rgba(0x111827CC))
                .text_xs()
                .text_color(rgb(0xF9FAFB))
                .child(format!("Pixel Grid ON ({:.0}%)", viewport.zoom_percent())),
        )
    });

    surface
}

/// Position a floating marquee badge independently of the marquee's own width so narrow
/// selections never wrap characters vertically, flipping below the marquee when near the
/// top edge of the canvas and clamping horizontally within canvas bounds.
pub fn compute_marquee_badge_origin(
    rx: f32,
    ry: f32,
    _rw: f32,
    rh: f32,
    canvas_w: f32,
    canvas_h: f32,
    est_badge_w: f32,
) -> (f32, f32) {
    let bx = rx.clamp(0.0, (canvas_w - est_badge_w).max(0.0));
    let by = if ry >= 26.0 {
        ry - 24.0
    } else if ry + rh + 26.0 <= canvas_h {
        ry + rh + 6.0
    } else {
        (ry + 6.0).min((canvas_h - 22.0).max(0.0))
    };
    (bx, by)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchored_zoom_and_20k_tall_viewport_culling() {
        let mut vp = CanvasViewportState {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: -5000.0,
            viewport_w: 1200.0,
            viewport_h: 800.0,
            ..Default::default()
        };

        // 1920 x 20000 tall scrolling image: culling must return only ~20-32 visible tiles, NOT 600+!
        let tiles = vp.visible_tile_coords(1920, 20000);
        assert!(!tiles.is_empty());
        assert!(tiles.len() <= 40, "Viewport culling failed: got {} tiles", tiles.len());

        // Cursor-anchored zoom at odd 137% keeps canvas point pinned
        let before = vp.screen_to_canvas(400.0, 300.0);
        vp.zoom_anchored_at(1.37, 400.0, 300.0);
        let after = vp.screen_to_canvas(400.0, 300.0);
        assert!((before.x - after.x).abs() < 1e-3);
        assert!((before.y - after.y).abs() < 1e-3);

        // Pixel grid activates at >= 400%
        assert!(!vp.should_show_pixel_grid());
        vp.zoom_anchored_at(4.0, 400.0, 300.0);
        assert!(vp.should_show_pixel_grid());
    }

    #[test]
    fn smart_alignment_guides_snap_to_center_and_siblings() {
        let moving = RectF::new(497.0, 298.0, 100.0, 60.0); // center (547, 328), canvas center (550, 330)
        let (dx, dy, guides) =
            CanvasViewportState::compute_smart_guides(moving, 1100.0, 660.0, &[], 5.0);
        assert!((dx - 3.0).abs() < 1e-6);
        assert!((dy - 2.0).abs() < 1e-6);
        assert_eq!(guides.len(), 2);
    }

    #[test]
    fn marquee_badge_origin_flips_below_near_top_and_clamps_right() {
        // Normal middle selection: badge placed 24px above top-left
        let (x1, y1) = compute_marquee_badge_origin(120.0, 80.0, 24.0, 154.0, 800.0, 600.0, 152.0);
        assert_eq!((x1, y1), (120.0, 56.0));

        // Near top edge (ry < 26): badge flips cleanly 6px below bottom of marquee
        let (x2, y2) = compute_marquee_badge_origin(200.0, 10.0, 24.0, 154.0, 800.0, 600.0, 152.0);
        assert_eq!((x2, y2), (200.0, 170.0));

        // Near right edge: horizontal origin clamped so badge stays inside canvas
        let (x3, _) = compute_marquee_badge_origin(760.0, 100.0, 24.0, 40.0, 800.0, 600.0, 152.0);
        assert_eq!(x3, 648.0);
    }
}
