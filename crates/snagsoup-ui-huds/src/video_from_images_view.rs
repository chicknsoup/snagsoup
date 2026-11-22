//! Create Video from Images GPUI Studio (`W3-13`) wired to `snagsoup_media::VideoFromImagesSession`:
//! Slide strip navigation (Prev/Next), background color selector, Mic + Webcam toggles,
//! and live presenter annotation tools (`Cursor`, `Arrow`, `Rectangle`, `Pen`, `Highlighter`).

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div, px, rgb,
};
use snagsoup_media::{MediaError, VideoFromImagesSession, WebcamOverlayMode};
use std::path::PathBuf;

/// Live presenter annotation tool available during Video-from-Images recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenterTool {
    Cursor,
    Arrow,
    Rectangle,
    Pen,
    Highlighter,
}

/// A single still image slide in the Video-from-Images tray.
#[derive(Debug, Clone)]
pub struct SlideItem {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// GPUI Create Video from Images Studio view.
pub struct VideoFromImagesView {
    pub slides: Vec<SlideItem>,
    pub active_slide: usize,
    pub background_color: [u8; 4],
    pub record_mic: bool,
    pub record_webcam: bool,
    pub active_tool: PresenterTool,
    pub output_width: u32,
    pub output_height: u32,
    pub fps: u32,
    pub status_message: String,
    session: Option<VideoFromImagesSession>,
}

impl VideoFromImagesView {
    pub fn new(slides: Vec<SlideItem>, output_width: u32, output_height: u32, fps: u32) -> Self {
        Self {
            slides,
            active_slide: 0,
            background_color: [24, 24, 28, 255],
            record_mic: true,
            record_webcam: false,
            active_tool: PresenterTool::Cursor,
            output_width,
            output_height,
            fps: fps.max(1),
            status_message: "Ready to record slides to MP4".to_string(),
            session: None,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.session.is_some()
    }

    pub fn prev_slide(&mut self) {
        if self.active_slide > 0 {
            self.active_slide -= 1;
        }
    }

    pub fn next_slide(&mut self) {
        if self.active_slide + 1 < self.slides.len() {
            self.active_slide += 1;
        }
    }

    pub fn select_tool(&mut self, tool: PresenterTool) {
        self.active_tool = tool;
    }

    /// Start recording an MP4 session to `output_path`.
    pub fn start_recording(&mut self, output_path: impl Into<PathBuf>) -> Result<(), MediaError> {
        let mut sess = VideoFromImagesSession::start(
            output_path,
            self.output_width,
            self.output_height,
            self.fps,
            false,
        )?;
        sess.webcam_mode = if self.record_webcam {
            WebcamOverlayMode::FullscreenWebcam
        } else {
            WebcamOverlayMode::Hidden
        };
        self.session = Some(sess);
        Ok(())
    }

    /// Push the currently active slide (plus optional live drawn RGBA overlay) into the active MP4.
    pub fn record_current_slide(
        &mut self,
        overlay_rgba: Option<&[u8]>,
        repeat_frames: u32,
    ) -> Result<(), MediaError> {
        let Some(ref mut sess) = self.session else {
            return Err(MediaError::InvalidConfig("recording not started".into()));
        };
        let Some(slide) = self.slides.get(self.active_slide) else {
            return Err(MediaError::InvalidConfig("no active slide".into()));
        };
        sess.push_slide(
            &slide.rgba,
            slide.width,
            slide.height,
            overlay_rgba,
            None,
            repeat_frames,
            None,
        )
    }

    /// Stop and finalize the MP4 output.
    pub fn stop_recording(&mut self) -> Result<(), MediaError> {
        if let Some(sess) = self.session.take() {
            sess.finalize()?;
        }
        Ok(())
    }
}

impl Render for VideoFromImagesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let slide_label = self
            .slides
            .get(self.active_slide)
            .map(|s| format!("{} ({}/{})", s.title, self.active_slide + 1, self.slides.len()))
            .unwrap_or_else(|| "No slides".to_string());
        let recording = self.is_recording();
        let mic_on = self.record_mic;
        let cam_on = self.record_webcam;

        let tools: &[(PresenterTool, &str, &str)] = &[
            (PresenterTool::Cursor, "vfi-tool-cursor", "Cursor"),
            (PresenterTool::Arrow, "vfi-tool-arrow", "Arrow"),
            (PresenterTool::Rectangle, "vfi-tool-rect", "Rectangle"),
            (PresenterTool::Pen, "vfi-tool-pen", "Pen"),
            (PresenterTool::Highlighter, "vfi-tool-hi", "Highlighter"),
        ];
        let mut tool_row = div().flex().flex_row().gap(px(6.0));
        for &(tool, tid, label) in tools {
            let active = self.active_tool == tool;
            tool_row = tool_row.child(
                div()
                    .id(tid)
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(if active { rgb(0x2563EB) } else { rgb(0x374151) })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_tool(tool);
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        div()
            .id("video-from-images-studio")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(16.0))
            .bg(rgb(0x181B20))
            .text_color(rgb(0xF3F4F6))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(format!("Create Video from Images — {slide_label}"))
                    .child(format!("Active Tool: {:?}", self.active_tool)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("vfi-prev")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x374151))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prev_slide();
                                cx.notify();
                            }))
                            .child("◀ Prev Slide"),
                    )
                    .child(
                        div()
                            .id("vfi-next")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x374151))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.next_slide();
                                cx.notify();
                            }))
                            .child("Next Slide ▶"),
                    )
                    .child(
                        div()
                            .id("vfi-mic-toggle")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(if mic_on { rgb(0x2563EB) } else { rgb(0x374151) })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.record_mic = !this.record_mic;
                                cx.notify();
                            }))
                            .child(if mic_on { "🎤 Mic ON" } else { "🎤 Mic OFF" }),
                    )
                    .child(
                        div()
                            .id("vfi-cam-toggle")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(if cam_on { rgb(0x2563EB) } else { rgb(0x374151) })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.record_webcam = !this.record_webcam;
                                cx.notify();
                            }))
                            .child(if cam_on { "📷 Webcam ON" } else { "📷 Webcam OFF" }),
                    )
                    .child(
                        div()
                            .id("vfi-record-toggle")
                            .px(px(12.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(rgb(0xDC2626))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.is_recording() {
                                    let _ = this.record_current_slide(None, this.fps);
                                    match this.stop_recording() {
                                        Ok(()) => {
                                            this.status_message =
                                                "Finalized MP4 presentation!".to_string();
                                        }
                                        Err(e) => {
                                            this.status_message = format!("Finalize error: {e}");
                                        }
                                    }
                                } else {
                                    let base = std::env::var("LOCALAPPDATA")
                                        .map(PathBuf::from)
                                        .unwrap_or_else(|_| std::env::temp_dir())
                                        .join("SnagSoup")
                                        .join("Captures");
                                    let _ = std::fs::create_dir_all(&base);
                                    let ts = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    let out = base.join(format!("VFI-{ts}.mp4"));
                                    match this.start_recording(&out) {
                                        Ok(()) => {
                                            let _ = this.record_current_slide(None, this.fps);
                                            this.status_message =
                                                format!("Recording slides to {}", out.display());
                                        }
                                        Err(e) => {
                                            this.status_message = format!("Record start error: {e}");
                                        }
                                    }
                                }
                                cx.notify();
                            }))
                            .child(if recording {
                                "⏹ Stop & Save MP4"
                            } else {
                                "🔴 Start Recording MP4"
                            }),
                    ),
            )
            .child(tool_row)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x9CA3AF))
                    .child(self.status_message.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_media::probe_video_metadata;

    #[test]
    fn video_from_images_view_navigates_and_records_mp4() {
        let s1 = SlideItem {
            title: "Intro".into(),
            width: 160,
            height: 120,
            rgba: vec![200u8; 160 * 120 * 4],
        };
        let s2 = SlideItem {
            title: "Demo".into(),
            width: 160,
            height: 120,
            rgba: vec![80u8; 160 * 120 * 4],
        };
        let mut studio = VideoFromImagesView::new(vec![s1, s2], 160, 120, 15);
        let out = std::env::temp_dir().join(format!("snagsoup_w3_13_vfi_{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&out);

        studio.start_recording(&out).expect("start VFI recording");
        assert!(studio.is_recording());
        studio.select_tool(PresenterTool::Arrow);
        studio.record_current_slide(None, 15).expect("record slide 1");
        studio.next_slide();
        assert_eq!(studio.active_slide, 1);
        studio.select_tool(PresenterTool::Highlighter);
        studio.record_current_slide(None, 15).expect("record slide 2");
        studio.stop_recording().expect("stop VFI recording");
        assert!(!studio.is_recording());

        let meta = probe_video_metadata(&out).expect("probe VFI mp4");
        assert_eq!(meta.width, 160);
        assert_eq!(meta.height, 120);
        assert!(meta.duration_ms >= 1500);
        let _ = std::fs::remove_file(&out);
    }
}
