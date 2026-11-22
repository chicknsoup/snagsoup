//! Video Pre-Recording, 3-2-1 Countdown, Mini Recording HUD & Webcam PiP controls (`W3-04`)
//! wired to `snagsoup_media`.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div, px, rgb,
};
use snagsoup_media::{
    AudioDeviceInfo, AudioLevelMeter, PipCorner, PipShape, TimestampOffsetter, WebcamDeviceInfo,
    WebcamOverlayMode, enumerate_audio_devices, enumerate_webcams,
};
use std::sync::Arc;
use std::time::Duration;

/// State machine for Video Recording HUD lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingPhase {
    PreRecording,
    /// 3 -> 2 -> 1 countdown before recording begins.
    Countdown(u8),
    Recording,
    Paused,
    Stopped,
}

/// Format an elapsed `Duration` as `MM:SS` for the Mini Recording HUD timer.
pub fn format_mm_ss(d: Duration) -> String {
    let total_secs = d.as_secs();
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

/// GPUI Video Pre-Recording, 3-2-1 Countdown & Mini Recording HUD view.
pub struct RecordingHudView {
    pub phase: RecordingPhase,
    pub mics: Vec<AudioDeviceInfo>,
    pub selected_mic_idx: Option<usize>,
    pub record_mic: bool,
    pub record_system_audio: bool,
    pub webcams: Vec<WebcamDeviceInfo>,
    pub selected_webcam_idx: Option<usize>,
    pub webcam_mode: WebcamOverlayMode,
    pub mic_meter: Arc<AudioLevelMeter>,
    pub offsetter: Option<TimestampOffsetter>,
    pub restart_count: u32,
}

impl Default for RecordingHudView {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingHudView {
    pub fn new() -> Self {
        let mics: Vec<AudioDeviceInfo> = enumerate_audio_devices()
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.is_capture)
            .collect();
        let webcams = enumerate_webcams().unwrap_or_default();
        let selected_mic_idx = if mics.is_empty() { None } else { Some(0) };
        let selected_webcam_idx = if webcams.is_empty() { None } else { Some(0) };

        Self {
            phase: RecordingPhase::PreRecording,
            mics,
            selected_mic_idx,
            record_mic: true,
            record_system_audio: true,
            webcams,
            selected_webcam_idx,
            webcam_mode: WebcamOverlayMode::Pip {
                corner: PipCorner::BottomRight,
                shape: PipShape::Circle,
                scale: 0.22,
                margin: 16,
            },
            mic_meter: Arc::new(AudioLevelMeter::new()),
            offsetter: None,
            restart_count: 0,
        }
    }

    pub fn start_countdown(&mut self) {
        self.phase = RecordingPhase::Countdown(3);
    }

    /// Advance 3 -> 2 -> 1 -> `Recording`.
    pub fn tick_countdown(&mut self) -> RecordingPhase {
        match self.phase {
            RecordingPhase::Countdown(n) if n > 1 => {
                self.phase = RecordingPhase::Countdown(n - 1);
            }
            RecordingPhase::Countdown(_) => {
                self.phase = RecordingPhase::Recording;
                self.offsetter = Some(TimestampOffsetter::start_now());
            }
            _ => {}
        }
        self.phase
    }

    pub fn toggle_pause(&mut self) {
        match self.phase {
            RecordingPhase::Recording => {
                if let Some(ref mut o) = self.offsetter {
                    o.pause();
                }
                self.phase = RecordingPhase::Paused;
            }
            RecordingPhase::Paused => {
                if let Some(ref mut o) = self.offsetter {
                    o.resume();
                }
                self.phase = RecordingPhase::Recording;
            }
            _ => {}
        }
    }

    pub fn restart(&mut self) {
        self.restart_count += 1;
        self.offsetter = None;
        self.phase = RecordingPhase::Countdown(3);
    }

    pub fn cancel(&mut self) {
        self.offsetter = None;
        self.phase = RecordingPhase::PreRecording;
    }

    pub fn stop(&mut self) {
        self.phase = RecordingPhase::Stopped;
    }

    /// Toggle smoothly between **Fullscreen Webcam** and **Screen + PiP** mid-recording!
    pub fn toggle_fullscreen_webcam(&mut self) {
        self.webcam_mode = match self.webcam_mode {
            WebcamOverlayMode::FullscreenWebcam => WebcamOverlayMode::Pip {
                corner: PipCorner::BottomRight,
                shape: PipShape::Circle,
                scale: 0.22,
                margin: 16,
            },
            _ => WebcamOverlayMode::FullscreenWebcam,
        };
    }

    /// Cycle PiP shape: `Circle` -> `RoundedSquare` -> `Rectangle` -> `Circle`.
    pub fn cycle_pip_shape(&mut self) {
        if let WebcamOverlayMode::Pip {
            corner,
            shape,
            scale,
            margin,
        } = self.webcam_mode
        {
            let next_shape = match shape {
                PipShape::Circle => PipShape::RoundedSquare { radius_ratio: 0.2 },
                PipShape::RoundedSquare { .. } => PipShape::Rectangle,
                PipShape::Rectangle => PipShape::Circle,
            };
            self.webcam_mode = WebcamOverlayMode::Pip {
                corner,
                shape: next_shape,
                scale,
                margin,
            };
        }
    }

    pub fn elapsed_text(&self) -> String {
        let d = self
            .offsetter
            .as_ref()
            .map(|o| o.active_elapsed())
            .unwrap_or(Duration::ZERO);
        format_mm_ss(d)
    }
}

impl Render for RecordingHudView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.phase {
            RecordingPhase::Countdown(n) => div()
                .id("recording-countdown")
                .flex()
                .items_center()
                .justify_center()
                .w(px(120.0))
                .h(px(120.0))
                .rounded(px(60.0))
                .bg(rgb(0xDC2626))
                .text_color(rgb(0xFFFFFF))
                .child(format!("{n}")),

            RecordingPhase::PreRecording => {
                let level_px = (self.mic_meter.rms() * 80.0).clamp(2.0, 80.0);
                div()
                    .id("pre-recording-hud")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(rgb(0x1E2228))
                    .text_color(rgb(0xF3F4F6))
                    .child(
                        div()
                            .id("rec-start-btn")
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(16.0))
                            .bg(rgb(0xDC2626))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_countdown();
                                cx.notify();
                            }))
                            .child("Record"),
                    )
                    .child(
                        div()
                            .id("rec-mic-toggle")
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(if self.record_mic {
                                rgb(0x2563EB)
                            } else {
                                rgb(0x374151)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.record_mic = !this.record_mic;
                                cx.notify();
                            }))
                            .child(if self.record_mic { "Mic: ON" } else { "Mic: OFF" }),
                    )
                    .child(
                        div()
                            .w(px(80.0))
                            .h(px(8.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x111317))
                            .child(div().w(px(level_px)).h(px(8.0)).rounded(px(4.0)).bg(rgb(0x22C55E))),
                    )
                    .child(
                        div()
                            .id("rec-sys-toggle")
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(if self.record_system_audio {
                                rgb(0x2563EB)
                            } else {
                                rgb(0x374151)
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.record_system_audio = !this.record_system_audio;
                                cx.notify();
                            }))
                            .child(if self.record_system_audio {
                                "SysAudio: ON"
                            } else {
                                "SysAudio: OFF"
                            }),
                    )
            }

            RecordingPhase::Recording | RecordingPhase::Paused | RecordingPhase::Stopped => {
                let paused = self.phase == RecordingPhase::Paused;
                let timer = self.elapsed_text();
                div()
                    .id("mini-recording-hud")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgb(0x1E2228))
                    .text_color(rgb(0xF3F4F6))
                    .child(timer)
                    .child(
                        div()
                            .id("rec-pause-resume")
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x374151))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_pause();
                                cx.notify();
                            }))
                            .child(if paused { "Resume" } else { "Pause" }),
                    )
                    .child(
                        div()
                            .id("rec-webcam-switch")
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x0D9488))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_fullscreen_webcam();
                                cx.notify();
                            }))
                            .child(match self.webcam_mode {
                                WebcamOverlayMode::FullscreenWebcam => "PiP",
                                _ => "Full Cam",
                            }),
                    )
                    .child(
                        div()
                            .id("rec-restart")
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x4B5563))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.restart();
                                cx.notify();
                            }))
                            .child("Restart"),
                    )
                    .child(
                        div()
                            .id("rec-stop")
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(0xDC2626))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop();
                                cx.notify();
                            }))
                            .child("Stop"),
                    )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_hud_countdown_pause_restart_and_pip_toggle() {
        assert_eq!(format_mm_ss(Duration::from_secs(125)), "02:05");

        let mut hud = RecordingHudView::new();
        assert_eq!(hud.phase, RecordingPhase::PreRecording);

        hud.start_countdown();
        assert_eq!(hud.phase, RecordingPhase::Countdown(3));
        assert_eq!(hud.tick_countdown(), RecordingPhase::Countdown(2));
        assert_eq!(hud.tick_countdown(), RecordingPhase::Countdown(1));
        assert_eq!(hud.tick_countdown(), RecordingPhase::Recording);

        hud.toggle_pause();
        assert_eq!(hud.phase, RecordingPhase::Paused);
        hud.toggle_pause();
        assert_eq!(hud.phase, RecordingPhase::Recording);

        hud.cycle_pip_shape();
        assert!(matches!(
            hud.webcam_mode,
            WebcamOverlayMode::Pip {
                shape: PipShape::RoundedSquare { .. },
                ..
            }
        ));
        hud.toggle_fullscreen_webcam();
        assert_eq!(hud.webcam_mode, WebcamOverlayMode::FullscreenWebcam);

        hud.restart();
        assert_eq!(hud.restart_count, 1);
        assert_eq!(hud.phase, RecordingPhase::Countdown(3));

        hud.stop();
        assert_eq!(hud.phase, RecordingPhase::Stopped);
    }
}
