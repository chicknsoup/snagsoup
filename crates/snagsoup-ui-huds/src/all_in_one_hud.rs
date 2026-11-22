//! All-in-One Post-Selection floating HUD (`W3-04`):
//! Displays live `W x H` dimensions, supports 8-handle region fine-tuning, and offers
//! one-click *Capture Image*, *Record Video*, *Panoramic Capture*, and *Cancel* actions.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div, px, rgb,
};
use snagsoup_win_platform::dpi::PhysicalRect;

/// Action selected from the All-in-One Post-Selection HUD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllInOneAction {
    CaptureImage,
    RecordVideo,
    PanoramicCapture,
    Cancel,
}

/// 8-handle resize anchors around the selected region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

/// GPUI floating All-in-One Post-Selection HUD view.
pub struct AllInOneHudView {
    pub selection: PhysicalRect,
    pub last_action: Option<AllInOneAction>,
}

impl AllInOneHudView {
    pub fn new(selection: PhysicalRect) -> Self {
        Self {
            selection,
            last_action: None,
        }
    }

    /// Fine-tune `selection` by dragging one of the 8 outer handles by `(dx, dy)` pixels
    /// while preserving a minimum `16x16` region size.
    pub fn apply_handle_drag(&mut self, handle: ResizeHandle, dx: i32, dy: i32) {
        const MIN_DIM: i32 = 16;
        let mut r = self.selection;
        match handle {
            ResizeHandle::TopLeft => {
                r.left = (r.left + dx).min(r.right - MIN_DIM);
                r.top = (r.top + dy).min(r.bottom - MIN_DIM);
            }
            ResizeHandle::Top => {
                r.top = (r.top + dy).min(r.bottom - MIN_DIM);
            }
            ResizeHandle::TopRight => {
                r.right = (r.right + dx).max(r.left + MIN_DIM);
                r.top = (r.top + dy).min(r.bottom - MIN_DIM);
            }
            ResizeHandle::Right => {
                r.right = (r.right + dx).max(r.left + MIN_DIM);
            }
            ResizeHandle::BottomRight => {
                r.right = (r.right + dx).max(r.left + MIN_DIM);
                r.bottom = (r.bottom + dy).max(r.top + MIN_DIM);
            }
            ResizeHandle::Bottom => {
                r.bottom = (r.bottom + dy).max(r.top + MIN_DIM);
            }
            ResizeHandle::BottomLeft => {
                r.left = (r.left + dx).min(r.right - MIN_DIM);
                r.bottom = (r.bottom + dy).max(r.top + MIN_DIM);
            }
            ResizeHandle::Left => {
                r.left = (r.left + dx).min(r.right - MIN_DIM);
            }
        }
        self.selection = r;
    }

    /// Set explicit width/height from the HUD dimension inputs.
    pub fn set_dimensions(&mut self, width: i32, height: i32) {
        self.selection.right = self.selection.left + width.max(16);
        self.selection.bottom = self.selection.top + height.max(16);
    }

    pub fn trigger(&mut self, action: AllInOneAction) {
        self.last_action = Some(action);
    }
}

impl Render for AllInOneHudView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let w = self.selection.width().max(0);
        let h = self.selection.height().max(0);

        div()
            .id("all-in-one-hud")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(rgb(0x1E2228))
            .text_color(rgb(0xF3F4F6))
            .child(
                div()
                    .id("aio-capture-image")
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(5.0))
                    .bg(rgb(0x2563EB))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.trigger(AllInOneAction::CaptureImage);
                        cx.notify();
                    }))
                    .child("Capture Image"),
            )
            .child(
                div()
                    .id("aio-record-video")
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(5.0))
                    .bg(rgb(0xDC2626))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.trigger(AllInOneAction::RecordVideo);
                        cx.notify();
                    }))
                    .child("Record Video"),
            )
            .child(
                div()
                    .id("aio-panoramic")
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(5.0))
                    .bg(rgb(0x0D9488))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.trigger(AllInOneAction::PanoramicCapture);
                        cx.notify();
                    }))
                    .child("Panoramic"),
            )
            .child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(rgb(0x111317))
                    .text_color(rgb(0xD1D5DB))
                    .child(format!("{w} x {h} px")),
            )
            .child(
                div()
                    .id("aio-cancel")
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(5.0))
                    .bg(rgb(0x374151))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.trigger(AllInOneAction::Cancel);
                        cx.notify();
                    }))
                    .child("Cancel"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_in_one_hud_8_handle_resize_and_actions() {
        let mut hud = AllInOneHudView::new(PhysicalRect::new(100, 100, 500, 400));
        hud.apply_handle_drag(ResizeHandle::BottomRight, 50, -40);
        assert_eq!(hud.selection, PhysicalRect::new(100, 100, 550, 360));

        hud.apply_handle_drag(ResizeHandle::TopLeft, -20, 10);
        assert_eq!(hud.selection, PhysicalRect::new(80, 110, 550, 360));

        hud.set_dimensions(1280, 720);
        assert_eq!(hud.selection.width(), 1280);
        assert_eq!(hud.selection.height(), 720);

        hud.trigger(AllInOneAction::RecordVideo);
        assert_eq!(hud.last_action, Some(AllInOneAction::RecordVideo));
    }
}
