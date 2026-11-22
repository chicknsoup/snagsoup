//! Live Screen Recording Controller & Floating Mini Recording HUD Window (`W3-04` + Wave 6):
//! - Starts `snagsoup_media::LiveScreenRecorder` for the selected physical screen rect
//! - Opens a topmost borderless Mini Recording HUD (`~390 × 56 px`) with `WDA_EXCLUDEFROMCAPTURE`
//!   so the HUD never appears in the recorded MP4
//! - Displays pulsing red `● REC`, live `MM:SS` timer, live Mic/Sys audio level bar,
//!   `Pause / Resume`, `Stop`, and `Cancel`
//! - On `Stop`: finalizes the `.mp4` via `LiveScreenRecorder::stop()`, indexes it in `LibraryDb`,
//!   closes the Mini HUD, and opens `EditorShellView` displaying the recorded `.mp4`!

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    App, AppContext, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, div, px, rgb, size,
};
use snagsoup_library_export::{LibraryDb, LibraryItem, MediaType};
use snagsoup_media::{LiveRecordConfig, LiveScreenRecorder};
use snagsoup_ui_huds::{
    extract_hwnd, format_mm_ss, hide_window_hwnd, position_window_physical,
    set_window_exclude_from_capture,
};

use crate::capture_shell::overlay::VideoRecordRequest;
use crate::editor_shell::shell::{capture_storage_dir, library_db_path};

/// Floating Mini Recording HUD GPUI view (`WDA_EXCLUDEFROMCAPTURE` + `HWND_TOPMOST`).
pub struct LiveRecordingHudView {
    pub request: VideoRecordRequest,
    pub output_path: PathBuf,
    pub recorder: Option<LiveScreenRecorder>,
    pub paused: bool,
    pub pulse_on: bool,
    pub status_error: Option<String>,
}

impl LiveRecordingHudView {
    pub fn new(request: VideoRecordRequest, output_path: PathBuf) -> Self {
        let cfg = LiveRecordConfig {
            region: request.global_rect,
            hmonitor: request.hmonitor,
            fps: 30,
            record_mic: request.record_mic,
            record_system_audio: request.record_system_audio,
            record_cursor: true,
            output_mp4: output_path.clone(),
        };

        let (recorder, status_error) = match LiveScreenRecorder::start(cfg) {
            Ok(rec) => (Some(rec), None),
            Err(err) => (None, Some(err.to_string())),
        };

        Self {
            request,
            output_path,
            recorder,
            paused: false,
            pulse_on: true,
            status_error,
        }
    }

    pub fn elapsed_mm_ss(&self) -> String {
        let ms = self.recorder.as_ref().map_or(0, |r| r.elapsed_ms());
        format_mm_ss(Duration::from_millis(ms))
    }

    fn current_audio_level(&self) -> f32 {
        self.recorder
            .as_ref()
            .map_or(0.0, |r| r.mic_level().max(r.system_audio_level()))
    }

    pub fn toggle_pause(&mut self) {
        if let Some(ref rec) = self.recorder {
            if self.paused {
                rec.resume();
                self.paused = false;
            } else {
                rec.pause();
                self.paused = true;
            }
        }
    }

    pub fn finalize_and_open_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let finalized_path = if let Some(rec) = self.recorder.take() {
            match rec.stop() {
                Ok(p) => Some(p),
                Err(err) => {
                    self.status_error = Some(err.to_string());
                    None
                }
            }
        } else {
            None
        };

        if let Some(hwnd) = extract_hwnd(window) {
            let _ = hide_window_hwnd(hwnd);
        }
        window.remove_window();

        let Some(out_path) = finalized_path else {
            return;
        };

        let w = (self.request.global_rect.width().max(2) as u32) & !1;
        let h = (self.request.global_rect.height().max(2) as u32) & !1;

        // Index recorded video into persistent SQLite FTS5 LibraryDb
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let db_path = library_db_path();
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut db) = LibraryDb::open_file(&db_path) {
            let title = out_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Recording.mp4")
                .to_string();
            let _ = db.upsert_item(&LibraryItem {
                id: 0,
                file_path: out_path.display().to_string(),
                media_type: MediaType::Video,
                capture_date: format!("{ts}"),
                app_name: "SnagSoup".into(),
                window_title: title,
                web_url: String::new(),
                callout_text: String::new(),
                ocr_text: String::new(),
                width: w,
                height: h,
                tags: vec!["video".into(), "recording".into()],
            });
        }

        crate::open_editor_with_video(out_path, cx);
    }

    fn restart_recording(&mut self) {
        if let Some(rec) = self.recorder.take() {
            rec.cancel();
        } else {
            let _ = std::fs::remove_file(&self.output_path);
        }
        let cfg = LiveRecordConfig {
            region: self.request.global_rect,
            hmonitor: self.request.hmonitor,
            fps: 30,
            record_mic: self.request.record_mic,
            record_system_audio: self.request.record_system_audio,
            record_cursor: true,
            output_mp4: self.output_path.clone(),
        };
        self.paused = false;
        match LiveScreenRecorder::start(cfg) {
            Ok(rec) => {
                self.recorder = Some(rec);
                self.status_error = None;
            }
            Err(err) => {
                self.recorder = None;
                self.status_error = Some(err.to_string());
            }
        }
    }

    fn cancel_recording(&mut self, window: &mut Window) {
        if let Some(rec) = self.recorder.take() {
            rec.cancel();
        } else {
            let _ = std::fs::remove_file(&self.output_path);
        }
        if let Some(hwnd) = extract_hwnd(window) {
            let _ = hide_window_hwnd(hwnd);
        }
        window.remove_window();
    }
}

impl Render for LiveRecordingHudView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let timer_text = self.elapsed_mm_ss();
        let rms = self.current_audio_level();
        let level_px = (rms * 54.0).clamp(4.0, 54.0);
        let dot_color = if self.status_error.is_some() {
            rgb(0x9CA3AF)
        } else if self.paused {
            rgb(0xF59E0B)
        } else if self.pulse_on {
            rgb(0xEF4444)
        } else {
            rgb(0x7F1D1D)
        };
        let status_lbl = if self.status_error.is_some() {
            "ERR"
        } else if self.paused {
            "PAUSED"
        } else {
            "REC"
        };

        div()
            .id("floating-mini-recording-hud")
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .size_full()
            .px(px(12.0))
            .py(px(8.0))
            .rounded(px(10.0))
            .bg(rgb(0x181B20))
            .border_1()
            .border_color(if self.paused {
                rgb(0xF59E0B)
            } else {
                rgb(0xDC2626)
            })
            .text_xs()
            .text_color(rgb(0xF9FAFB))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().size(px(10.0)).rounded(px(5.0)).bg(dot_color))
                    .child(status_lbl)
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x111317))
                            .child(timer_text),
                    )
                    // Live Mic / Sys audio level bar
                    .child(
                        div()
                            .w(px(54.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(rgb(0x111317))
                            .child(
                                div()
                                    .w(px(level_px))
                                    .h(px(6.0))
                                    .rounded(px(3.0))
                                    .bg(rgb(0x22C55E)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("mini-hud-pause")
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(0x374151))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_pause();
                                cx.notify();
                            }))
                            .child(if self.paused { "▶ Resume" } else { "⏸ Pause" }),
                    )
                    .child(
                        div()
                            .id("mini-hud-stop")
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(0xDC2626))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.finalize_and_open_editor(window, cx);
                            }))
                            .child("⏹ Stop"),
                    )
                    .child(
                        div()
                            .id("mini-hud-restart")
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(0x374151))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.restart_recording();
                                cx.notify();
                            }))
                            .child("↺"),
                    )
                    .child(
                        div()
                            .id("mini-hud-cancel")
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(0x4B5563))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, _cx| {
                                this.cancel_recording(window);
                            }))
                            .child("✕"),
                    ),
            )
    }
}

/// Start live screen recording for `request`, saving to `%LOCALAPPDATA%\SnagSoup\Captures\VID-<ts>.mp4`,
/// and open the topmost capture-excluded Floating Mini Recording HUD.
pub fn open_live_recording_hud(
    request: VideoRecordRequest,
    cx: &mut App,
) -> Option<WindowHandle<LiveRecordingHudView>> {
    let dir = capture_storage_dir();
    let _ = std::fs::create_dir_all(&dir);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let output_path = dir.join(format!("VID-{ts}.mp4"));

    let hud_w = 390.0f32;
    let hud_h = 56.0f32;
    let phys_x = request.global_rect.left;
    let phys_y = (request.global_rect.bottom + 12).max(12);

    let bounds = Bounds::new(
        gpui::point(px(phys_x as f32), px(phys_y as f32)),
        size(px(hud_w), px(hud_h)),
    );

    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            focus: true,
            ..Default::default()
        },
        move |window, cx| {
            let scale = window.scale_factor().max(0.5);
            if let Some(hwnd) = extract_hwnd(window) {
                let _ = set_window_exclude_from_capture(hwnd, true);
                position_window_physical(
                    hwnd,
                    phys_x,
                    phys_y,
                    (hud_w * scale).round() as i32,
                    (hud_h * scale).round() as i32,
                    true,
                    true,
                );
            }

            cx.new(move |cx| {
                // Periodic 250ms UI tick for live MM:SS timer, pulsing REC dot, and audio level meter
                cx.spawn(async |weak, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(250))
                            .await;
                        let alive = weak
                            .update(cx, |view: &mut LiveRecordingHudView, cx| {
                                view.pulse_on = !view.pulse_on;
                                cx.notify();
                                true
                            })
                            .unwrap_or(false);
                        if !alive {
                            break;
                        }
                    }
                })
                .detach();

                LiveRecordingHudView::new(request, output_path)
            })
        },
    )
    .ok()
}
