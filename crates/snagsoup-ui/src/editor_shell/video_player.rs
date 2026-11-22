//! In-Editor Video Player & Snagit Selection Playhead Timeline (`W3-12` Part A):
//! - Play/Pause, single-frame step (`<` / `>`), volume/mute
//! - White center playhead + Green (In-point) & Red (Out-point) handles
//! - Floating **Cut** (`trim_mp4_intervals`), **GIF** (opens `CreateGifDialog`),
//!   and **PNG** (*Save Video Frame as PNG* via `extract_video_frame_rgba`)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    IntoElement, ParentElement, RenderImage, Styled, StyledImage, div, img,
    prelude::FluentBuilder, px, rgb,
};
use snagsoup_canvas::TiledBitmap;
use snagsoup_media::{
    CutInterval, DecodedFrame, Mp4StreamDecoder, VideoMetadata, extract_video_frame_rgba,
    trim_mp4_intervals,
};

use crate::editor_shell::gif_dialog::CreateGifDialogState;
use crate::rgba_to_bgra_render_image;
use crate::theme::SnagTheme;

/// State of Snagit's signature 3-handle Selection Playhead (Green In · White Playhead · Red Out).
pub struct VideoPlayerState {
    pub path: Option<PathBuf>,
    pub metadata: Option<VideoMetadata>,
    pub playing: bool,
    pub playhead_ms: u64,
    pub in_point_ms: u64,
    pub out_point_ms: u64,
    pub volume: f32,
    pub muted: bool,
    pub pending_cuts: Vec<CutInterval>,
    pub current_frame: Option<DecodedFrame>,
    pub current_render_image: Option<Arc<RenderImage>>,
    stream_decoder: Option<Mp4StreamDecoder>,
}

impl std::fmt::Debug for VideoPlayerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPlayerState")
            .field("path", &self.path)
            .field("metadata", &self.metadata)
            .field("playing", &self.playing)
            .field("playhead_ms", &self.playhead_ms)
            .field("in_point_ms", &self.in_point_ms)
            .field("out_point_ms", &self.out_point_ms)
            .finish()
    }
}

impl Default for VideoPlayerState {
    fn default() -> Self {
        Self {
            path: None,
            metadata: None,
            playing: false,
            playhead_ms: 0,
            in_point_ms: 0,
            out_point_ms: 0,
            volume: 1.0,
            muted: false,
            pending_cuts: Vec::new(),
            current_frame: None,
            current_render_image: None,
            stream_decoder: None,
        }
    }
}

impl VideoPlayerState {
    fn refresh_render_image(&mut self) {
        self.current_render_image = self
            .current_frame
            .as_ref()
            .and_then(|f| rgba_to_bgra_render_image(f.width, f.height, &f.rgba));
    }

    /// Open an `.mp4` file, probe metadata, and decode its initial frame into a BGRA [`RenderImage`].
    pub fn open_video(&mut self, path: &Path) -> Result<VideoMetadata, String> {
        let mut decoder =
            Mp4StreamDecoder::open(path).map_err(|e| format!("Mp4StreamDecoder::open: {e}"))?;
        let meta = decoder.metadata();
        let frame = decoder.next_frame().ok().flatten();
        self.path = Some(path.to_path_buf());
        self.metadata = Some(meta);
        self.playhead_ms = 0;
        self.in_point_ms = 0;
        self.out_point_ms = 0;
        self.playing = false;
        self.pending_cuts.clear();
        self.current_frame = frame;
        self.stream_decoder = Some(decoder);
        self.refresh_render_image();
        Ok(meta)
    }

    /// Clear active video mode (e.g. when switching back to an image capture).
    pub fn close_video(&mut self) {
        self.path = None;
        self.metadata = None;
        self.playing = false;
        self.playhead_ms = 0;
        self.in_point_ms = 0;
        self.out_point_ms = 0;
        self.pending_cuts.clear();
        self.current_frame = None;
        self.current_render_image = None;
        self.stream_decoder = None;
    }

    /// Advance sequential playback by one decoded frame; returns `false` when end-of-stream is reached.
    pub fn advance_playback_frame(&mut self) -> bool {
        if self.stream_decoder.is_none() {
            if let Some(ref p) = self.path {
                if let Ok(mut dec) = Mp4StreamDecoder::open(p) {
                    if self.playhead_ms > 0 {
                        dec.seek_ms(self.playhead_ms);
                    }
                    self.stream_decoder = Some(dec);
                }
            }
        }
        if let Some(ref mut dec) = self.stream_decoder {
            match dec.next_frame() {
                Ok(Some(frame)) => {
                    self.playhead_ms = frame.timestamp_ms;
                    self.current_frame = Some(frame);
                    self.refresh_render_image();
                    return true;
                }
                _ => {
                    dec.seek_ms(0);
                    if let Ok(Some(first)) = dec.next_frame() {
                        self.playhead_ms = 0;
                        self.current_frame = Some(first);
                        self.refresh_render_image();
                    } else {
                        self.playhead_ms = 0;
                    }
                    self.playing = false;
                    return false;
                }
            }
        }
        self.playing = false;
        false
    }

    pub fn duration_ms(&self) -> u64 {
        self.metadata.map_or(0, |m| m.duration_ms)
    }

    /// True when the Green (In) and Red (Out) handles are pulled apart, revealing floating Cut & GIF buttons.
    pub fn has_selection_range(&self) -> bool {
        self.out_point_ms > self.in_point_ms
    }

    /// Drag Green (In) and Red (Out) handles to select a time range.
    pub fn set_selection_range(&mut self, in_ms: u64, out_ms: u64) {
        let dur = self.duration_ms();
        let a = in_ms.min(dur);
        let b = out_ms.min(dur);
        self.in_point_ms = a.min(b);
        self.out_point_ms = a.max(b);
    }

    /// Seek to an explicit timestamp in milliseconds and decode the frame.
    pub fn seek_ms(&mut self, ms: u64) {
        let next = ms.min(self.duration_ms());
        self.playhead_ms = next;
        if let Some(ref mut dec) = self.stream_decoder {
            dec.seek_ms(next);
            if let Ok(Some(frame)) = dec.next_frame() {
                self.current_frame = Some(frame);
                self.refresh_render_image();
                return;
            }
        }
        if let Some(ref p) = self.path {
            if let Ok(frame) = extract_video_frame_rgba(p, next) {
                self.current_frame = Some(frame);
                self.refresh_render_image();
            }
        }
    }

    /// Step forward (`+1`) or backward (`-1`) by exactly one video frame (`<` / `>`).
    pub fn step_frame(&mut self, delta_frames: i64) {
        let fps = self.metadata.map_or(30.0, |m| m.fps.max(1.0));
        let frame_ms = (1000.0 / fps).round().max(1.0) as i64;
        let next = (self.playhead_ms as i64 + delta_frames * frame_ms)
            .clamp(0, self.duration_ms() as i64) as u64;
        self.seek_ms(next);
    }

    /// Queue the current Green–Red selection as a cut interval and export trimmed MP4.
    pub fn apply_cut_selection(&mut self, output_mp4: &Path) -> Result<VideoMetadata, String> {
        let input = self
            .path
            .as_ref()
            .ok_or_else(|| "no video loaded".to_string())?;
        if !self.has_selection_range() {
            return Err("no Green-Red timeline selection range to cut".to_string());
        }
        self.pending_cuts
            .push(CutInterval::new(self.in_point_ms, self.out_point_ms));
        trim_mp4_intervals(input, &self.pending_cuts, output_mp4)
            .map_err(|e| format!("trim_mp4_intervals failed: {e}"))
    }

    /// Export the active video (or Green–Red selection range) as an Animated GIF.
    pub fn export_animated_gif(&self, output_gif: &Path) -> Result<(), String> {
        let input = self
            .path
            .as_ref()
            .ok_or_else(|| "no video loaded".to_string())?;
        let mut gif_state = CreateGifDialogState::default();
        if self.has_selection_range() {
            gif_state.start_ms = self.in_point_ms;
            gif_state.end_ms = self.out_point_ms;
        } else {
            gif_state.start_ms = 0;
            gif_state.end_ms = self.duration_ms().max(1000);
        }
        gif_state.export_from_mp4(input, output_gif).map(|_| ())
    }

    /// Extract the frame at `playhead_ms` as a PNG-backed [`TiledBitmap`] (*Save Frame as PNG*).
    pub fn capture_current_frame_as_bitmap(&self) -> Result<TiledBitmap, String> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| "no video loaded".to_string())?;
        let frame = extract_video_frame_rgba(path, self.playhead_ms)
            .map_err(|e| format!("extract_video_frame_rgba failed: {e}"))?;
        TiledBitmap::from_rgba(frame.width, frame.height, &frame.rgba)
            .map_err(|e| format!("TiledBitmap::from_rgba failed: {e}"))
    }
}

/// Render the decoded video frame surface inside the Editor canvas well.
pub fn render_video_stage(player: &VideoPlayerState, theme: SnagTheme) -> impl IntoElement {
    let dims_text = player
        .metadata
        .map(|m| format!("{} × {} · {:.1} fps", m.width, m.height, m.fps))
        .unwrap_or_else(|| "Video".to_string());

    div()
        .relative()
        .flex()
        .flex_1()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .p(px(16.0))
        .bg(theme.canvas_well_bg)
        .child(
            div()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .rounded(theme.radius_md)
                .bg(rgb(0x0B0E14))
                .border_1()
                .border_color(theme.border_strong)
                .overflow_hidden()
                .when_some(player.current_render_image.clone(), |el, frame_img| {
                    el.child(
                        img(frame_img)
                            .object_fit(gpui::ObjectFit::Contain)
                            .w_full()
                            .h_full(),
                    )
                })
                .when(player.current_render_image.is_none(), |el| {
                    el.child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(format!("Loaded video ({dims_text})")),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .top(px(10.0))
                        .left(px(10.0))
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(theme.radius_sm)
                        .bg(rgb(0x111827))
                        .text_xs()
                        .text_color(rgb(0xF3F4F6))
                        .child(format!(
                            "VIDEO · {dims_text} · {}",
                            format_timestamp_ms(player.playhead_ms)
                        )),
                ),
        )
}

/// Format milliseconds as `MM:SS.d`.
pub fn format_timestamp_ms(ms: u64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    let tenths = (ms % 1000) / 100;
    format!("{mins:02}:{secs:02}.{tenths}")
}

/// Render the Snagit Selection Playhead Timeline bar (Green In, White Playhead, Red Out + floating Cut/GIF/PNG).
pub fn render_video_timeline(player: &VideoPlayerState, theme: SnagTheme) -> impl IntoElement {
    let dur = player.duration_ms().max(1) as f32;
    let track_w = 560.0f32;
    let in_x = (player.in_point_ms as f32 / dur) * track_w;
    let out_x = (player.out_point_ms as f32 / dur) * track_w;
    let head_x = (player.playhead_ms as f32 / dur) * track_w;
    let has_sel = player.has_selection_range();

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(12.0))
        .bg(theme.toolbar_bg)
        .border_t_1()
        .border_color(theme.border)
        .when(has_sel, |el| {
            el.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.danger)
                            .text_xs()
                            .text_color(theme.text_on_accent)
                            .child(format!(
                                "Cut ({} – {})",
                                format_timestamp_ms(player.in_point_ms),
                                format_timestamp_ms(player.out_point_ms)
                            )),
                    )
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.accent_primary)
                            .text_xs()
                            .text_color(theme.text_on_accent)
                            .child("Create GIF"),
                    ),
            )
        })
        .child(
            div()
                .relative()
                .w(px(track_w))
                .h(px(28.0))
                .flex()
                .items_center()
                .child(div().w_full().h(px(6.0)).rounded(px(3.0)).bg(theme.border))
                .when(has_sel, |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(px(in_x))
                            .w(px((out_x - in_x).max(2.0)))
                            .h(px(6.0))
                            .bg(theme.accent_primary),
                    )
                })
                // Green In-point handle
                .child(
                    div()
                        .absolute()
                        .left(px(in_x - 4.0))
                        .w(px(8.0))
                        .h(px(18.0))
                        .rounded(px(3.0))
                        .bg(rgb(0x22C55E)),
                )
                // Red Out-point handle
                .child(
                    div()
                        .absolute()
                        .left(px(out_x - 4.0))
                        .w(px(8.0))
                        .h(px(18.0))
                        .rounded(px(3.0))
                        .bg(rgb(0xEF4444)),
                )
                // White center playhead
                .child(
                    div()
                        .absolute()
                        .left(px(head_x - 2.0))
                        .w(px(4.0))
                        .h(px(24.0))
                        .rounded(px(2.0))
                        .bg(rgb(0xFFFFFF))
                        .border_1()
                        .border_color(theme.border_strong),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(format!(
                    "{} / {}  ·  PNG Frame Capture Ready",
                    format_timestamp_ms(player.playhead_ms),
                    format_timestamp_ms(player.duration_ms())
                )),
        )
}
