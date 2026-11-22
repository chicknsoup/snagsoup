//! Easy-to-drive live screen + audio recorder (`LiveScreenRecorder`) for the Recording HUD.
//!
//! Captures live frames of a target `PhysicalRect` at `fps` using a continuous
//! `Windows.Graphics.Capture` (`WgcMonitorSession`) frame pool on the target `HMONITOR`
//! (falling back seamlessly to `ScreenFreezer`), mixes optional WASAPI Microphone and
//! System Audio Loopback via `LockFreeAudioMixer`, and encodes H.264 + 48 kHz AAC into
//! `Mp4SinkWriter`.

use crate::MediaError;
use crate::audio_mixer::{LockFreeAudioMixer, TARGET_SAMPLE_RATE, WasapiCaptureStream};
use crate::recorder::{HNS_PER_SECOND, Mp4RecorderConfig, Mp4SinkWriter, TimestampOffsetter};
use snagsoup_capture::ScreenFreezer;
use snagsoup_capture::wgc::WgcMonitorSession;
use snagsoup_win_platform::dpi::{MonitorInfo, PhysicalPoint, PhysicalRect, enumerate_monitors};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Configuration for `LiveScreenRecorder::start`.
#[derive(Debug, Clone)]
pub struct LiveRecordConfig {
    /// Physical virtual-desktop region to record (dimensions rounded down to even `w & !1, h & !1`).
    pub region: PhysicalRect,
    /// Optional target `HMONITOR` handle; if `None`, automatically resolved from `region`.
    pub hmonitor: Option<isize>,
    /// Target video frame rate (e.g. `30` or `60`).
    pub fps: u32,
    /// Capture default microphone via WASAPI.
    pub record_mic: bool,
    /// Capture default system audio loopback via WASAPI.
    pub record_system_audio: bool,
    /// Include the mouse cursor in WGC capture.
    pub record_cursor: bool,
    /// Destination MP4 file path.
    pub output_mp4: PathBuf,
}

struct RecorderSharedState {
    stop: AtomicBool,
    cancel: AtomicBool,
    paused: AtomicBool,
    mic_muted: AtomicBool,
    sys_muted: AtomicBool,
    elapsed_ms: AtomicU64,
    mic_level_bits: AtomicU32,
    sys_level_bits: AtomicU32,
}

impl RecorderSharedState {
    fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            mic_muted: AtomicBool::new(false),
            sys_muted: AtomicBool::new(false),
            elapsed_ms: AtomicU64::new(0),
            mic_level_bits: AtomicU32::new(0.0f32.to_bits()),
            sys_level_bits: AtomicU32::new(0.0f32.to_bits()),
        }
    }
}

/// Active background screen & audio recorder with thread-safe HUD controls.
pub struct LiveScreenRecorder {
    shared: Arc<RecorderSharedState>,
    output_mp4: PathBuf,
    thread: Option<JoinHandle<Result<PathBuf, MediaError>>>,
}

impl LiveScreenRecorder {
    /// Start recording `cfg.region` to `cfg.output_mp4` on a dedicated background thread.
    pub fn start(cfg: LiveRecordConfig) -> Result<Self, MediaError> {
        let raw_w = cfg.region.width();
        let raw_h = cfg.region.height();
        if raw_w < 2 || raw_h < 2 {
            return Err(MediaError::InvalidConfig(format!(
                "recording region too small: {raw_w}x{raw_h}"
            )));
        }
        let enc_w = (raw_w as u32) & !1;
        let enc_h = (raw_h as u32) & !1;
        if enc_w < 2 || enc_h < 2 {
            return Err(MediaError::InvalidConfig(format!(
                "even recording dimensions too small: {enc_w}x{enc_h}"
            )));
        }

        let shared = Arc::new(RecorderSharedState::new());
        let shared_bg = Arc::clone(&shared);
        let output_mp4 = cfg.output_mp4.clone();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), MediaError>>(1);

        let handle = thread::Builder::new()
            .name("snagsoup-live-recorder".into())
            .spawn(move || recording_thread(cfg, enc_w, enc_h, shared_bg, init_tx))
            .map_err(|e| MediaError::Io(e.to_string()))?;

        match init_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                output_mp4,
                thread: Some(handle),
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                Err(MediaError::Io(
                    "recording thread exited before initialization completed".into(),
                ))
            }
        }
    }

    /// Pause recording (active elapsed time and MP4 timestamps freeze without gaps).
    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::SeqCst);
    }

    /// Resume recording after `pause()`.
    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::SeqCst);
    }

    /// Whether recording is currently paused.
    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// Active (non-paused) recorded duration in milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.shared.elapsed_ms.load(Ordering::Relaxed)
    }

    /// Current microphone RMS level (`0.0..=1.0`) for the HUD level meter.
    pub fn mic_level(&self) -> f32 {
        f32::from_bits(self.shared.mic_level_bits.load(Ordering::Relaxed))
    }

    /// Current system audio RMS level (`0.0..=1.0`) for the HUD level meter.
    pub fn system_audio_level(&self) -> f32 {
        f32::from_bits(self.shared.sys_level_bits.load(Ordering::Relaxed))
    }

    /// Mute or unmute microphone input live.
    pub fn set_mic_muted(&self, muted: bool) {
        self.shared.mic_muted.store(muted, Ordering::SeqCst);
    }

    /// Mute or unmute system audio loopback live.
    pub fn set_system_audio_muted(&self, muted: bool) {
        self.shared.sys_muted.store(muted, Ordering::SeqCst);
    }

    /// Stop recording, finalize the MP4 container (`moov` atom), and return the output path.
    pub fn stop(mut self) -> Result<PathBuf, MediaError> {
        self.shared.stop.store(true, Ordering::SeqCst);
        match self.thread.take() {
            Some(handle) => match handle.join() {
                Ok(res) => res,
                Err(_) => Err(MediaError::Io("recording thread panicked".into())),
            },
            None => Ok(self.output_mp4.clone()),
        }
    }

    /// Stop recording and delete any partial output file.
    pub fn cancel(mut self) {
        self.shared.cancel.store(true, Ordering::SeqCst);
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(&self.output_mp4);
    }
}

impl Drop for LiveScreenRecorder {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn resolve_target_monitor(
    region: PhysicalRect,
    explicit_hmonitor: Option<isize>,
) -> Option<MonitorInfo> {
    let topo = enumerate_monitors().ok()?;
    if let Some(hm) = explicit_hmonitor {
        if let Some(m) = topo.monitors.iter().find(|m| m.hmonitor == hm) {
            return Some(m.clone());
        }
    }
    let center = PhysicalPoint {
        x: region.left + region.width() / 2,
        y: region.top + region.height() / 2,
    };
    topo.monitor_at_physical(center)
        .or_else(|| topo.primary_monitor())
        .cloned()
}

fn rect_fits_inside(inner: PhysicalRect, outer: PhysicalRect) -> bool {
    inner.left >= outer.left
        && inner.top >= outer.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

fn crop_rgba_region(
    src_rgba: &[u8],
    src_w: u32,
    src_h: u32,
    src_bounds: PhysicalRect,
    dst_rect: PhysicalRect,
    enc_w: u32,
    enc_h: u32,
    dst_buf: &mut [u8],
) {
    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = enc_w as usize;
    let dh = enc_h as usize;

    dst_buf.fill(0);
    // Ensure alpha is opaque even for any out-of-bounds padding pixels.
    for px in dst_buf.chunks_exact_mut(4) {
        px[3] = 255;
    }

    let ox = dst_rect.left - src_bounds.left;
    let oy = dst_rect.top - src_bounds.top;

    let src_x_start = ox.max(0) as usize;
    let src_y_start = oy.max(0) as usize;
    let dst_x_start = (-ox).max(0) as usize;
    let dst_y_start = (-oy).max(0) as usize;

    if src_x_start >= sw || src_y_start >= sh || dst_x_start >= dw || dst_y_start >= dh {
        return;
    }

    let copy_w = (sw - src_x_start).min(dw - dst_x_start);
    let copy_h = (sh - src_y_start).min(dh - dst_y_start);

    for y in 0..copy_h {
        let s_off = ((src_y_start + y) * sw + src_x_start) * 4;
        let d_off = ((dst_y_start + y) * dw + dst_x_start) * 4;
        let byte_len = copy_w * 4;
        if s_off + byte_len <= src_rgba.len() && d_off + byte_len <= dst_buf.len() {
            dst_buf[d_off..d_off + byte_len]
                .copy_from_slice(&src_rgba[s_off..s_off + byte_len]);
        }
    }
}

fn recording_thread(
    cfg: LiveRecordConfig,
    enc_w: u32,
    enc_h: u32,
    shared: Arc<RecorderSharedState>,
    init_tx: mpsc::SyncSender<Result<(), MediaError>>,
) -> Result<PathBuf, MediaError> {
    let fps = cfg.fps.clamp(1, 120);
    let want_audio = cfg.record_mic || cfg.record_system_audio;

    let mut mp4_cfg = Mp4RecorderConfig::new(&cfg.output_mp4, enc_w, enc_h);
    mp4_cfg.fps = fps;
    mp4_cfg.record_audio = want_audio;

    let writer = match Mp4SinkWriter::new(&mp4_cfg) {
        Ok(w) => w,
        Err(e) => {
            let _ = init_tx.send(Err(e.clone()));
            return Err(e);
        }
    };

    // Start optional WASAPI streams (non-fatal if an endpoint is unavailable on this host).
    let mut _mic_stream: Option<WasapiCaptureStream> = None;
    let mut mic_consumer = None;
    if cfg.record_mic {
        if let Ok((stream, rx)) = WasapiCaptureStream::start_default(false) {
            let rate = stream.sample_rate();
            let ch = stream.channels();
            _mic_stream = Some(stream);
            mic_consumer = Some((rx, rate, ch));
        }
    }

    let mut _sys_stream: Option<WasapiCaptureStream> = None;
    let mut sys_consumer = None;
    if cfg.record_system_audio {
        if let Ok((stream, rx)) = WasapiCaptureStream::start_default(true) {
            let rate = stream.sample_rate();
            let ch = stream.channels();
            _sys_stream = Some(stream);
            sys_consumer = Some((rx, rate, ch));
        }
    }

    let mut mixer = if want_audio {
        Some(LockFreeAudioMixer::new(mic_consumer, sys_consumer))
    } else {
        None
    };

    // Prefer continuous WGC on the target monitor when `region` fits inside that monitor.
    let target_mon = resolve_target_monitor(cfg.region, cfg.hmonitor);
    let mut wgc_session: Option<(WgcMonitorSession, PhysicalRect)> = target_mon
        .as_ref()
        .filter(|m| rect_fits_inside(cfg.region, m.physical_rect))
        .and_then(|m| {
            WgcMonitorSession::start(m.hmonitor, cfg.record_cursor)
                .ok()
                .map(|s| (s, m.physical_rect))
        });

    let mut freezer: Option<ScreenFreezer> = None;
    let mut last_full_frame: Option<(u32, u32, PhysicalRect, Vec<u8>)> = None;

    // Pre-warm the initial frame before starting the recording clock so `elapsed_ms`
    // and MP4 timestamps begin cleanly without WGC startup latency.
    if let Some((ref mut wgc, mon_bounds)) = wgc_session {
        match wgc.acquire_frame_rgba(1000) {
            Ok((w, h, rgba)) => {
                last_full_frame = Some((w, h, mon_bounds, rgba));
            }
            Err(_) => {
                wgc_session = None;
            }
        }
    }
    if last_full_frame.is_none() {
        let fz = freezer.get_or_insert_with(ScreenFreezer::new);
        if let Ok(frozen) = fz.freeze_virtual_desktop() {
            last_full_frame = Some((
                frozen.frame.width,
                frozen.frame.height,
                frozen.frame.bounds,
                frozen.frame.rgba,
            ));
        }
    }

    // Notify caller that initialization succeeded and recording has begun.
    let _ = init_tx.send(Ok(()));

    let frame_interval = Duration::from_secs_f64(1.0 / fps as f64);
    let mut ts = TimestampOffsetter::start_now();
    let mut cropped = vec![0u8; (enc_w as usize) * (enc_h as usize) * 4];
    let mut wrote_any_video = false;
    let mut last_video_ts_hns: i64 = 0;
    let mut audio_frames_written: i64 = 0;
    let mut audio_scratch = Vec::<f32>::new();

    while !shared.stop.load(Ordering::Relaxed) {
        let tick_start = Instant::now();

        if shared.paused.load(Ordering::Relaxed) {
            if !ts.is_paused() {
                ts.pause();
            }
            // Drain audio ring buffers during pause so stale samples don't burst on resume.
            if let Some(ref mut m) = mixer {
                audio_scratch.resize((TARGET_SAMPLE_RATE as usize / 50) * 2, 0.0);
                m.mix_block(&mut audio_scratch);
                shared
                    .mic_level_bits
                    .store(0.0f32.to_bits(), Ordering::Relaxed);
                shared
                    .sys_level_bits
                    .store(0.0f32.to_bits(), Ordering::Relaxed);
            }
            thread::sleep(Duration::from_millis(10));
            continue;
        } else if ts.is_paused() {
            ts.resume();
        }

        // 1. Acquire latest frame (WGC continuous pool -> cached static frame -> ScreenFreezer fallback)
        let mut got_new_frame = false;
        if let Some((ref mut wgc, mon_bounds)) = wgc_session {
            let wait_ms = if last_full_frame.is_none() { 500 } else { 2 };
            match wgc.acquire_frame_rgba(wait_ms) {
                Ok((w, h, rgba)) => {
                    last_full_frame = Some((w, h, mon_bounds, rgba));
                    got_new_frame = true;
                }
                Err(snagsoup_capture::CaptureError::WaitTimeout) => {
                    // Desktop is static; reuse `last_full_frame` if we already have one.
                }
                Err(_) => {
                    // WGC session failed; drop it and switch to ScreenFreezer fallback.
                    wgc_session = None;
                }
            }
        }

        if !got_new_frame && last_full_frame.is_none() {
            let fz = freezer.get_or_insert_with(ScreenFreezer::new);
            if let Ok(frozen) = fz.freeze_virtual_desktop() {
                last_full_frame = Some((
                    frozen.frame.width,
                    frozen.frame.height,
                    frozen.frame.bounds,
                    frozen.frame.rgba,
                ));
            }
        } else if wgc_session.is_none() {
            let fz = freezer.get_or_insert_with(ScreenFreezer::new);
            if let Ok(frozen) = fz.freeze_virtual_desktop() {
                last_full_frame = Some((
                    frozen.frame.width,
                    frozen.frame.height,
                    frozen.frame.bounds,
                    frozen.frame.rgba,
                ));
            }
        }

        // 2. Crop & encode video frame (re-check pause flag so elapsed_ms freezes immediately)
        if shared.paused.load(Ordering::Relaxed) {
            if !ts.is_paused() {
                ts.pause();
            }
            continue;
        }
        if let Some((fw, fh, fbounds, ref frgba)) = last_full_frame {
            crop_rgba_region(
                frgba,
                fw,
                fh,
                fbounds,
                cfg.region,
                enc_w,
                enc_h,
                &mut cropped,
            );

            let active_hns = ts.active_hns();
            let video_ts_hns = if wrote_any_video {
                active_hns.max(last_video_ts_hns + 10_000)
            } else {
                0
            };

            if writer
                .write_video_frame_rgba(&cropped, enc_w, enc_h, video_ts_hns)
                .is_ok()
            {
                wrote_any_video = true;
                last_video_ts_hns = video_ts_hns;
            }

            // 3. Mix & write synchronized 48kHz stereo audio
            if let Some(ref mut m) = mixer {
                m.mic_gain = if shared.mic_muted.load(Ordering::Relaxed) {
                    0.0
                } else {
                    1.0
                };
                m.sys_gain = if shared.sys_muted.load(Ordering::Relaxed) {
                    0.0
                } else {
                    1.0
                };

                let target_audio_frames =
                    ((active_hns.max(0) as i128 * TARGET_SAMPLE_RATE as i128)
                        / HNS_PER_SECOND as i128) as i64;
                let min_chunk = (TARGET_SAMPLE_RATE / fps).max(480) as i64;
                let needed = (target_audio_frames - audio_frames_written).max(min_chunk);
                let chunk_frames = needed.min(9600) as usize;

                audio_scratch.resize(chunk_frames * 2, 0.0);
                m.mix_block(&mut audio_scratch);

                shared
                    .mic_level_bits
                    .store(m.mic_meter.rms().to_bits(), Ordering::Relaxed);
                shared
                    .sys_level_bits
                    .store(m.sys_meter.rms().to_bits(), Ordering::Relaxed);

                let audio_ts_hns =
                    (audio_frames_written * HNS_PER_SECOND) / (TARGET_SAMPLE_RATE as i64);
                if writer
                    .write_audio_f32_stereo(&audio_scratch, TARGET_SAMPLE_RATE, audio_ts_hns)
                    .is_ok()
                {
                    audio_frames_written += chunk_frames as i64;
                }
            }

            shared
                .elapsed_ms
                .store(ts.active_elapsed().as_millis() as u64, Ordering::Relaxed);
        }

        let spent = tick_start.elapsed();
        if spent < frame_interval {
            thread::sleep(frame_interval - spent);
        }
    }

    // Explicitly stop capture sessions before finalizing the MP4.
    drop(wgc_session);
    drop(_mic_stream);
    drop(_sys_stream);

    if shared.cancel.load(Ordering::Relaxed) {
        drop(writer);
        let _ = std::fs::remove_file(&cfg.output_mp4);
        return Ok(cfg.output_mp4);
    }

    writer.finalize()?;
    Ok(cfg.output_mp4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_decoder::{extract_video_frame_rgba, probe_video_metadata};
    use snagsoup_capture::non_black_pixel_ratio;
    use snagsoup_win_platform::dpi::init_per_monitor_dpi_v2;

    #[test]
    fn live_screen_recorder_records_pauses_resumes_and_produces_non_black_mp4() {
        let _ = init_per_monitor_dpi_v2();
        let out_mp4 = std::env::temp_dir().join("snagsoup_live_recorder_test.mp4");
        let _ = std::fs::remove_file(&out_mp4);

        let cfg = LiveRecordConfig {
            region: PhysicalRect::new(0, 0, 640, 360),
            hmonitor: None,
            fps: 30,
            record_mic: false,
            record_system_audio: false,
            record_cursor: true,
            output_mp4: out_mp4.clone(),
        };

        let rec = LiveScreenRecorder::start(cfg).expect("LiveScreenRecorder::start should succeed");
        thread::sleep(Duration::from_millis(450));

        rec.pause();
        assert!(rec.is_paused());
        let elapsed_at_pause = rec.elapsed_ms();
        thread::sleep(Duration::from_millis(300));
        assert!(
            rec.elapsed_ms().abs_diff(elapsed_at_pause) <= 50,
            "elapsed_ms should freeze while paused"
        );

        rec.resume();
        assert!(!rec.is_paused());
        thread::sleep(Duration::from_millis(450));

        let saved_path = rec.stop().expect("LiveScreenRecorder::stop should finalize MP4");
        assert_eq!(saved_path, out_mp4);

        let meta = probe_video_metadata(&saved_path).expect("probe_video_metadata should succeed");
        eprintln!(
            "[live_recorder_test] probed MP4: {}x{} duration={}ms fps={:.1}",
            meta.width, meta.height, meta.duration_ms, meta.fps
        );
        assert_eq!(meta.width, 640);
        assert_eq!(meta.height, 360);
        assert!(meta.duration_ms > 0, "recorded MP4 duration must be > 0ms");

        let frame = extract_video_frame_rgba(&saved_path, 0)
            .expect("extract_video_frame_rgba should decode first frame");
        let non_black = non_black_pixel_ratio(&frame.rgba);
        eprintln!(
            "[live_recorder_test] decoded frame {}x{} non_black={:.2}%",
            frame.width,
            frame.height,
            non_black * 100.0
        );
        assert!(
            non_black > 0.0,
            "decoded MP4 frame should contain non-black pixels from the live desktop"
        );

        let _ = std::fs::remove_file(&saved_path);
    }
}
