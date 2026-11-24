//! Interactive Per-Monitor High-DPI Screen Selection Overlay (`W3-03` + Snagit Floating All-in-One Dialog):
//! - Renders each monitor's `FrozenScreen` at 1:1 physical pixels (`BGRA8` via `rgba_to_bgra_render_image`,
//!   `ObjectFit::Fill` -> zero gray letterbox bars) with full `window.scale_factor()` DPI coordinate mapping
//! - Auto-detects visible top-level windows under cursor in monitor-local physical coordinates
//! - 11x11 Pixel Magnifier Loupe (`(X, Y)`, `W x H px`, `#RRGGBB`, 1px arrow-key nudge)
//! - Snagit's Post-Selection Floating All-in-One Dialog (`Image`, `Video`, `Scrolling`, `Redo`, `Cancel`)
//!   with bright interior cutout, 8 draggable resize handles, and in-place Video Pre-Recording Toolbar
//!   (`Record`, `Mic`, `System Audio`, `3-2-1` Countdown)

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, ParentElement, Render, RenderImage, StatefulInteractiveElement, Styled, StyledImage,
    Window, WindowBounds, WindowKind, WindowOptions, div, img, prelude::FluentBuilder, px, rgb,
    rgba, size,
};
use snagsoup_canvas::TiledBitmap;
use snagsoup_capture::{FrozenScreen, ScreenFreezer};
use snagsoup_model::{CaptureObject, ColorRgba, PointF, RectF};
use snagsoup_stitch::{
    AutoScrollConfig, AutoScrollController, RgbaFrame, stitch_scrolling_sequence,
};
use snagsoup_ui_huds::{
    ResizeHandle, extract_hwnd, hide_window_hwnd, position_window_physical,
};
use snagsoup_win_platform::dpi::{PhysicalPoint, PhysicalRect};
use snagsoup_win_platform::object_finder::{
    WindowHierarchyEntry, WindowInfo, enumerate_window_hierarchy_fast, find_snagit_target_at,
    find_topmost_window_at,
};

use crate::icons::SnagIcon;
use crate::theme::SnagTheme;
use crate::{bgra_vec_to_render_image, rgba_to_bgra_render_image};

/// Aspect-ratio / standard resolution guideline presets shown when holding `Shift`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuidelinePreset {
    Free,
    Ratio16x9,
    Ratio4x3,
    Ratio1x1,
    Hd1080p,
    Hd720p,
}

impl GuidelinePreset {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Ratio16x9 => "16:9",
            Self::Ratio4x3 => "4:3",
            Self::Ratio1x1 => "1:1",
            Self::Hd1080p => "1920×1080",
            Self::Hd720p => "1280×720",
        }
    }

    /// Constrain a dragged rectangle `(anchor -> current)` to this guideline.
    pub fn constrain(self, anchor: PointF, current: PointF) -> RectF {
        let dx = current.x - anchor.x;
        let dy = current.y - anchor.y;
        let sx = if dx < 0.0 { -1.0 } else { 1.0 };
        let sy = if dy < 0.0 { -1.0 } else { 1.0 };
        let aw = dx.abs();
        let ah = dy.abs();

        let (w, h) = match self {
            Self::Free => (aw, ah),
            Self::Ratio16x9 => {
                let h = (aw * 9.0 / 16.0).max(1.0);
                (aw.max(1.0), h)
            }
            Self::Ratio4x3 => {
                let h = (aw * 3.0 / 4.0).max(1.0);
                (aw.max(1.0), h)
            }
            Self::Ratio1x1 => {
                let side = aw.max(ah).max(1.0);
                (side, side)
            }
            Self::Hd1080p => (1920.0, 1080.0),
            Self::Hd720p => (1280.0, 720.0),
        };

        let x = if sx < 0.0 { anchor.x - w } else { anchor.x };
        let y = if sy < 0.0 { anchor.y - h } else { anchor.y };
        RectF::new(x, y, w, h)
    }
}

/// Lifecycle phase of the interactive Selection Overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPhase {
    /// Live crosshair + 11×11 magnifier loupe + window auto-detection.
    Selecting,
    /// Selection confirmed: bright interior, 8 resize handles, Snagit Floating All-in-One Dialog.
    PostSelectionToolbar,
    /// In-place Video Pre-Recording Toolbar (`Record`, `Mic`, `System Audio`, `Redo`, `Cancel`).
    VideoPreRecord,
    /// 3 -> 2 -> 1 countdown before closing overlay and starting live screen recording.
    Countdown(u8),
}

/// Request emitted when the user clicks **Record** in the Video Pre-Recording Toolbar.
#[derive(Debug, Clone, Copy)]
pub struct VideoRecordRequest {
    /// Selected region in virtual-desktop physical screen coordinates.
    pub global_rect: PhysicalRect,
    /// Optional target `HMONITOR` handle from the monitor where the selection was made.
    pub hmonitor: Option<isize>,
    pub record_mic: bool,
    pub record_system_audio: bool,
}

/// Scrollable directions detected under the hovered window/control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollArrowsHint {
    pub vertical: bool,
    pub horizontal: bool,
}

/// Single 11x11 pixel sample extracted from a [`FrozenScreen`] for the Magnifier Loupe.
#[derive(Debug, Clone)]
pub struct MagnifierSample {
    pub grid_size: usize,
    /// `grid_size * grid_size` RGB24 colors (`0xRRGGBB`).
    pub pixels: Vec<u32>,
    pub center_rgb: u32,
}

impl MagnifierSample {
    pub fn center_hex(&self) -> String {
        format!("#{:06X}", self.center_rgb & 0x00FF_FFFF)
    }
}

/// Minimum mouse movement in physical pixels to distinguish a freehand Region drag
/// from a Snagit single-click Window Auto-Detect selection.
pub const DRAG_THRESHOLD_PX: i32 = 6;

/// Pure, testable state for the interactive selection overlay (all coords in monitor-local physical pixels).
#[derive(Debug, Clone)]
pub struct SelectionOverlayState {
    pub monitor_left: i32,
    pub monitor_top: i32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub cursor_x: i32,
    pub cursor_y: i32,
    pub cursor_active: bool,
    pub press_origin: Option<(i32, i32)>,
    pub drag_anchor: Option<PointF>,
    pub selection: Option<RectF>,
    pub autodetect_rect: Option<RectF>,
    pub autodetect_title: Option<String>,
    pub scroll_hint: ScrollArrowsHint,
    pub guideline: GuidelinePreset,
    pub shift_held: bool,
    pub confirmed: Option<RectF>,
    pub phase: OverlayPhase,
    pub active_resize_handle: Option<ResizeHandle>,
    pub last_resize_pos: Option<(i32, i32)>,
    pub record_mic: bool,
    pub record_system_audio: bool,
    pub start_in_video_mode: bool,
}

impl SelectionOverlayState {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self::new_for_monitor(0, 0, screen_width, screen_height)
    }

    pub fn new_for_monitor(
        monitor_left: i32,
        monitor_top: i32,
        screen_width: u32,
        screen_height: u32,
    ) -> Self {
        Self {
            monitor_left,
            monitor_top,
            screen_width,
            screen_height,
            cursor_x: (screen_width / 2) as i32,
            cursor_y: (screen_height / 2) as i32,
            cursor_active: true,
            press_origin: None,
            drag_anchor: None,
            selection: None,
            autodetect_rect: None,
            autodetect_title: None,
            scroll_hint: ScrollArrowsHint::default(),
            guideline: GuidelinePreset::Free,
            shift_held: false,
            confirmed: None,
            phase: OverlayPhase::Selecting,
            active_resize_handle: None,
            last_resize_pos: None,
            record_mic: true,
            record_system_audio: true,
            start_in_video_mode: false,
        }
    }

    /// Move cursor by `(dx, dy)` physical pixels (e.g. 1px arrow-key precision nudge).
    pub fn nudge_cursor(&mut self, dx: i32, dy: i32) {
        let max_x = self.screen_width.saturating_sub(1) as i32;
        let max_y = self.screen_height.saturating_sub(1) as i32;
        self.cursor_x = (self.cursor_x + dx).clamp(0, max_x);
        self.cursor_y = (self.cursor_y + dy).clamp(0, max_y);
        self.update_drag_from_cursor();
    }

    /// Begin a drag selection at the current cursor position.
    pub fn begin_drag(&mut self) {
        self.drag_anchor = Some(PointF::new(self.cursor_x as f64, self.cursor_y as f64));
        self.update_drag_from_cursor();
    }

    /// Recompute active selection rect from `drag_anchor` and `(cursor_x, cursor_y)`.
    pub fn update_drag_from_cursor(&mut self) {
        if let Some(anchor) = self.drag_anchor {
            let cur = PointF::new(self.cursor_x as f64, self.cursor_y as f64);
            let preset = if self.shift_held && self.guideline == GuidelinePreset::Free {
                GuidelinePreset::Ratio16x9
            } else {
                self.guideline
            };
            self.selection = Some(self.clamp_rect(preset.constrain(anchor, cur)));
        }
    }

    /// Clamp a monitor-local physical rectangle to `[0..screen_width, 0..screen_height]`.
    pub fn clamp_rect(&self, rect: RectF) -> RectF {
        let max_w = self.screen_width.max(1) as f64;
        let max_h = self.screen_height.max(1) as f64;
        let x0 = rect.x.clamp(0.0, max_w - 1.0);
        let y0 = rect.y.clamp(0.0, max_h - 1.0);
        let x1 = (rect.x + rect.width).clamp(x0 + 1.0, max_w);
        let y1 = (rect.y + rect.height).clamp(y0 + 1.0, max_h);
        RectF::new(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0))
    }

    /// Confirm the active drag selection (or auto-detected window rect) via `Enter`/`Space` or mouse-up.
    pub fn confirm_selection(&mut self) -> Option<RectF> {
        let chosen = self
            .selection
            .filter(|r| r.width >= 4.0 && r.height >= 4.0)
            .or(self.autodetect_rect)
            .map(|r| self.clamp_rect(r));
        if let Some(rect) = chosen {
            self.selection = Some(rect);
            self.confirmed = Some(rect);
        }
        chosen
    }

    /// Transition from `Selecting` to `PostSelectionToolbar` (or `VideoPreRecord`).
    pub fn enter_post_selection(&mut self) -> Option<RectF> {
        let rect = self.confirm_selection()?;
        self.drag_anchor = None;
        self.active_resize_handle = None;
        self.phase = if self.start_in_video_mode {
            OverlayPhase::VideoPreRecord
        } else {
            OverlayPhase::PostSelectionToolbar
        };
        Some(rect)
    }

    /// Record mouse-down origin without discarding `autodetect_rect` until the user drags `>= 6px`.
    pub fn on_pointer_press(&mut self, phys_x: i32, phys_y: i32, shift: bool) {
        let max_x = self.screen_width.saturating_sub(1) as i32;
        let max_y = self.screen_height.saturating_sub(1) as i32;
        let px_clamped = phys_x.clamp(0, max_x);
        let py_clamped = phys_y.clamp(0, max_y);
        self.cursor_x = px_clamped;
        self.cursor_y = py_clamped;
        self.shift_held = shift;
        self.press_origin = Some((px_clamped, py_clamped));
        self.drag_anchor = None;
        self.selection = None;
    }

    /// Update pointer position: returns `true` if a region drag (`>= 6px`) is active,
    /// or `false` when hovering for live window auto-detect.
    pub fn on_pointer_move(&mut self, phys_x: i32, phys_y: i32, shift: bool) -> bool {
        let max_x = self.screen_width.saturating_sub(1) as i32;
        let max_y = self.screen_height.saturating_sub(1) as i32;
        self.cursor_x = phys_x.clamp(0, max_x);
        self.cursor_y = phys_y.clamp(0, max_y);
        self.shift_held = shift;

        if let Some((ox, oy)) = self.press_origin {
            if self.drag_anchor.is_none()
                && ((self.cursor_x - ox).abs() >= DRAG_THRESHOLD_PX
                    || (self.cursor_y - oy).abs() >= DRAG_THRESHOLD_PX)
            {
                self.drag_anchor = Some(PointF::new(ox as f64, oy as f64));
            }
            if self.drag_anchor.is_some() {
                self.update_drag_from_cursor();
                return true;
            }
            // Pressed but still within `< 6px` click tolerance: keep showing `autodetect_rect`!
            return true;
        }
        false
    }

    /// Release pointer: if the user dragged `>= 6px`, confirms the dragged region;
    /// otherwise (single click `< 6px`) confirms the hovered `autodetect_rect` (or full screen).
    pub fn on_pointer_release(&mut self, phys_x: i32, phys_y: i32) -> Option<RectF> {
        let max_x = self.screen_width.saturating_sub(1) as i32;
        let max_y = self.screen_height.saturating_sub(1) as i32;
        self.cursor_x = phys_x.clamp(0, max_x);
        self.cursor_y = phys_y.clamp(0, max_y);
        self.press_origin = None;

        if self.drag_anchor.is_some() {
            self.update_drag_from_cursor();
            self.drag_anchor = None;
        } else if self.selection.is_none() && self.autodetect_rect.is_none() {
            // Single click on empty desktop -> select Fullscreen monitor
            self.selection = Some(RectF::new(
                0.0,
                0.0,
                self.screen_width.max(1) as f64,
                self.screen_height.max(1) as f64,
            ));
        }
        self.enter_post_selection()
    }

    /// Select the entire monitor (`Fullscreen` button).
    pub fn select_fullscreen(&mut self) -> Option<RectF> {
        self.press_origin = None;
        self.drag_anchor = None;
        self.selection = Some(RectF::new(
            0.0,
            0.0,
            self.screen_width.max(1) as f64,
            self.screen_height.max(1) as f64,
        ));
        self.enter_post_selection()
    }

    /// Reset selection and return to live crosshair `Selecting` phase.
    pub fn redo_selection(&mut self) {
        self.press_origin = None;
        self.drag_anchor = None;
        self.selection = None;
        self.confirmed = None;
        self.active_resize_handle = None;
        self.last_resize_pos = None;
        self.phase = OverlayPhase::Selecting;
    }

    /// Fine-tune the confirmed selection rectangle by dragging one of the 8 resize handles.
    pub fn apply_resize_handle_drag(&mut self, handle: ResizeHandle, dx: i32, dy: i32) {
        const MIN_DIM: f64 = 16.0;
        let Some(r) = self.confirmed.or(self.selection) else {
            return;
        };
        let max_w = self.screen_width.max(16) as f64;
        let max_h = self.screen_height.max(16) as f64;
        let mut left = r.x;
        let mut top = r.y;
        let mut right = r.x + r.width;
        let mut bottom = r.y + r.height;
        let dxf = dx as f64;
        let dyf = dy as f64;

        match handle {
            ResizeHandle::TopLeft => {
                left = (left + dxf).clamp(0.0, right - MIN_DIM);
                top = (top + dyf).clamp(0.0, bottom - MIN_DIM);
            }
            ResizeHandle::Top => {
                top = (top + dyf).clamp(0.0, bottom - MIN_DIM);
            }
            ResizeHandle::TopRight => {
                right = (right + dxf).clamp(left + MIN_DIM, max_w);
                top = (top + dyf).clamp(0.0, bottom - MIN_DIM);
            }
            ResizeHandle::Right => {
                right = (right + dxf).clamp(left + MIN_DIM, max_w);
            }
            ResizeHandle::BottomRight => {
                right = (right + dxf).clamp(left + MIN_DIM, max_w);
                bottom = (bottom + dyf).clamp(top + MIN_DIM, max_h);
            }
            ResizeHandle::Bottom => {
                bottom = (bottom + dyf).clamp(top + MIN_DIM, max_h);
            }
            ResizeHandle::BottomLeft => {
                left = (left + dxf).clamp(0.0, right - MIN_DIM);
                bottom = (bottom + dyf).clamp(top + MIN_DIM, max_h);
            }
            ResizeHandle::Left => {
                left = (left + dxf).clamp(0.0, right - MIN_DIM);
            }
        }

        let updated = RectF::new(left, top, right - left, bottom - top);
        self.selection = Some(updated);
        self.confirmed = Some(updated);
    }

    /// Convert a monitor-local physical `RectF` into a virtual-desktop [`PhysicalRect`].
    pub fn to_global_physical_rect(&self, local: RectF) -> PhysicalRect {
        let left = self.monitor_left + local.x.round() as i32;
        let top = self.monitor_top + local.y.round() as i32;
        let w = local.width.round().max(2.0) as i32;
        let h = local.height.round().max(2.0) as i32;
        PhysicalRect::new(left, top, left + w, top + h)
    }

    /// Sample an `11x11` (radius = 5) pixel grid centered on `(cursor_x, cursor_y)` from `FrozenScreen`.
    pub fn sample_magnifier(&self, frozen: Option<&FrozenScreen>) -> MagnifierSample {
        const RADIUS: i32 = 5;
        const GRID: usize = 11;
        let mut pixels = vec![0x1E2229u32; GRID * GRID];

        if let Some(screen) = frozen {
            let w = screen.frame.width as i32;
            let h = screen.frame.height as i32;
            let raw = &screen.frame.rgba;
            for gy in 0..GRID {
                let sy = self.cursor_y + (gy as i32 - RADIUS);
                for gx in 0..GRID {
                    let sx = self.cursor_x + (gx as i32 - RADIUS);
                    if sx >= 0 && sx < w && sy >= 0 && sy < h {
                        let idx = ((sy as usize) * (w as usize) + (sx as usize)) * 4;
                        if idx + 2 < raw.len() {
                            let r = raw[idx] as u32;
                            let g = raw[idx + 1] as u32;
                            let b = raw[idx + 2] as u32;
                            pixels[gy * GRID + gx] = (r << 16) | (g << 8) | b;
                        }
                    }
                }
            }
        }

        let center_rgb = pixels[(GRID / 2) * GRID + (GRID / 2)];
        MagnifierSample {
            grid_size: GRID,
            pixels,
            center_rgb,
        }
    }

    /// Sample an `11 x 11` RGB grid directly from a BGRA8 [`RenderImage`] (zero extra buffer copy!).
    pub fn sample_magnifier_bgra_bytes(&self, bgra: Option<&[u8]>) -> MagnifierSample {
        const GRID: usize = 11;
        const RADIUS: i32 = 5;
        let mut pixels = vec![0x000000u32; GRID * GRID];

        if let Some(raw) = bgra {
            let w = self.screen_width as i32;
            let h = self.screen_height as i32;
            for gy in 0..GRID {
                let sy = self.cursor_y + (gy as i32 - RADIUS);
                for gx in 0..GRID {
                    let sx = self.cursor_x + (gx as i32 - RADIUS);
                    if sx >= 0 && sx < w && sy >= 0 && sy < h {
                        let idx = ((sy as usize) * (w as usize) + (sx as usize)) * 4;
                        if idx + 2 < raw.len() {
                            let b = raw[idx] as u32;
                            let g = raw[idx + 1] as u32;
                            let r = raw[idx + 2] as u32;
                            pixels[gy * GRID + gx] = (r << 16) | (g << 8) | b;
                        }
                    }
                }
            }
        }

        let center_rgb = pixels[(GRID / 2) * GRID + (GRID / 2)];
        MagnifierSample {
            grid_size: GRID,
            pixels,
            center_rgb,
        }
    }
}

/// Ultra-fast crop directly out of a `BGRA8` [`RenderImage`] — only copies and converts `B <-> R`
/// on the small selected `(cw × ch)` sub-rectangle (`< 0.5ms`), never touching the rest of the 4K screen!
fn crop_from_bgra_render_image(
    render_img: &RenderImage,
    fw: u32,
    fh: u32,
    bounds: PhysicalRect,
    cursor: Option<&snagsoup_win_platform::cursor::CapturedCursor>,
    rect: RectF,
    include_cursor: bool,
) -> (TiledBitmap, Option<CaptureObject>) {
    let cx = (rect.x.round().max(0.0) as u32).min(fw.saturating_sub(1));
    let cy = (rect.y.round().max(0.0) as u32).min(fh.saturating_sub(1));
    let cw = (rect.width.round().max(1.0) as u32)
        .min(fw.saturating_sub(cx))
        .max(1);
    let ch = (rect.height.round().max(1.0) as u32)
        .min(fh.saturating_sub(cy))
        .max(1);

    let cropped = if let Some(bgra) = render_img.as_bytes(0) {
        let mut rgba = vec![0u8; (cw as usize) * (ch as usize) * 4];
        let src_stride = (fw as usize) * 4;
        let dst_stride = (cw as usize) * 4;
        for row in 0..ch as usize {
            let s_off = ((cy as usize + row) * src_stride) + (cx as usize) * 4;
            let d_off = row * dst_stride;
            if s_off + dst_stride <= bgra.len() {
                let srow = &bgra[s_off..s_off + dst_stride];
                let drow = &mut rgba[d_off..d_off + dst_stride];
                for (s, d) in srow.chunks_exact(4).zip(drow.chunks_exact_mut(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
        }
        TiledBitmap::from_rgba(cw, ch, &rgba)
            .unwrap_or_else(|_| TiledBitmap::new_filled(cw, ch, ColorRgba::WHITE))
    } else {
        TiledBitmap::new_filled(cw, ch, ColorRgba::WHITE)
    };

    let cursor_obj = if include_cursor {
        cursor.and_then(|c| {
            let ox = bounds.left + cx as i32;
            let oy = bounds.top + cy as i32;
            c.intersects_rect(ox, oy, cw, ch)
                .then(|| c.to_capture_object("captured-cursor", ox, oy))
        })
    } else {
        None
    };

    (cropped, cursor_obj)
}

/// Crop a [`FrozenScreen`] to `rect`, returning the cropped [`TiledBitmap`] and an optional
/// movable `ToolMode::Cursor` object shifted into cropped coordinates.
pub fn crop_frozen_screen(
    frozen: &FrozenScreen,
    rect: RectF,
    include_cursor: bool,
) -> (TiledBitmap, Option<CaptureObject>) {
    crate::record_last_capture_rect(rect);
    let fw = frozen.frame.width;
    let fh = frozen.frame.height;
    let cx = (rect.x.round().max(0.0) as u32).min(fw.saturating_sub(1));
    let cy = (rect.y.round().max(0.0) as u32).min(fh.saturating_sub(1));
    let cw = (rect.width.round().max(1.0) as u32)
        .min(fw.saturating_sub(cx))
        .max(1);
    let ch = (rect.height.round().max(1.0) as u32)
        .min(fh.saturating_sub(cy))
        .max(1);

    let mut sub_rgba = vec![0u8; (cw as usize) * (ch as usize) * 4];
    let src_stride = (fw as usize) * 4;
    let dst_stride = (cw as usize) * 4;
    for row in 0..ch as usize {
        let s_off = ((cy as usize + row) * src_stride) + (cx as usize) * 4;
        let d_off = row * dst_stride;
        if s_off + dst_stride <= frozen.frame.rgba.len() {
            sub_rgba[d_off..d_off + dst_stride]
                .copy_from_slice(&frozen.frame.rgba[s_off..s_off + dst_stride]);
        }
    }
    let cropped = TiledBitmap::from_rgba(cw, ch, &sub_rgba)
        .unwrap_or_else(|_| TiledBitmap::new_filled(cw, ch, ColorRgba::WHITE));

    let cursor_obj = if include_cursor {
        frozen.cursor.as_ref().and_then(|c| {
            let ox = frozen.frame.bounds.left + cx as i32;
            let oy = frozen.frame.bounds.top + cy as i32;
            c.intersects_rect(ox, oy, cw, ch)
                .then(|| c.to_capture_object("captured-cursor", ox, oy))
        })
    } else {
        None
    };

    (cropped, cursor_obj)
}

/// Capture a scrolling sequence over `global_rect` via `AutoScrollController` + `stitch_scrolling_sequence`.
fn capture_scrolling_region(global_rect: PhysicalRect) -> Option<TiledBitmap> {
    let w = global_rect.width().max(1) as u32;
    let h = global_rect.height().max(1) as u32;
    let cancel = Arc::new(AtomicBool::new(false));
    let ctrl = AutoScrollController::new(AutoScrollConfig {
        max_steps: 24,
        settle_poll_interval: Duration::from_millis(60),
        max_settle_checks: 6,
        zero_shift_stop_count: 2,
        ..AutoScrollConfig::default()
    });

    // Give the desktop a brief moment after overlay dismissal
    std::thread::sleep(Duration::from_millis(120));

    let mut freezer = ScreenFreezer::new();
    let result = ctrl.run(
        &cancel,
        || -> Result<RgbaFrame, String> {
            let frozen = freezer
                .freeze_virtual_desktop()
                .map_err(|e| e.to_string())?;
            let local_rect = RectF::new(
                (global_rect.left - frozen.frame.bounds.left) as f64,
                (global_rect.top - frozen.frame.bounds.top) as f64,
                global_rect.width().max(1) as f64,
                global_rect.height().max(1) as f64,
            );
            let (bmp, _) = crop_frozen_screen(&frozen, local_rect, false);
            RgbaFrame::new(bmp.width(), bmp.height(), bmp.to_rgba()).map_err(|e| e.to_string())
        },
        || -> Result<(), String> {
            let _ = snagsoup_win_platform::scroll::send_scroll_wheel(-360, 0);
            std::thread::sleep(Duration::from_millis(90));
            Ok(())
        },
    );

    let (frames, _) = result.ok()?;
    if frames.is_empty() {
        return None;
    }
    let stitched = if frames.len() == 1 {
        frames.into_iter().next()?
    } else {
        stitch_scrolling_sequence(&frames)
            .ok()
            .map(|(frame, _, _)| frame)
            .unwrap_or_else(|| frames.into_iter().next().unwrap())
    };
    TiledBitmap::from_rgba(
        stitched.width.max(w.min(1)),
        stitched.height.max(h.min(1)),
        &stitched.rgba,
    )
    .ok()
}

/// Hide every overlay `HWND` immediately via Win32 (`SW_HIDE` + `HWND_NOTOPMOST`) and remove all
/// sibling GPUI overlay windows tracked in `handles`.
fn close_all_overlays(
    handles: &Rc<RefCell<Vec<AnyWindowHandle>>>,
    hwnds: &Rc<RefCell<Vec<isize>>>,
    cx: &mut App,
) {
    let raw_hwnds: Vec<isize> = hwnds.borrow_mut().drain(..).collect();
    for hwnd in raw_hwnds {
        let _ = hide_window_hwnd(hwnd);
    }
    let list: Vec<AnyWindowHandle> = handles.borrow_mut().drain(..).collect();
    for h in list {
        let _ = h.update(cx, |_, window, _| {
            if let Some(hwnd) = extract_hwnd(window) {
                let _ = hide_window_hwnd(hwnd);
            }
            window.remove_window();
        });
    }
}

/// Open interactive Selection Overlay window(s) (one per monitor when multi-monitor capture is
/// available) with Snagit's Floating All-in-One Post-Selection Dialog, Image/Video/Scrolling
/// actions, and High-DPI 1:1 physical alignment.
pub fn open_selection_overlay_window(
    cx: &mut App,
    include_cursor: bool,
    on_captured: impl Fn(TiledBitmap, Option<CaptureObject>, &mut App) + 'static,
) {
    open_selection_overlay_window_with_video(
        cx,
        include_cursor,
        false,
        on_captured,
        |req, cx| {
            crate::start_live_video_recording_flow(req, cx);
        },
    );
}

/// Full-featured entry point allowing callers to open in Video mode and supply both Image and
/// Video recording callbacks.
pub fn open_selection_overlay_window_with_video(
    cx: &mut App,
    include_cursor: bool,
    start_in_video_mode: bool,
    on_captured: impl Fn(TiledBitmap, Option<CaptureObject>, &mut App) + 'static,
    on_record_video: impl Fn(VideoRecordRequest, &mut App) + 'static,
) {
    open_selection_overlay_window_full(
        cx,
        include_cursor,
        start_in_video_mode,
        on_captured,
        on_record_video,
        |_| {},
    );
}

/// Full-featured entry point with `on_captured`, `on_record_video`, and `on_cancel` callbacks.
pub fn open_selection_overlay_window_full(
    cx: &mut App,
    include_cursor: bool,
    start_in_video_mode: bool,
    on_captured: impl Fn(TiledBitmap, Option<CaptureObject>, &mut App) + 'static,
    on_record_video: impl Fn(VideoRecordRequest, &mut App) + 'static,
    on_cancel: impl Fn(&mut App) + 'static,
) {
    // Run fast kernel-title window hierarchy discovery IN PARALLEL alongside the zero-copy WGC
    // monitor ring readback (~4–8ms total wall-clock across all monitors, 0ms extra hierarchy lag!).
    let mut freezer = ScreenFreezer::with_cursor(include_cursor && !start_in_video_mode);
    let (hierarchy, parallel_res) = std::thread::scope(|s| {
        let hier_job = s.spawn(|| enumerate_window_hierarchy_fast(4));
        let freeze_res = freezer.freeze_all_monitors_bgra_parallel();
        let hier = hier_job.join().unwrap_or_default();
        (hier, freeze_res)
    });

    let (frozen_monitors, frames_are_bgra): (Vec<(Option<isize>, FrozenScreen)>, bool) =
        match parallel_res {
            Ok(pairs) if !pairs.is_empty() => (
                pairs
                    .into_iter()
                    .map(|(mon, screen)| (Some(mon.hmonitor), screen))
                    .collect(),
                true,
            ),
            _ => match freezer.freeze_virtual_desktop() {
                Ok(f) => (vec![(None, f)], false),
                Err(_) => {
                    on_cancel(cx);
                    return;
                }
            },
        };

    open_overlay_windows_internal(
        cx,
        frozen_monitors,
        Some(hierarchy),
        frames_are_bgra,
        include_cursor,
        start_in_video_mode,
        on_captured,
        on_record_video,
        on_cancel,
    );
}

/// Open one borderless topmost overlay window per `(Option<HMONITOR>, FrozenScreen)`
/// (each positioned at its exact physical monitor rectangle via Win32 `SetWindowPos`),
/// sharing a unified close handle list.
pub fn open_overlay_windows_for_screens(
    cx: &mut App,
    screens: Vec<(Option<isize>, FrozenScreen)>,
    include_cursor: bool,
    start_in_video_mode: bool,
    on_captured: impl Fn(TiledBitmap, Option<CaptureObject>, &mut App) + 'static,
    on_record_video: impl Fn(VideoRecordRequest, &mut App) + 'static,
) {
    open_overlay_windows_for_screens_with_cancel(
        cx,
        screens,
        include_cursor,
        start_in_video_mode,
        on_captured,
        on_record_video,
        |_| {},
    );
}

/// Open one borderless topmost overlay window per `(Option<HMONITOR>, FrozenScreen)` (`RGBA8`) with an
/// explicit `on_cancel` handler so hidden caller windows can be restored on `Esc`/`Cancel`.
pub fn open_overlay_windows_for_screens_with_cancel(
    cx: &mut App,
    screens: Vec<(Option<isize>, FrozenScreen)>,
    include_cursor: bool,
    start_in_video_mode: bool,
    on_captured: impl Fn(TiledBitmap, Option<CaptureObject>, &mut App) + 'static,
    on_record_video: impl Fn(VideoRecordRequest, &mut App) + 'static,
    on_cancel: impl Fn(&mut App) + 'static,
) {
    open_overlay_windows_internal(
        cx,
        screens,
        None,
        false,
        include_cursor,
        start_in_video_mode,
        on_captured,
        on_record_video,
        on_cancel,
    );
}

fn open_overlay_windows_internal(
    cx: &mut App,
    screens: Vec<(Option<isize>, FrozenScreen)>,
    pre_hierarchy: Option<Vec<WindowHierarchyEntry>>,
    frames_are_bgra: bool,
    include_cursor: bool,
    start_in_video_mode: bool,
    on_captured: impl Fn(TiledBitmap, Option<CaptureObject>, &mut App) + 'static,
    on_record_video: impl Fn(VideoRecordRequest, &mut App) + 'static,
    on_cancel: impl Fn(&mut App) + 'static,
) {
    if screens.is_empty() {
        on_cancel(cx);
        return;
    }

    let hierarchy = pre_hierarchy.unwrap_or_else(|| enumerate_window_hierarchy_fast(4));
    let windows: Vec<WindowInfo> = hierarchy.iter().map(|e| e.top_window.clone()).collect();
    let gpui_displays = cx.displays();
    let cursor_pos = snagsoup_ui_huds::get_cursor_physical_pos();
    let on_captured = Rc::new(on_captured);
    let on_record_video = Rc::new(on_record_video);
    let on_cancel = Rc::new(on_cancel);
    let all_handles: Rc<RefCell<Vec<AnyWindowHandle>>> = Rc::new(RefCell::new(Vec::new()));
    let all_hwnds: Rc<RefCell<Vec<isize>>> = Rc::new(RefCell::new(Vec::new()));
    let active_mon_cell: Rc<std::cell::Cell<usize>> = Rc::new(std::cell::Cell::new(0));

    // Order `screens` so the monitor currently containing the mouse cursor opens & presents FIRST!
    let mut indexed_screens: Vec<(usize, Option<isize>, FrozenScreen, bool)> = screens
        .into_iter()
        .enumerate()
        .map(|(orig_idx, (hm, fs))| {
            let b = fs.frame.bounds;
            let sw = fs.frame.width;
            let sh = fs.frame.height;
            let has_cur = cursor_pos.map_or(orig_idx == 0, |(cx_p, cy_p)| {
                cx_p >= b.left
                    && cx_p < b.left + sw as i32
                    && cy_p >= b.top
                    && cy_p < b.top + sh as i32
            });
            (orig_idx, hm, fs, has_cur)
        })
        .collect();
    indexed_screens.sort_by_key(|&(_, _, _, has_cur)| !has_cur);

    for (mon_idx, _orig_idx, hmonitor, frozen, has_cursor) in indexed_screens
        .into_iter()
        .enumerate()
        .map(|(idx, (o, hm, fs, hc))| (idx, o, hm, fs, hc))
    {
        let bounds = frozen.frame.bounds;
        let mon_left = bounds.left;
        let mon_top = bounds.top;
        let sw = frozen.frame.width;
        let sh = frozen.frame.height;
        let captured_cursor = frozen.cursor;
        if has_cursor {
            active_mon_cell.set(mon_idx);
        }

        // Match this monitor against `frozen.topology.monitors` and `cx.displays()` so GPUI
        // creates the `PopUp` HWND directly on this specific monitor at its native Per-Monitor DPI!
        let topo_mon = hmonitor
            .and_then(|hm| frozen.topology.monitors.iter().find(|m| m.hmonitor == hm))
            .or_else(|| {
                frozen.topology.monitors.iter().find(|m| {
                    m.physical_rect.left == mon_left
                        && m.physical_rect.top == mon_top
                        && m.physical_rect.width() == sw as i32
                        && m.physical_rect.height() == sh as i32
                })
            });
        let effective_hmonitor = hmonitor.or_else(|| topo_mon.map(|m| m.hmonitor));
        let mon_scale = topo_mon.map(|m| m.scale_factor.max(0.5)).unwrap_or(1.0);

        let matched_display = effective_hmonitor
            .and_then(|hm| {
                gpui_displays
                    .iter()
                    .find(|d| u64::from(d.id()) == (hm as usize as u64))
            })
            .or_else(|| {
                let expected_origin_x = mon_left as f32 / mon_scale;
                let expected_origin_y = mon_top as f32 / mon_scale;
                let expected_w = sw as f32 / mon_scale;
                let expected_h = sh as f32 / mon_scale;
                gpui_displays.iter().find(|d| {
                    let b = d.bounds();
                    (f32::from(b.origin.x) - expected_origin_x).abs() < 2.0
                        && (f32::from(b.origin.y) - expected_origin_y).abs() < 2.0
                        && (f32::from(b.size.width) - expected_w).abs() < 2.0
                        && (f32::from(b.size.height) - expected_h).abs() < 2.0
                })
            });

        let matched_display_id = matched_display.map(|d| d.id());
        let init_bounds = if let Some(d) = matched_display {
            d.bounds()
        } else {
            Bounds::new(
                gpui::point(
                    px(mon_left as f32 / mon_scale),
                    px(mon_top as f32 / mon_scale),
                ),
                size(px(sw as f32 / mon_scale), px(sh as f32 / mon_scale)),
            )
        };

        // ZERO-COPY move of BGRA8 Vec<u8> straight into Arc<RenderImage>!
        let render_image_opt = if frames_are_bgra {
            bgra_vec_to_render_image(sw, sh, frozen.frame.rgba)
        } else {
            rgba_to_bgra_render_image(sw, sh, &frozen.frame.rgba)
        };

        let on_cap = Rc::clone(&on_captured);
        let on_vid = Rc::clone(&on_record_video);
        let on_can = Rc::clone(&on_cancel);
        let handles_for_view = Rc::clone(&all_handles);
        let hwnds_for_view = Rc::clone(&all_hwnds);
        let active_mon_for_view = Rc::clone(&active_mon_cell);
        let wins_clone = windows.clone();
        let hierarchy_clone = hierarchy.clone();

        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(init_bounds)),
                display_id: matched_display_id,
                titlebar: None,
                kind: WindowKind::PopUp,
                focus: has_cursor,
                is_resizable: false,
                is_movable: false,
                ..Default::default()
            },
            move |window, cx| {
                // Snap popup window onto exact physical monitor rectangle (100% physical coverage including taskbar, zero WM_DPICHANGED rescaling)
                if let Some(hwnd) = extract_hwnd(window) {
                    hwnds_for_view.borrow_mut().push(hwnd);
                    position_window_physical(
                        hwnd,
                        mon_left,
                        mon_top,
                        sw as i32,
                        sh as i32,
                        true,
                        has_cursor,
                    );
                }

                let render_for_crop = render_image_opt.clone();
                let cursor_for_crop = captured_cursor.clone();
                let on_cap_img = Rc::clone(&on_cap);
                let on_cap_scroll = Rc::clone(&on_cap);
                let on_vid_cb = Rc::clone(&on_vid);
                let on_cancel_cb = Rc::clone(&on_can);
                let handles_for_close = Rc::clone(&handles_for_view);
                let hwnds_for_close = Rc::clone(&hwnds_for_view);

                cx.new(move |cx| {
                    let focus_handle = cx.focus_handle();
                    if has_cursor {
                        focus_handle.focus(window, cx);
                    }
                    let frozen_cursor_overlay = build_frozen_cursor_overlay(
                        captured_cursor.as_ref(),
                        mon_left,
                        mon_top,
                        sw,
                        sh,
                        include_cursor && !start_in_video_mode,
                    );
                    let mut view = SelectionOverlayView::new_from_render_image(
                        sw,
                        sh,
                        render_image_opt,
                        wins_clone,
                        move |rect, _window, cx| {
                            let (bmp, cursor_obj) = if let Some(ref r_img) = render_for_crop {
                                crop_from_bgra_render_image(
                                    r_img,
                                    sw,
                                    sh,
                                    bounds,
                                    cursor_for_crop.as_ref(),
                                    rect,
                                    include_cursor,
                                )
                            } else {
                                (
                                    TiledBitmap::new_filled(
                                        rect.width.max(1.0) as u32,
                                        rect.height.max(1.0) as u32,
                                        ColorRgba::WHITE,
                                    ),
                                    None,
                                )
                            };
                            on_cap_img(bmp, cursor_obj, cx);
                        },
                    );
                    view.frozen_cursor_overlay = frozen_cursor_overlay;
                    view.hierarchy = hierarchy_clone;
                    view.state.monitor_left = mon_left;
                    view.state.monitor_top = mon_top;
                    view.state.start_in_video_mode = start_in_video_mode;
                    view.state.cursor_active = has_cursor;
                    view.monitor_idx = mon_idx;
                    view.active_monitor_cell = Some(active_mon_for_view);

                    // Immediately snap cursor & auto-detect window/sub-pane under mouse on Frame 0 (like Snagit!)
                    if has_cursor {
                        if let Some((gx, gy)) = cursor_pos {
                            let lx = (gx - mon_left).clamp(0, sw.saturating_sub(1) as i32);
                            let ly = (gy - mon_top).clamp(0, sh.saturating_sub(1) as i32);
                            view.state.cursor_x = lx;
                            view.state.cursor_y = ly;
                            view.update_autodetect_at_global(PhysicalPoint { x: gx, y: gy });
                        }
                    }

                    view.hmonitor = effective_hmonitor;
                    view.focus_handle = Some(focus_handle);
                    view.all_overlay_handles = Some(handles_for_close);
                    view.all_overlay_hwnds = Some(hwnds_for_close);
                    view.on_record_video = Some(on_vid_cb);
                    view.on_cancel = Some(on_cancel_cb);
                    view.on_scrolling_capture = Some(Rc::new(move |global_rect, cx| {
                        std::thread::sleep(Duration::from_millis(35));
                        if let Some(stitched) = capture_scrolling_region(global_rect) {
                            on_cap_scroll(stitched, None, cx);
                        }
                    }));
                    view
                })
            },
        );

        if let Ok(handle) = opened {
            all_handles.borrow_mut().push(handle.into());
        }
    }
}

fn build_frozen_cursor_overlay(
    cursor: Option<&snagsoup_win_platform::cursor::CapturedCursor>,
    mon_left: i32,
    mon_top: i32,
    sw: u32,
    sh: u32,
    include_cursor: bool,
) -> Option<(i32, i32, u32, u32, Arc<RenderImage>)> {
    if !include_cursor {
        return None;
    }
    let cur = cursor?;
    if !cur.intersects_rect(mon_left, mon_top, sw, sh) {
        return None;
    }
    let img = crate::rgba_to_bgra_render_image(cur.width, cur.height, &cur.rgba)?;
    Some((
        cur.top_left_x() - mon_left,
        cur.top_left_y() - mon_top,
        cur.width,
        cur.height,
        img,
    ))
}

/// GPUI view rendering the interactive screen selection overlay + Snagit Floating All-in-One Dialog.
pub struct SelectionOverlayView {
    pub state: SelectionOverlayState,
    pub frozen: Option<FrozenScreen>,
    pub windows: Vec<WindowInfo>,
    pub hierarchy: Vec<WindowHierarchyEntry>,
    frozen_render_image: Option<Arc<RenderImage>>,
    frozen_cursor_overlay: Option<(i32, i32, u32, u32, Arc<RenderImage>)>,
    hmonitor: Option<isize>,
    monitor_idx: usize,
    active_monitor_cell: Option<Rc<std::cell::Cell<usize>>>,
    focus_handle: Option<FocusHandle>,
    all_overlay_handles: Option<Rc<RefCell<Vec<AnyWindowHandle>>>>,
    all_overlay_hwnds: Option<Rc<RefCell<Vec<isize>>>>,
    on_complete: Option<Rc<dyn Fn(RectF, &mut Window, &mut App) + 'static>>,
    on_record_video: Option<Rc<dyn Fn(VideoRecordRequest, &mut App) + 'static>>,
    on_scrolling_capture: Option<Rc<dyn Fn(PhysicalRect, &mut App) + 'static>>,
    on_cancel: Option<Rc<dyn Fn(&mut App) + 'static>>,
}

impl SelectionOverlayView {
    pub fn new(screen_width: u32, screen_height: u32, frozen: Option<FrozenScreen>) -> Self {
        Self::new_interactive(screen_width, screen_height, frozen, Vec::new(), |_, _, _| {})
    }

    pub fn new_interactive(
        screen_width: u32,
        screen_height: u32,
        frozen: Option<FrozenScreen>,
        windows: Vec<WindowInfo>,
        on_complete: impl Fn(RectF, &mut Window, &mut App) + 'static,
    ) -> Self {
        let frozen_render_image = frozen
            .as_ref()
            .and_then(|f| rgba_to_bgra_render_image(f.frame.width, f.frame.height, &f.frame.rgba));
        Self {
            state: SelectionOverlayState::new(screen_width, screen_height),
            frozen,
            windows,
            hierarchy: Vec::new(),
            frozen_render_image,
            frozen_cursor_overlay: None,
            hmonitor: None,
            monitor_idx: 0,
            active_monitor_cell: None,
            focus_handle: None,
            all_overlay_handles: None,
            all_overlay_hwnds: None,
            on_complete: Some(Rc::new(on_complete)),
            on_record_video: None,
            on_scrolling_capture: None,
            on_cancel: None,
        }
    }

    pub fn new_from_render_image(
        screen_width: u32,
        screen_height: u32,
        frozen_render_image: Option<Arc<RenderImage>>,
        windows: Vec<WindowInfo>,
        on_complete: impl Fn(RectF, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            state: SelectionOverlayState::new(screen_width, screen_height),
            frozen: None,
            windows,
            hierarchy: Vec::new(),
            frozen_render_image,
            frozen_cursor_overlay: None,
            hmonitor: None,
            monitor_idx: 0,
            active_monitor_cell: None,
            focus_handle: None,
            all_overlay_handles: None,
            all_overlay_hwnds: None,
            on_complete: Some(Rc::new(on_complete)),
            on_record_video: None,
            on_scrolling_capture: None,
            on_cancel: None,
        }
    }

    /// Resolve the Snagit auto-detect target (child sub-pane or top-level window) at `global_pt`
    /// using the pre-freeze `hierarchy` first, falling back to `self.windows` when `hierarchy` is empty.
    pub fn update_autodetect_at_global(&mut self, global_pt: PhysicalPoint) {
        let resolved: Option<(PhysicalRect, &WindowInfo)> = if !self.hierarchy.is_empty() {
            find_snagit_target_at(&self.hierarchy, global_pt)
        } else {
            find_topmost_window_at(&self.windows, global_pt).map(|w| (w.rect, w))
        };

        if let Some((target_rect, top_info)) = resolved {
            let local = RectF::new(
                (target_rect.left - self.state.monitor_left) as f64,
                (target_rect.top - self.state.monitor_top) as f64,
                target_rect.width() as f64,
                target_rect.height() as f64,
            );
            self.state.autodetect_rect = Some(self.state.clamp_rect(local));
            self.state.autodetect_title = if top_info.title.is_empty() {
                top_info.app_name.clone()
            } else {
                Some(top_info.title.clone())
            };
        } else {
            self.state.autodetect_rect = None;
            self.state.autodetect_title = None;
        }
    }

    /// Immediately hide and destroy EVERY monitor overlay window — including `window` (the active
    /// monitor's `Window`, which is currently mutably borrowed by GPUI during input callbacks!).
    fn dismiss_overlays(&self, window: &mut Window, cx: &mut App) {
        if let Some(hwnd) = extract_hwnd(window) {
            let _ = hide_window_hwnd(hwnd);
        }
        if let (Some(handles), Some(hwnds)) = (&self.all_overlay_handles, &self.all_overlay_hwnds) {
            close_all_overlays(handles, hwnds, cx);
        } else if let Some(ref handles) = self.all_overlay_handles {
            let empty = Rc::new(RefCell::new(Vec::new()));
            close_all_overlays(handles, &empty, cx);
        }
        // CRITICAL: `h.update(cx, ...)` skips the currently borrowed `window`, so we MUST always
        // call `window.remove_window()` directly on the active monitor's window too!
        window.remove_window();
    }

    fn cancel_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.phase = OverlayPhase::Selecting;
        let cb = self.on_cancel.clone();
        self.dismiss_overlays(window, cx);
        if let Some(cb) = cb {
            cb(cx);
        }
    }

    fn trigger_image_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rect) = self.state.confirmed.or(self.state.selection) {
            let cb = self.on_complete.clone();
            self.dismiss_overlays(window, cx);
            if let Some(cb) = cb {
                cb(rect, window, cx);
            }
        }
    }

    fn trigger_scrolling_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rect) = self.state.confirmed.or(self.state.selection) {
            let global_rect = self.state.to_global_physical_rect(rect);
            let cb = self.on_scrolling_capture.clone();
            self.dismiss_overlays(window, cx);
            if let Some(cb) = cb {
                cb(global_rect, cx);
            }
        }
    }

    fn start_video_countdown(&mut self, cx: &mut Context<Self>) {
        let Some(rect) = self.state.confirmed.or(self.state.selection) else {
            return;
        };
        let global_rect = self.state.to_global_physical_rect(rect);
        let hmonitor = self.hmonitor;
        let record_mic = self.state.record_mic;
        let record_system_audio = self.state.record_system_audio;
        let handles = self.all_overlay_handles.clone();
        let hwnds = self.all_overlay_hwnds.clone();
        let on_vid = self.on_record_video.clone();

        self.state.phase = OverlayPhase::Countdown(3);
        cx.notify();

        cx.spawn(async move |weak, cx| {
            for next in [2u8, 1u8] {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let still_counting = weak
                    .update(cx, |view, cx| {
                        if matches!(view.state.phase, OverlayPhase::Countdown(_)) {
                            view.state.phase = OverlayPhase::Countdown(next);
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !still_counting {
                    return;
                }
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let should_start = weak
                .update(cx, |view, _cx| {
                    matches!(view.state.phase, OverlayPhase::Countdown(_))
                })
                .unwrap_or(false);
            if !should_start {
                return;
            }
            let _ = cx.update(|cx| {
                let empty_hwnds = Rc::new(RefCell::new(Vec::new()));
                let hw = hwnds.as_ref().unwrap_or(&empty_hwnds);
                if let Some(ref h) = handles {
                    close_all_overlays(h, hw, cx);
                }
                if let Some(ref cb) = on_vid {
                    cb(
                        VideoRecordRequest {
                            global_rect,
                            hmonitor,
                            record_mic,
                            record_system_audio,
                        },
                        cx,
                    );
                }
            });
        })
        .detach();
    }
}

impl Render for SelectionOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::dark);

        let scale = window.scale_factor().max(0.5);
        let to_logical = |phys: f32| phys / scale;
        let mon_logical_w = to_logical(self.state.screen_width as f32);
        let mon_logical_h = to_logical(self.state.screen_height as f32);

        let is_active_mon = self
            .active_monitor_cell
            .as_ref()
            .map_or(self.state.cursor_active, |c| c.get() == self.monitor_idx);
        let phase = self.state.phase;
        let highlight = self
            .state
            .confirmed
            .or(self.state.selection)
            .or(if is_active_mon {
                self.state.autodetect_rect
            } else {
                None
            });
        let cx_logical = to_logical(self.state.cursor_x as f32);
        let cy_logical = to_logical(self.state.cursor_y as f32);

        let mut root = div()
            .relative()
            .size_full()
            .when_some(self.focus_handle.clone(), |el, fh| el.track_focus(&fh))
            .when(phase == OverlayPhase::Selecting, |el| el.cursor_crosshair())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                match ev.keystroke.key.as_str() {
                    "escape" => {
                        this.cancel_overlays(window, cx);
                    }
                    "enter" => match this.state.phase {
                        OverlayPhase::Selecting => {
                            if this.state.enter_post_selection().is_some() {
                                cx.notify();
                            }
                        }
                        OverlayPhase::PostSelectionToolbar => {
                            this.trigger_image_capture(window, cx);
                        }
                        OverlayPhase::VideoPreRecord => {
                            this.start_video_countdown(cx);
                        }
                        OverlayPhase::Countdown(_) => {}
                    },
                    "up" if this.state.phase == OverlayPhase::Selecting => {
                        this.state.nudge_cursor(0, -1);
                        cx.notify();
                    }
                    "down" if this.state.phase == OverlayPhase::Selecting => {
                        this.state.nudge_cursor(0, 1);
                        cx.notify();
                    }
                    "left" if this.state.phase == OverlayPhase::Selecting => {
                        this.state.nudge_cursor(-1, 0);
                        cx.notify();
                    }
                    "right" if this.state.phase == OverlayPhase::Selecting => {
                        this.state.nudge_cursor(1, 0);
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                let scale = window.scale_factor().max(0.5);
                let phys_x = ((f32::from(ev.position.x) * scale).round() as i32)
                    .clamp(0, this.state.screen_width.saturating_sub(1) as i32);
                let phys_y = ((f32::from(ev.position.y) * scale).round() as i32)
                    .clamp(0, this.state.screen_height.saturating_sub(1) as i32);

                if let Some(handle) = this.state.active_resize_handle {
                    if let Some((lx, ly)) = this.state.last_resize_pos {
                        let dx = phys_x - lx;
                        let dy = phys_y - ly;
                        if dx != 0 || dy != 0 {
                            this.state.apply_resize_handle_drag(handle, dx, dy);
                            this.state.last_resize_pos = Some((phys_x, phys_y));
                            cx.notify();
                        }
                    }
                    return;
                }

                if this.state.phase != OverlayPhase::Selecting {
                    return;
                }

                // Seamlessly switch active monitor focus/crosshair when cursor crosses monitor border
                if let Some(ref cell) = this.active_monitor_cell {
                    if cell.get() != this.monitor_idx {
                        cell.set(this.monitor_idx);
                        this.state.cursor_active = true;
                        if let Some(ref fh) = this.focus_handle {
                            if !fh.is_focused(window) {
                                fh.focus(window, cx);
                            }
                        }
                        if let Some(ref handles) = this.all_overlay_handles {
                            let siblings: Vec<AnyWindowHandle> = handles.borrow().clone();
                            for h in siblings {
                                let _ = h.update(cx, |_, sibling_win, _| sibling_win.refresh());
                            }
                        }
                    }
                }

                let dragging = this.state.on_pointer_move(phys_x, phys_y, ev.modifiers.shift);
                if !dragging && (!this.hierarchy.is_empty() || !this.windows.is_empty()) {
                    let global_pt = PhysicalPoint {
                        x: this.state.monitor_left + phys_x,
                        y: this.state.monitor_top + phys_y,
                    };
                    this.update_autodetect_at_global(global_pt);
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if this.state.phase != OverlayPhase::Selecting {
                        return;
                    }
                    let scale = window.scale_factor().max(0.5);
                    let phys_x = ((f32::from(ev.position.x) * scale).round() as i32)
                        .clamp(0, this.state.screen_width.saturating_sub(1) as i32);
                    let phys_y = ((f32::from(ev.position.y) * scale).round() as i32)
                        .clamp(0, this.state.screen_height.saturating_sub(1) as i32);
                    this.state.on_pointer_press(phys_x, phys_y, ev.modifiers.shift);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                    if this.state.active_resize_handle.is_some() {
                        this.state.active_resize_handle = None;
                        this.state.last_resize_pos = None;
                        cx.notify();
                        return;
                    }
                    if this.state.phase != OverlayPhase::Selecting {
                        return;
                    }
                    let scale = window.scale_factor().max(0.5);
                    let phys_x = ((f32::from(ev.position.x) * scale).round() as i32)
                        .clamp(0, this.state.screen_width.saturating_sub(1) as i32);
                    let phys_y = ((f32::from(ev.position.y) * scale).round() as i32)
                        .clamp(0, this.state.screen_height.saturating_sub(1) as i32);

                    // Click (< 6px) confirms hovered auto-detected window; drag (>= 6px) confirms dragged region!
                    if this.state.on_pointer_release(phys_x, phys_y).is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _ev: &MouseDownEvent, window, cx| {
                    this.cancel_overlays(window, cx);
                }),
            )
            // Frozen desktop background image (BGRA8, ObjectFit::Fill -> ZERO gray letterbox bars!)
            .when_some(self.frozen_render_image.clone(), |el, bg_img| {
                el.child(
                    img(bg_img)
                        .object_fit(ObjectFit::Fill)
                        .absolute()
                        .inset_0()
                        .w_full()
                        .h_full(),
                )
            })
            // Snapshotted mouse cursor preview rendered ONLY when `Capture Cursor` is toggled ON
            .when_some(
                self.frozen_cursor_overlay.clone(),
                |el, (cx_phys, cy_phys, cw_phys, ch_phys, cur_img)| {
                    el.child(
                        img(cur_img)
                            .object_fit(ObjectFit::Fill)
                            .absolute()
                            .left(px(to_logical(cx_phys as f32)))
                            .top(px(to_logical(cy_phys as f32)))
                            .w(px(to_logical(cw_phys as f32)))
                            .h(px(to_logical(ch_phys as f32))),
                    )
                },
            );

        // Dimmed veil: 4-band exterior cutout around `highlight` so hovered window / dragged region stays 100% bright!
        if let Some(sel) = highlight {
            let sx = to_logical(sel.x as f32).clamp(0.0, mon_logical_w);
            let sy = to_logical(sel.y as f32).clamp(0.0, mon_logical_h);
            let sw = to_logical(sel.width as f32).clamp(1.0, (mon_logical_w - sx).max(1.0));
            let sh = to_logical(sel.height as f32).clamp(1.0, (mon_logical_h - sy).max(1.0));
            let veil = if phase == OverlayPhase::Selecting {
                rgba(0x00000044)
            } else {
                rgba(0x00000077)
            };
            // Top band
            root = root
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(mon_logical_w))
                        .h(px(sy))
                        .bg(veil),
                )
                // Bottom band
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(sy + sh))
                        .w(px(mon_logical_w))
                        .h(px((mon_logical_h - (sy + sh)).max(0.0)))
                        .bg(veil),
                )
                // Left band
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(sy))
                        .w(px(sx))
                        .h(px(sh))
                        .bg(veil),
                )
                // Right band
                .child(
                    div()
                        .absolute()
                        .left(px(sx + sw))
                        .top(px(sy))
                        .w(px((mon_logical_w - (sx + sw)).max(0.0)))
                        .h(px(sh))
                        .bg(veil),
                );
        } else if phase == OverlayPhase::Selecting {
            root = root.child(div().absolute().inset_0().bg(rgba(0x00000038)));
        }

        // Active selection / auto-detect border + 8 draggable resize handles (in PostSelection/VideoPreRecord)
        if let Some(rect) = highlight {
            let rx = to_logical(rect.x as f32);
            let ry = to_logical(rect.y as f32);
            let rw = to_logical(rect.width as f32);
            let rh = to_logical(rect.height as f32);
            let show_handles = matches!(
                phase,
                OverlayPhase::PostSelectionToolbar | OverlayPhase::VideoPreRecord
            );

            let mut frame_div = div()
                .absolute()
                .left(px(rx))
                .top(px(ry))
                .w(px(rw))
                .h(px(rh))
                .border_2()
                .border_dashed()
                .border_color(rgb(0xF5A623));

            if show_handles {
                let handles: &[(ResizeHandle, &str, f32, f32)] = &[
                    (ResizeHandle::TopLeft, "ov-h-tl", -5.0, -5.0),
                    (ResizeHandle::Top, "ov-h-t", rw * 0.5 - 5.0, -5.0),
                    (ResizeHandle::TopRight, "ov-h-tr", rw - 5.0, -5.0),
                    (ResizeHandle::Right, "ov-h-r", rw - 5.0, rh * 0.5 - 5.0),
                    (ResizeHandle::BottomRight, "ov-h-br", rw - 5.0, rh - 5.0),
                    (ResizeHandle::Bottom, "ov-h-b", rw * 0.5 - 5.0, rh - 5.0),
                    (ResizeHandle::BottomLeft, "ov-h-bl", -5.0, rh - 5.0),
                    (ResizeHandle::Left, "ov-h-l", -5.0, rh * 0.5 - 5.0),
                ];
                for &(h_kind, h_id, hx, hy) in handles {
                    frame_div = frame_div.child(
                        div()
                            .id(h_id)
                            .absolute()
                            .left(px(hx))
                            .top(px(hy))
                            .size(px(10.0))
                            .bg(rgba(0x111317BB))
                            .border_2()
                            .border_color(rgb(0xF5A623))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    let sc = window.scale_factor().max(0.5);
                                    let px_x = (f32::from(ev.position.x) * sc).round() as i32;
                                    let px_y = (f32::from(ev.position.y) * sc).round() as i32;
                                    this.state.active_resize_handle = Some(h_kind);
                                    this.state.last_resize_pos = Some((px_x, px_y));
                                    cx.notify();
                                }),
                            ),
                    );
                }
            }

            if let OverlayPhase::Countdown(n) = phase {
                frame_div = frame_div.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(110.0))
                                .rounded(px(55.0))
                                .bg(rgb(0xDC2626))
                                .border_2()
                                .border_color(rgb(0xFFFFFF))
                                .text_color(rgb(0xFFFFFF))
                                .text_size(px(54.0))
                                .shadow_lg()
                                .child(format!("{n}")),
                        )
                        .child(
                            div()
                                .id("countdown-cancel")
                                .px(px(14.0))
                                .py(px(6.0))
                                .rounded(px(16.0))
                                .bg(rgb(0x1E2228))
                                .border_1()
                                .border_color(rgb(0xFFFFFF))
                                .text_xs()
                                .text_color(rgb(0xFFFFFF))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.cancel_overlays(window, cx);
                                }))
                                .child("✕ Cancel"),
                        ),
                );
            }

            root = root.child(frame_div);
        }

        // Snagit-style top-center `Fullscreen` pill button on every monitor while Selecting
        if phase == OverlayPhase::Selecting {
            let pill_w = 118.0f32;
            let pill_left = ((mon_logical_w - pill_w) * 0.5).max(8.0);
            root = root.child(
                div()
                    .id("ov-fullscreen-pill")
                    .absolute()
                    .left(px(pill_left))
                    .top(px(12.0))
                    .w(px(pill_w))
                    .h(px(30.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .rounded(px(15.0))
                    .bg(rgba(0x2F3640DD))
                    .hover(|s| s.bg(rgb(0x00A4E4)))
                    .border_1()
                    .border_color(rgba(0xFFFFFF66))
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .cursor_pointer()
                    .shadow_md()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        if this.state.select_fullscreen().is_some() {
                            cx.notify();
                        }
                    }))
                    .child("⛶ Fullscreen"),
            );
        }

        // Crosshair + 11×11 Magnifier Loupe ONLY on the monitor currently under the cursor during Selecting phase
        if phase == OverlayPhase::Selecting && is_active_mon {
            let loupe = if self.frozen.is_some() {
                self.state.sample_magnifier(self.frozen.as_ref())
            } else {
                let bgra = self
                    .frozen_render_image
                    .as_ref()
                    .and_then(|img| img.as_bytes(0));
                self.state.sample_magnifier_bgra_bytes(bgra)
            };
            let mut grid_rows = div()
                .flex()
                .flex_col()
                .border_1()
                .border_color(theme.border_strong);
            for row in 0..loupe.grid_size {
                let mut row_el = div().flex().flex_row();
                for col in 0..loupe.grid_size {
                    let is_center = row == loupe.grid_size / 2 && col == loupe.grid_size / 2;
                    let color_hex = loupe.pixels[row * loupe.grid_size + col];
                    row_el = row_el.child(
                        div()
                            .size(px(10.0))
                            .bg(rgb(color_hex))
                            .when(is_center, |el| el.border_1().border_color(rgb(0xFF9F1C))),
                    );
                }
                grid_rows = grid_rows.child(row_el);
            }

            let dim_label = highlight
                .map(|r| {
                    if self.state.selection.is_none() {
                        if let Some(ref title) = self.state.autodetect_title {
                            let short: String = title.chars().take(24).collect();
                            return format!("{short} · {:.0} × {:.0} px", r.width, r.height);
                        }
                    }
                    format!("{:.0} × {:.0} px", r.width, r.height)
                })
                .unwrap_or_else(|| "Click window or drag · Right-click cancel".to_string());

            let loupe_left = (cx_logical + 20.0).min((mon_logical_w - 190.0).max(8.0));
            let loupe_top = (cy_logical + 20.0).min((mon_logical_h - 210.0).max(8.0));

            root = root
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(cy_logical))
                        .w_full()
                        .h(px(1.0))
                        .bg(rgb(0xFFB703)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(cx_logical))
                        .top(px(0.0))
                        .w(px(1.0))
                        .h_full()
                        .bg(rgb(0xFFB703)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(loupe_left))
                        .top(px(loupe_top))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .p(px(8.0))
                        .bg(theme.tooltip_bg)
                        .border_1()
                        .border_color(theme.tooltip_border)
                        .rounded(theme.radius_md)
                        .shadow_lg()
                        .child(grid_rows)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .text_xs()
                                .text_color(theme.tooltip_text)
                                .child(format!(
                                    "({}, {})  {}",
                                    self.state.cursor_x,
                                    self.state.cursor_y,
                                    loupe.center_hex()
                                ))
                                .child(dim_label)
                                .when(
                                    self.state.shift_held
                                        || self.state.guideline != GuidelinePreset::Free,
                                    |el| el.child(format!("Guide: {}", self.state.guideline.label())),
                                ),
                        ),
                );
        }

        // Snagit 2022 Large-Icon Floating All-in-One Capture Dialog & Video Pre-Recording Toolbar
        if matches!(
            phase,
            OverlayPhase::PostSelectionToolbar | OverlayPhase::VideoPreRecord
        ) {
            if let Some(sel) = highlight {
                let rx = to_logical(sel.x as f32);
                let ry = to_logical(sel.y as f32);
                let rw = to_logical(sel.width as f32);
                let rh = to_logical(sel.height as f32);

                let bar_w = if phase == OverlayPhase::PostSelectionToolbar {
                    468.0f32
                } else {
                    548.0f32
                };
                let bar_h = 58.0f32;
                let centered_x = rx + (rw - bar_w) * 0.5;
                let bar_x = centered_x.clamp(8.0, (mon_logical_w - bar_w - 8.0).max(8.0));
                let below_y = ry + rh + 12.0;
                let bar_y = if below_y + bar_h <= mon_logical_h - 8.0 {
                    below_y
                } else if ry - bar_h - 12.0 >= 8.0 {
                    ry - bar_h - 12.0
                } else {
                    (mon_logical_h - bar_h - 12.0).max(8.0)
                };

                let w_str = format!("{:.0}", sel.width);
                let h_str = format!("{:.0}", sel.height);
                let white_icon: gpui::Hsla = rgb(0xFFFFFF).into();

                let dims_box = |txt: String| {
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_w(px(50.0))
                        .h(px(28.0))
                        .px(px(8.0))
                        .rounded(px(3.0))
                        .bg(rgb(0x25313C))
                        .border_1()
                        .border_color(rgb(0x5A7084))
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(txt)
                };

                let dims_group = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(4.0))
                    .child(dims_box(w_str))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xDCE4EC))
                            .child("×"),
                    )
                    .child(dims_box(h_str));

                let toolbar = if phase == OverlayPhase::PostSelectionToolbar {
                    div()
                        .id("snagit-floating-aio-toolbar")
                        .absolute()
                        .left(px(bar_x))
                        .top(px(bar_y))
                        .max_w(px((mon_logical_w - 16.0).max(240.0)))
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .px(px(8.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgb(0x3B4C5C))
                        .border_1()
                        .border_color(rgb(0x5C7388))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        // 1. Capture Image (Big Cyan Camera Button)
                        .child(
                            div()
                                .id("aio-btn-image")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(58.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0x00A4E4))
                                .hover(|s| s.bg(rgb(0x1BB4F2)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.trigger_image_capture(window, cx);
                                }))
                                .child(SnagIcon::CameraCapture.render(px(25.0), white_icon)),
                        )
                        // 2. Record Video (Big Cyan Camcorder Button)
                        .child(
                            div()
                                .id("aio-btn-video")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(58.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0x00A4E4))
                                .hover(|s| s.bg(rgb(0x1BB4F2)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.phase = OverlayPhase::VideoPreRecord;
                                    cx.notify();
                                }))
                                .child(SnagIcon::VideoCapture.render(px(25.0), white_icon)),
                        )
                        // 3. Panoramic / Scrolling Capture (Big Cyan 4-Arrow Camera Button)
                        .child(
                            div()
                                .id("aio-btn-scroll")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(58.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0x00A4E4))
                                .hover(|s| s.bg(rgb(0x1BB4F2)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.trigger_scrolling_capture(window, cx);
                                }))
                                .child(SnagIcon::PanoramicCapture.render(px(25.0), white_icon)),
                        )
                        // Center: [ W ] × [ H ]
                        .child(dims_group)
                        // 4. Redo Selection (Slate Circular Counter-Clockwise Arrow Button)
                        .child(
                            div()
                                .id("aio-btn-redo")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(54.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0x607385))
                                .hover(|s| s.bg(rgb(0x73889C)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.redo_selection();
                                    cx.notify();
                                }))
                                .child(SnagIcon::RedoCycle.render(px(22.0), white_icon)),
                        )
                        // 5. Cancel (Slate Bold Close X Button)
                        .child(
                            div()
                                .id("aio-btn-cancel")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(54.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0x607385))
                                .hover(|s| s.bg(rgb(0x73889C)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.cancel_overlays(window, cx);
                                }))
                                .child(SnagIcon::CloseBold.render(px(21.0), white_icon)),
                        )
                } else {
                    let mic_on = self.state.record_mic;
                    let sys_on = self.state.record_system_audio;
                    div()
                        .id("snagit-floating-video-toolbar")
                        .absolute()
                        .left(px(bar_x))
                        .top(px(bar_y))
                        .max_w(px((mon_logical_w - 16.0).max(240.0)))
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .px(px(8.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgb(0x3B4C5C))
                        .border_1()
                        .border_color(rgb(0x5C7388))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        // 1. Start Recording (Big Red Record Circle Button)
                        .child(
                            div()
                                .id("vid-btn-record")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(58.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0xE02424))
                                .hover(|s| s.bg(rgb(0xF03E3E)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.start_video_countdown(cx);
                                }))
                                .child(SnagIcon::RecordDot.render(px(24.0), white_icon)),
                        )
                        // 2. Toggle Microphone (Green ON / Slate OFF)
                        .child(
                            div()
                                .id("vid-btn-mic")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(48.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(if mic_on { rgb(0x16A34A) } else { rgb(0x607385) })
                                .hover(|s| s.opacity(0.9))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.record_mic = !this.state.record_mic;
                                    cx.notify();
                                }))
                                .child(
                                    (if mic_on {
                                        SnagIcon::MicOn
                                    } else {
                                        SnagIcon::MicOff
                                    })
                                    .render(px(21.0), white_icon),
                                ),
                        )
                        // 3. Toggle System Audio (Green ON / Slate OFF)
                        .child(
                            div()
                                .id("vid-btn-sys")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(48.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(if sys_on { rgb(0x16A34A) } else { rgb(0x607385) })
                                .hover(|s| s.opacity(0.9))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.record_system_audio = !this.state.record_system_audio;
                                    cx.notify();
                                }))
                                .child(
                                    (if sys_on {
                                        SnagIcon::SpeakerOn
                                    } else {
                                        SnagIcon::SpeakerOff
                                    })
                                    .render(px(21.0), white_icon),
                                ),
                        )
                        // 4. Switch back to Image Capture Mode
                        .child(
                            div()
                                .id("vid-btn-to-img")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(48.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0x00A4E4))
                                .hover(|s| s.bg(rgb(0x1BB4F2)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.phase = OverlayPhase::PostSelectionToolbar;
                                    cx.notify();
                                }))
                                .child(SnagIcon::CameraCapture.render(px(22.0), white_icon)),
                        )
                        // Center: [ W ] × [ H ]
                        .child(dims_group)
                        // 5. Redo Selection
                        .child(
                            div()
                                .id("vid-btn-redo")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(48.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0x607385))
                                .hover(|s| s.bg(rgb(0x73889C)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.redo_selection();
                                    cx.notify();
                                }))
                                .child(SnagIcon::RedoCycle.render(px(21.0), white_icon)),
                        )
                        // 6. Cancel
                        .child(
                            div()
                                .id("vid-btn-cancel")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(48.0))
                                .h(px(42.0))
                                .rounded(px(6.0))
                                .bg(rgb(0x607385))
                                .hover(|s| s.bg(rgb(0x73889C)))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.cancel_overlays(window, cx);
                                }))
                                .child(SnagIcon::CloseBold.render(px(20.0), white_icon)),
                        )
                };

                root = root.child(toolbar);
            }
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_win_platform::object_finder::enumerate_window_hierarchy;

    #[test]
    fn nudge_1px_and_guideline_aspect_lock() {
        let mut st = SelectionOverlayState::new(1920, 1080);
        assert_eq!((st.cursor_x, st.cursor_y), (960, 540));

        st.nudge_cursor(1, -1);
        assert_eq!((st.cursor_x, st.cursor_y), (961, 539));

        st.begin_drag();
        st.shift_held = true;
        st.nudge_cursor(160, 40);
        let rect = st.selection.expect("drag selection should exist");
        assert!((rect.width - 160.0).abs() < 1e-3);
        assert!((rect.height - 90.0).abs() < 1e-3); // locked to 16:9

        let confirmed = st.confirm_selection().unwrap();
        assert_eq!(confirmed, rect);
    }

    #[test]
    fn post_selection_toolbar_and_8_handle_resize() {
        let mut st = SelectionOverlayState::new_for_monitor(-1920, 0, 1920, 1080);
        st.selection = Some(RectF::new(100.0, 100.0, 400.0, 300.0));
        let sel = st.enter_post_selection().expect("should enter post-selection");
        assert_eq!(st.phase, OverlayPhase::PostSelectionToolbar);
        assert_eq!(sel, RectF::new(100.0, 100.0, 400.0, 300.0));

        st.apply_resize_handle_drag(ResizeHandle::BottomRight, 50, -20);
        let updated = st.confirmed.unwrap();
        assert_eq!(updated, RectF::new(100.0, 100.0, 450.0, 280.0));

        let global = st.to_global_physical_rect(updated);
        assert_eq!(global, PhysicalRect::new(-1820, 100, -1370, 380));

        st.redo_selection();
        assert_eq!(st.phase, OverlayPhase::Selecting);
        assert!(st.confirmed.is_none());
    }

    #[test]
    fn magnifier_samples_11x11_grid() {
        let st = SelectionOverlayState::new(100, 100);
        let sample = st.sample_magnifier(None);
        assert_eq!(sample.grid_size, 11);
        assert_eq!(sample.pixels.len(), 121);
        assert_eq!(sample.center_hex().len(), 7);
    }

    #[test]
    fn floating_aio_and_video_toolbar_buttons_state_transitions() {
        let mut st = SelectionOverlayState::new(1920, 1080);
        st.start_in_video_mode = true;
        st.selection = Some(RectF::new(50.0, 60.0, 800.0, 600.0));
        assert!(st.enter_post_selection().is_some());
        assert_eq!(st.phase, OverlayPhase::VideoPreRecord);

        // Toggle Mic & System Audio buttons
        assert!(st.record_mic);
        assert!(st.record_system_audio);
        st.record_mic = !st.record_mic;
        st.record_system_audio = !st.record_system_audio;
        assert!(!st.record_mic && !st.record_system_audio);

        // Switch from Video toolbar to Image toolbar and back
        st.phase = OverlayPhase::PostSelectionToolbar;
        assert_eq!(st.phase, OverlayPhase::PostSelectionToolbar);
        st.phase = OverlayPhase::VideoPreRecord;
        assert_eq!(st.phase, OverlayPhase::VideoPreRecord);

        // Countdown & cancel reset
        st.phase = OverlayPhase::Countdown(3);
        assert!(matches!(st.phase, OverlayPhase::Countdown(3)));
        st.redo_selection();
        assert_eq!(st.phase, OverlayPhase::Selecting);
    }

    #[test]
    fn single_click_selects_hovered_window_while_drag_selects_region() {
        let mut st = SelectionOverlayState::new(1920, 1080);
        let win_rect = RectF::new(200.0, 150.0, 900.0, 620.0);
        st.autodetect_rect = Some(win_rect);

        // 1. Slight 3px hand jitter during a single click MUST select the auto-detected window (NOT a 3x3 region!)
        st.on_pointer_press(400, 300, false);
        let _ = st.on_pointer_move(403, 302, false);
        assert!(st.selection.is_none());
        let picked = st.on_pointer_release(403, 302).unwrap();
        assert_eq!(picked, win_rect);
        assert_eq!(st.phase, OverlayPhase::PostSelectionToolbar);

        // 2. Redo, then drag >= 6px -> selects exact dragged custom region
        st.redo_selection();
        st.autodetect_rect = Some(win_rect);
        st.on_pointer_press(300, 250, false);
        let _ = st.on_pointer_move(650, 500, false);
        let dragged = st.on_pointer_release(650, 500).unwrap();
        assert_eq!(dragged, RectF::new(300.0, 250.0, 350.0, 250.0));
    }

    #[test]
    fn live_multi_monitor_freeze_and_hierarchy_preserve_1_to_1_physical_geometry() {
        let _ = snagsoup_win_platform::dpi::init_per_monitor_dpi_v2();
        let topology =
            snagsoup_win_platform::dpi::enumerate_monitors().expect("enumerate monitor topology");
        assert!(
            !topology.monitors.is_empty(),
            "at least one physical monitor must exist"
        );

        let mut freezer = ScreenFreezer::new();
        let frozen_screens = freezer
            .freeze_all_monitors_bgra_parallel()
            .expect("instant parallel BGRA freeze across all monitors");
        assert_eq!(
            frozen_screens.len(),
            topology.monitors.len(),
            "every physical monitor must get its own frozen frame"
        );

        let hierarchy = enumerate_window_hierarchy(4);
        for mon in &topology.monitors {
            let (frozen_mon, matched_frozen) = frozen_screens
                .iter()
                .find(|(m, _)| m.hmonitor == mon.hmonitor)
                .expect("frozen frame matching physical HMONITOR");

            // Frozen frame MUST be 1:1 physical pixels of its own monitor, never scaled by cursor monitor DPI
            assert_eq!(
                matched_frozen.frame.width as i32,
                mon.physical_rect.width(),
                "monitor {} frozen width must equal exact physical width",
                mon.device_name
            );
            assert_eq!(
                matched_frozen.frame.height as i32,
                mon.physical_rect.height(),
                "monitor {} frozen height must equal exact physical height",
                mon.device_name
            );
            assert_eq!(frozen_mon.dpi_x, mon.dpi_x);
            assert!(mon.scale_factor >= 0.5 && mon.scale_factor <= 4.0);

            // Probe Snagit target finder at monitor center & verify local clamp stays within monitor
            let cx = mon.physical_rect.left + mon.physical_rect.width() / 2;
            let cy = mon.physical_rect.top + mon.physical_rect.height() / 2;
            if let Some((hit_rect, _win)) =
                find_snagit_target_at(&hierarchy, PhysicalPoint { x: cx, y: cy })
            {
                let lx = (hit_rect.left - mon.physical_rect.left).max(0) as f32;
                let ly = (hit_rect.top - mon.physical_rect.top).max(0) as f32;
                let rx =
                    (hit_rect.right - mon.physical_rect.left).min(mon.physical_rect.width()) as f32;
                let ry = (hit_rect.bottom - mon.physical_rect.top).min(mon.physical_rect.height())
                    as f32;
                assert!(rx > lx && ry > ly);
            }
        }
    }

    #[test]
    fn captured_cursor_renders_real_bitmap_without_red_square_and_skips_outside_crop() {
        let bounds = PhysicalRect::new(0, 0, 200, 200);
        let mut cur_rgba = vec![0u8; 16 * 16 * 4];
        // Paint a solid emerald cursor pixel block at (2..6, 2..6) inside the 16x16 cursor
        for y in 2..6 {
            for x in 2..6 {
                let idx = (y * 16 + x) * 4;
                cur_rgba[idx..idx + 4].copy_from_slice(&[16, 185, 129, 255]);
            }
        }
        let cursor = snagsoup_win_platform::cursor::CapturedCursor {
            screen_x: 50,
            screen_y: 50,
            hotspot_x: 0,
            hotspot_y: 0,
            width: 16,
            height: 16,
            rgba: cur_rgba,
        };
        let frozen = FrozenScreen {
            frame: snagsoup_capture::CapturedFrame {
                bounds,
                width: 200,
                height: 200,
                rgba: vec![255u8; 200 * 200 * 4],
                backend: snagsoup_capture::CaptureBackend::GdiFallback,
                elapsed_ms: 1.0,
            },
            topology: snagsoup_win_platform::dpi::DisplayTopology {
                virtual_bounds: bounds,
                monitors: Vec::new(),
            },
            cursor: Some(cursor),
        };

        // 1. Cropping a region far from (50, 50) skips attaching an off-canvas cursor object
        let (_bmp_out, cur_outside) =
            crop_frozen_screen(&frozen, RectF::new(120.0, 120.0, 60.0, 60.0), true);
        assert!(cur_outside.is_none());
        assert!(build_frozen_cursor_overlay(frozen.cursor.as_ref(), 0, 0, 200, 200, false).is_none());
        let ov_on = build_frozen_cursor_overlay(frozen.cursor.as_ref(), 0, 0, 200, 200, true)
            .expect("frozen cursor overlay must be created when Capture Cursor is ON");
        assert_eq!((ov_on.0, ov_on.1, ov_on.2, ov_on.3), (50, 50, 16, 16));

        // 2. Cropping a region containing (50, 50) embeds the PNG cursor and renders real emerald pixels (zero #D92B2B red!)
        let (bmp_in, cur_inside) =
            crop_frozen_screen(&frozen, RectF::new(40.0, 40.0, 60.0, 60.0), true);
        let cur_obj = cur_inside.expect("cursor inside crop rect must be attached");
        assert!(cur_obj.image.as_deref().is_some_and(|s| !s.is_empty()));

        let page = snagsoup_model::SnagxPage {
            capture_canvas_width: Some(60.0),
            capture_canvas_height: Some(60.0),
            capture_background_color: Some(ColorRgba::WHITE),
            capture_objects: vec![cur_obj],
            ..Default::default()
        };
        let comp = snagsoup_canvas::SceneCompositor::new(&page, Some(&bmp_in));
        let (fw, fh, flat) = comp.flatten_to_rgba().expect("flatten succeeds");
        assert_eq!((fw, fh), (60, 60));

        // Cursor top-left is at (50 - 40, 50 - 40) = (10, 10), so emerald pixel is at (13, 13)
        let p_idx = (13 * 60 + 13) * 4;
        assert_eq!(&flat[p_idx..p_idx + 4], &[16, 185, 129, 255]);
        assert!(
            !flat.chunks_exact(4).any(|px| px[0] == 217 && px[1] == 43 && px[2] == 43),
            "flattened capture with cursor must never contain default #D92B2B red rectangle border"
        );
    }
}
