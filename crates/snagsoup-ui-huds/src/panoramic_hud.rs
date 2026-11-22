//! Panoramic Live Preview HUD (`W3-04`) wired directly to `snagsoup_stitch::PanoramicSession`:
//! Start/Stop controls, live `<= 320x180` stitched preview state, and Green/Amber/Red
//! scroll-speed quality indicator.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div, px, rgb,
};
use snagsoup_stitch::{PanoramicSession, PushOutcome, RgbaFrame};

/// Real-time feedback indicator for user scroll speed during Panoramic capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollSpeedStatus {
    /// Smooth overlap, crisp 2D FFT correlation.
    Optimal,
    /// Large frame-to-frame displacement; approaching blur risk.
    Fast,
    /// Motion blur or insufficient overlap — frame rejected; slow down!
    TooFast,
}

/// GPUI Panoramic Live Preview HUD view.
pub struct PanoramicHudView {
    pub session: PanoramicSession,
    pub is_capturing: bool,
    pub speed_status: ScrollSpeedStatus,
    pub stitched_frames: usize,
    pub last_preview: Option<RgbaFrame>,
}

impl Default for PanoramicHudView {
    fn default() -> Self {
        Self::new()
    }
}

impl PanoramicHudView {
    pub fn new() -> Self {
        Self {
            session: PanoramicSession::new(),
            is_capturing: false,
            speed_status: ScrollSpeedStatus::Optimal,
            stitched_frames: 0,
            last_preview: None,
        }
    }

    pub fn start(&mut self) {
        self.session = PanoramicSession::new();
        self.is_capturing = true;
        self.speed_status = ScrollSpeedStatus::Optimal;
        self.stitched_frames = 0;
        self.last_preview = None;
    }

    /// Push a captured viewport frame into the live `PanoramicSession`, updating
    /// `speed_status` and the `<= 320x180` preview image.
    pub fn push_live_frame(&mut self, frame: RgbaFrame) -> Option<PushOutcome> {
        if !self.is_capturing {
            return None;
        }
        let max_dim = frame.width.max(frame.height) as i32;
        let outcome = self.session.push_frame(frame).ok()?;
        match outcome {
            PushOutcome::Initialized => {
                self.stitched_frames = 1;
                self.speed_status = ScrollSpeedStatus::Optimal;
                self.last_preview = self.session.preview_image(320, 180);
            }
            PushOutcome::Stitched { dx, dy, .. } => {
                self.stitched_frames += 1;
                let shift = dx.abs().max(dy.abs());
                self.speed_status = if shift * 100 > max_dim * 65 {
                    ScrollSpeedStatus::Fast
                } else {
                    ScrollSpeedStatus::Optimal
                };
                self.last_preview = self.session.preview_image(320, 180);
            }
            PushOutcome::Stationary => {}
            PushOutcome::RejectedLowConfidence => {
                self.speed_status = ScrollSpeedStatus::TooFast;
            }
        }
        Some(outcome)
    }

    /// Stop panoramic capture and materialize the final stitched RGBA8 image.
    pub fn stop(&mut self) -> Option<RgbaFrame> {
        self.is_capturing = false;
        self.session.finalize().ok()
    }
}

impl Render for PanoramicHudView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (tw, th) = self.session.dimensions();
        let (status_label, status_color) = match self.speed_status {
            ScrollSpeedStatus::Optimal => ("Speed: Optimal", rgb(0x16A34A)),
            ScrollSpeedStatus::Fast => ("Speed: Slow Down Slightly", rgb(0xD97706)),
            ScrollSpeedStatus::TooFast => ("Too Fast — Back Up Slightly!", rgb(0xDC2626)),
        };

        div()
            .id("panoramic-hud")
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(px(8.0))
            .bg(rgb(0x1E2228))
            .text_color(rgb(0xF3F4F6))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(format!("Panoramic ({tw} x {th} px)"))
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .bg(status_color)
                            .child(status_label),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("pano-start-stop")
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(5.0))
                            .bg(if self.is_capturing {
                                rgb(0xDC2626)
                            } else {
                                rgb(0x16A34A)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.is_capturing {
                                    let _ = this.stop();
                                } else {
                                    this.start();
                                }
                                cx.notify();
                            }))
                            .child(if self.is_capturing { "Stop" } else { "Start" }),
                    )
                    .child(format!("Frames: {}", self.stitched_frames)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_patch(ox: i32, oy: i32, w: u32, h: u32) -> RgbaFrame {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            let wy = (oy + y as i32) as u32;
            for x in 0..w {
                let wx = (ox + x as i32) as u32;
                let hash = (wx / 16)
                    .wrapping_mul(374_761_393)
                    .wrapping_add((wy / 16).wrapping_mul(668_265_263))
                    ^ (wx.wrapping_mul(17) ^ wy.wrapping_mul(29));
                let idx = ((y * w + x) * 4) as usize;
                rgba[idx] = (hash & 0xFF) as u8;
                rgba[idx + 1] = ((hash >> 8) & 0xFF) as u8;
                rgba[idx + 2] = ((hash >> 16) & 0xFF) as u8;
                rgba[idx + 3] = 255;
            }
        }
        RgbaFrame::new(w, h, rgba).unwrap()
    }

    #[test]
    fn panoramic_hud_lifecycle_and_preview_generation() {
        let mut hud = PanoramicHudView::new();
        hud.start();
        assert!(hud.is_capturing);

        hud.push_live_frame(test_patch(0, 0, 200, 200));
        hud.push_live_frame(test_patch(0, 80, 200, 200));
        assert_eq!(hud.stitched_frames, 2);
        assert_eq!(hud.speed_status, ScrollSpeedStatus::Optimal);
        let preview = hud.last_preview.as_ref().expect("preview should exist");
        assert!(preview.width <= 320 && preview.height <= 180);

        let final_img = hud.stop().expect("finalize should produce image");
        assert_eq!(final_img.width, 200);
        assert_eq!(final_img.height, 280);
        assert!(!hud.is_capturing);
    }
}
