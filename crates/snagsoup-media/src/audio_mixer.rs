//! Lock-free WASAPI Audio Mixer (Microphone + System Audio Loopback) backed by `rtrb`
//! single-producer single-consumer ring buffers (zero heap allocation on real-time audio
//! threads), arbitrary-rate linear resampler to `48 kHz Stereo`, soft-clipping limiter,
//! and atomic RMS/peak level meters for the Recording HUD.

use crate::MediaError;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE, IAudioCaptureClient,
    IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, eCapture, eConsole,
    eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::core::GUID;

/// Target mixer output sample rate (48 kHz Stereo for AAC Media Foundation).
pub const TARGET_SAMPLE_RATE: u32 = 48_000;

/// PKEY_Device_FriendlyName `{a45c254e-df1c-4efd-8020-67d146a850e0}, 14`.
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

/// Metadata for a Windows WASAPI capture (mic) or render (loopback) audio endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_capture: bool,
    pub is_default: bool,
}

/// Enumerate active Windows Microphone (`eCapture`) and System Audio (`eRender`) endpoints.
pub fn enumerate_audio_devices() -> Result<Vec<AudioDeviceInfo>, MediaError> {
    let co_init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let result = (|| -> Result<Vec<AudioDeviceInfo>, MediaError> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| {
                MediaError::Wasapi {
                    api: "CoCreateInstance(MMDeviceEnumerator)",
                    hresult: e.code().0,
                }
            })?;

        let mut devices = Vec::new();
        for (flow, is_capture) in [(eCapture, true), (eRender, false)] {
            let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }
                .ok()
                .and_then(|d| unsafe { d.GetId().ok() })
                .and_then(|pw| unsafe { pw.to_string().ok() });

            let Ok(coll) = (unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) })
            else {
                continue;
            };
            let count = unsafe { coll.GetCount() }.unwrap_or(0);
            for i in 0..count {
                let Ok(dev) = (unsafe { coll.Item(i) }) else {
                    continue;
                };
                let id = unsafe { dev.GetId().ok() }
                    .and_then(|pw| unsafe { pw.to_string().ok() })
                    .unwrap_or_default();
                let name = unsafe { dev.OpenPropertyStore(STGM_READ).ok() }
                    .and_then(|store| unsafe { store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME).ok() })
                    .and_then(|pv| {
                        let pw = unsafe { PropVariantToStringAlloc(&pv).ok()? };
                        let s = unsafe { pw.to_string().ok() };
                        unsafe { CoTaskMemFree(Some(pw.0 as *const _)) };
                        s.filter(|s| !s.is_empty())
                    })
                    .unwrap_or_else(|| id.clone());
                let is_default = default_id.as_deref() == Some(id.as_str());
                devices.push(AudioDeviceInfo {
                    id,
                    name,
                    is_capture,
                    is_default,
                });
            }
        }
        Ok(devices)
    })();

    if co_init.is_ok() {
        unsafe { CoUninitialize() };
    }
    result
}

/// Lock-free atomic RMS & peak audio level meter (`0.0..=1.0`) for 60Hz Recording HUD.
#[derive(Debug, Default)]
pub struct AudioLevelMeter {
    rms_bits: AtomicU32,
    peak_bits: AtomicU32,
}

impl AudioLevelMeter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update_from_interleaved(&self, samples: &[f32]) {
        if samples.is_empty() {
            self.rms_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            self.peak_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            return;
        }
        let mut sum_sq = 0.0f64;
        let mut peak = 0.0f32;
        for &s in samples {
            let a = s.abs();
            if a > peak {
                peak = a;
            }
            sum_sq += (s as f64) * (s as f64);
        }
        let rms = (sum_sq / samples.len() as f64).sqrt().clamp(0.0, 1.0) as f32;
        self.rms_bits.store(rms.to_bits(), Ordering::Relaxed);
        self.peak_bits
            .store(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn rms(&self) -> f32 {
        f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak_bits.load(Ordering::Relaxed))
    }
}

/// Smooth soft-clipping limiter preventing digital distortion when Mic + System Audio sum > 1.0.
#[inline]
pub fn soft_limit(x: f32) -> f32 {
    let ax = x.abs();
    if ax <= 0.85 {
        x
    } else {
        let excess = ax - 0.85;
        let limited = 0.85 + 0.15 * (excess / 0.15).tanh();
        x.signum() * limited.min(0.999)
    }
}

/// Lightweight streaming linear resampler converting arbitrary input sample rate & channel
/// count (`mono` or `stereo`) to `48 kHz Stereo` (`[f32; 2]` frames).
#[derive(Debug, Clone)]
pub struct LinearResampler {
    in_rate: u32,
    in_channels: u16,
    out_rate: u32,
    phase: f64,
    prev_frame: [f32; 2],
    pending: Vec<[f32; 2]>,
}

impl LinearResampler {
    pub fn new(in_rate: u32, in_channels: u16, out_rate: u32) -> Self {
        Self {
            in_rate: in_rate.max(8_000),
            in_channels: in_channels.max(1),
            out_rate: out_rate.max(8_000),
            phase: 0.0,
            prev_frame: [0.0, 0.0],
            pending: Vec::new(),
        }
    }

    /// Push raw interleaved float32 samples in the source format.
    pub fn push_interleaved(&mut self, samples: &[f32]) {
        let ch = self.in_channels as usize;
        for frame in samples.chunks_exact(ch) {
            let stereo = if ch == 1 {
                [frame[0], frame[0]]
            } else {
                [frame[0], frame[1]]
            };
            self.pending.push(stereo);
        }
    }

    /// Produce `out_frames` stereo frames at `out_rate` into `dst` (`dst.len() >= out_frames * 2`).
    /// Automatically pads with `0.0` silence when the source stream has no data (e.g. silent loopback).
    pub fn pull_stereo(&mut self, dst: &mut [f32]) {
        if self.pending.is_empty() {
            dst.fill(0.0);
            self.prev_frame = [0.0, 0.0];
            self.phase = 0.0;
            return;
        }
        let out_frames = dst.len() / 2;
        let step = self.in_rate as f64 / self.out_rate as f64;
        let mut consumed = 0usize;

        for i in 0..out_frames {
            let idx = self.phase.floor() as usize;
            let frac = (self.phase - idx as f64) as f32;

            if idx >= self.pending.len() {
                dst[i * 2] = 0.0;
                dst[i * 2 + 1] = 0.0;
                consumed = self.pending.len();
                continue;
            }

            let f0 = if idx == 0 {
                self.prev_frame
            } else {
                self.pending[idx - 1]
            };
            let f1 = self.pending[idx];

            dst[i * 2] = f0[0] + (f1[0] - f0[0]) * frac;
            dst[i * 2 + 1] = f0[1] + (f1[1] - f0[1]) * frac;

            self.phase += step;
            consumed = self.phase.floor() as usize;
        }

        if consumed > 0 {
            let drain_count = consumed.min(self.pending.len());
            self.prev_frame = self.pending[drain_count - 1];
            self.pending.drain(0..drain_count);
            self.phase = (self.phase - drain_count as f64).max(0.0);
        }
    }
}

/// Lock-free dual-stream audio mixer (Microphone + System Loopback -> `48 kHz Stereo`).
pub struct LockFreeAudioMixer {
    mic_rx: Option<Consumer<f32>>,
    sys_rx: Option<Consumer<f32>>,
    mic_resampler: LinearResampler,
    sys_resampler: LinearResampler,
    pub mic_gain: f32,
    pub sys_gain: f32,
    pub mic_meter: Arc<AudioLevelMeter>,
    pub sys_meter: Arc<AudioLevelMeter>,
    pub master_meter: Arc<AudioLevelMeter>,
    scratch_raw: Vec<f32>,
    scratch_mic: Vec<f32>,
    scratch_sys: Vec<f32>,
}

impl LockFreeAudioMixer {
    pub fn new(
        mic_rx: Option<(Consumer<f32>, u32, u16)>,
        sys_rx: Option<(Consumer<f32>, u32, u16)>,
    ) -> Self {
        let (mic_c, mic_rate, mic_ch) = match mic_rx {
            Some((c, r, ch)) => (Some(c), r, ch),
            None => (None, TARGET_SAMPLE_RATE, 2),
        };
        let (sys_c, sys_rate, sys_ch) = match sys_rx {
            Some((c, r, ch)) => (Some(c), r, ch),
            None => (None, TARGET_SAMPLE_RATE, 2),
        };
        Self {
            mic_rx: mic_c,
            sys_rx: sys_c,
            mic_resampler: LinearResampler::new(mic_rate, mic_ch, TARGET_SAMPLE_RATE),
            sys_resampler: LinearResampler::new(sys_rate, sys_ch, TARGET_SAMPLE_RATE),
            mic_gain: 1.0,
            sys_gain: 1.0,
            mic_meter: Arc::new(AudioLevelMeter::new()),
            sys_meter: Arc::new(AudioLevelMeter::new()),
            master_meter: Arc::new(AudioLevelMeter::new()),
            scratch_raw: Vec::with_capacity(4096),
            scratch_mic: Vec::with_capacity(4096),
            scratch_sys: Vec::with_capacity(4096),
        }
    }

    /// Mix `out_stereo_48k` (`frames * 2` floats) from both lock-free `rtrb` rings,
    /// injecting silence when a stream is quiet and applying `soft_limit`.
    pub fn mix_block(&mut self, out_stereo_48k: &mut [f32]) {
        let len = out_stereo_48k.len();
        self.scratch_mic.resize(len, 0.0);
        self.scratch_sys.resize(len, 0.0);

        // Drain mic lock-free ring
        if let Some(ref mut rx) = self.mic_rx {
            self.scratch_raw.clear();
            let avail = rx.slots().min(4096);
            if let Ok(chunk) = rx.read_chunk(avail) {
                let (s1, s2) = chunk.as_slices();
                self.scratch_raw.extend_from_slice(s1);
                self.scratch_raw.extend_from_slice(s2);
                chunk.commit_all();
            }
            if !self.scratch_raw.is_empty() {
                self.mic_resampler.push_interleaved(&self.scratch_raw);
            }
            self.mic_resampler.pull_stereo(&mut self.scratch_mic);
            self.mic_meter.update_from_interleaved(&self.scratch_mic);
        } else {
            self.scratch_mic.fill(0.0);
        }

        // Drain system loopback lock-free ring
        if let Some(ref mut rx) = self.sys_rx {
            self.scratch_raw.clear();
            let avail = rx.slots().min(4096);
            if let Ok(chunk) = rx.read_chunk(avail) {
                let (s1, s2) = chunk.as_slices();
                self.scratch_raw.extend_from_slice(s1);
                self.scratch_raw.extend_from_slice(s2);
                chunk.commit_all();
            }
            if !self.scratch_raw.is_empty() {
                self.sys_resampler.push_interleaved(&self.scratch_raw);
            }
            self.sys_resampler.pull_stereo(&mut self.scratch_sys);
            self.sys_meter.update_from_interleaved(&self.scratch_sys);
        } else {
            self.scratch_sys.fill(0.0);
        }

        for i in 0..len {
            let mixed = self.scratch_mic[i] * self.mic_gain + self.scratch_sys[i] * self.sys_gain;
            out_stereo_48k[i] = soft_limit(mixed);
        }
        self.master_meter.update_from_interleaved(out_stereo_48k);
    }
}

/// Live WASAPI capture stream pushing float32 PCM into a lock-free `rtrb::Producer<f32>`.
pub struct WasapiCaptureStream {
    stop: Arc<AtomicBool>,
    sample_rate: u32,
    channels: u16,
    thread: Option<JoinHandle<()>>,
}

impl WasapiCaptureStream {
    /// Start capturing the default Microphone (`loopback = false`) or default System Audio
    /// (`loopback = true`, `AUDCLNT_STREAMFLAGS_LOOPBACK`).
    pub fn start_default(loopback: bool) -> Result<(Self, Consumer<f32>), MediaError> {
        let (tx, rx) = RingBuffer::<f32>::new(TARGET_SAMPLE_RATE as usize * 4);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let (info_tx, info_rx) = std::sync::mpsc::sync_channel::<Result<(u32, u16), MediaError>>(1);

        let handle = thread::Builder::new()
            .name(if loopback {
                "snagsoup-wasapi-loopback".into()
            } else {
                "snagsoup-wasapi-mic".into()
            })
            .spawn(move || wasapi_capture_thread(loopback, tx, stop_bg, info_tx))
            .map_err(|e| MediaError::Io(e.to_string()))?;

        let (sample_rate, channels) = info_rx
            .recv()
            .map_err(|_| MediaError::Io("WASAPI thread exited during init".into()))??;

        Ok((
            Self {
                stop,
                sample_rate,
                channels,
                thread: Some(handle),
            },
            rx,
        ))
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }
}

impl Drop for WasapiCaptureStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

fn wasapi_capture_thread(
    loopback: bool,
    mut tx: Producer<f32>,
    stop: Arc<AtomicBool>,
    info_tx: std::sync::mpsc::SyncSender<Result<(u32, u16), MediaError>>,
) {
    let co_init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let init = (|| -> Result<(IAudioClient, IAudioCaptureClient, u32, u16), MediaError> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| {
                MediaError::Wasapi {
                    api: "MMDeviceEnumerator",
                    hresult: e.code().0,
                }
            })?;
        let flow = if loopback { eRender } else { eCapture };
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }.map_err(|e| {
            MediaError::Wasapi {
                api: "GetDefaultAudioEndpoint",
                hresult: e.code().0,
            }
        })?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|e| {
            MediaError::Wasapi {
                api: "IMMDevice::Activate(IAudioClient)",
                hresult: e.code().0,
            }
        })?;
        let fmt_ptr: *mut WAVEFORMATEX = unsafe { client.GetMixFormat() }.map_err(|e| {
            MediaError::Wasapi {
                api: "IAudioClient::GetMixFormat",
                hresult: e.code().0,
            }
        })?;
        let rate = unsafe { (*fmt_ptr).nSamplesPerSec };
        let channels = unsafe { (*fmt_ptr).nChannels };
        let flags = if loopback {
            AUDCLNT_STREAMFLAGS_LOOPBACK
        } else {
            0
        };
        let init_res = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                1_000_000, // 100ms buffer
                0,
                fmt_ptr,
                None,
            )
        };
        unsafe { CoTaskMemFree(Some(fmt_ptr as *const _)) };
        init_res.map_err(|e| MediaError::Wasapi {
            api: "IAudioClient::Initialize",
            hresult: e.code().0,
        })?;

        let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(|e| {
            MediaError::Wasapi {
                api: "IAudioClient::GetService(IAudioCaptureClient)",
                hresult: e.code().0,
            }
        })?;
        unsafe { client.Start() }.map_err(|e| MediaError::Wasapi {
            api: "IAudioClient::Start",
            hresult: e.code().0,
        })?;
        Ok((client, capture, rate, channels))
    })();

    let (client, capture, rate, channels) = match init {
        Ok(v) => {
            let _ = info_tx.send(Ok((v.2, v.3)));
            v
        }
        Err(e) => {
            let _ = info_tx.send(Err(e));
            if co_init.is_ok() {
                unsafe { CoUninitialize() };
            }
            return;
        }
    };

    let ch = channels as usize;
    let _ = rate;
    while !stop.load(Ordering::Relaxed) {
        let Ok(mut packet_len) = (unsafe { capture.GetNextPacketSize() }) else {
            break;
        };
        if packet_len == 0 {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        while packet_len > 0 && !stop.load(Ordering::Relaxed) {
            let mut data_ptr: *mut u8 = std::ptr::null_mut();
            let mut num_frames = 0u32;
            let mut flags = 0u32;
            if unsafe {
                capture
                    .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                    .is_err()
            } {
                break;
            }
            let sample_count = (num_frames as usize) * ch;
            let is_silent = (flags & 0x2) != 0 || data_ptr.is_null(); // AUDCLNT_BUFFERFLAGS_SILENT
            if let Ok(mut chunk) = tx.write_chunk(sample_count.min(tx.slots())) {
                let (s1, s2) = chunk.as_mut_slices();
                if is_silent {
                    s1.fill(0.0);
                    s2.fill(0.0);
                } else {
                    let src = unsafe { std::slice::from_raw_parts(data_ptr as *const f32, sample_count) };
                    let n1 = s1.len().min(src.len());
                    s1[..n1].copy_from_slice(&src[..n1]);
                    let rem = src.len().saturating_sub(n1);
                    let n2 = s2.len().min(rem);
                    s2[..n2].copy_from_slice(&src[n1..n1 + n2]);
                }
                chunk.commit_all();
            }
            let _ = unsafe { capture.ReleaseBuffer(num_frames) };
            packet_len = unsafe { capture.GetNextPacketSize() }.unwrap_or(0);
        }
    }

    let _ = unsafe { client.Stop() };
    if co_init.is_ok() {
        unsafe { CoUninitialize() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_resamples_44100_mono_and_48000_stereo_with_soft_limiter() {
        let (mut mic_tx, mic_rx) = RingBuffer::<f32>::new(8192);
        let (mut sys_tx, sys_rx) = RingBuffer::<f32>::new(8192);

        // Push 44.1 kHz mono hot sine wave (+0.8 amplitude)
        for i in 0..441 {
            let phase = 2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 44_100.0;
            let _ = mic_tx.push(0.8 * phase.sin());
        }
        // Push 48 kHz stereo hot signal (+0.8 amplitude) so sum peaks at 1.6 -> must soft-limit < 1.0!
        for i in 0..480 {
            let phase = 2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 48_000.0;
            let v = 0.8 * phase.sin();
            let _ = sys_tx.push(v);
            let _ = sys_tx.push(v);
        }

        let mut mixer = LockFreeAudioMixer::new(Some((mic_rx, 44_100, 1)), Some((sys_rx, 48_000, 2)));
        let mut out = vec![0.0f32; 480 * 2];
        mixer.mix_block(&mut out);

        assert!(out.iter().all(|&s| s.abs() < 1.0), "soft limiter must prevent hard clipping");
        assert!(mixer.master_meter.peak() > 0.5);
        assert!(mixer.mic_meter.rms() > 0.1);
        assert!(mixer.sys_meter.rms() > 0.1);

        // Subsequent block with silent loopback/mic must smoothly fade to silence without stalling!
        let mut quiet = vec![1.0f32; 480 * 2];
        mixer.mix_block(&mut quiet);
        assert!(quiet[4..].iter().all(|&s| s.abs() < 1e-4));
    }

    #[test]
    fn enumerate_wasapi_devices_does_not_crash() {
        let devices = enumerate_audio_devices().expect("WASAPI enumeration should succeed");
        // Verify any returned devices have non-empty IDs.
        for d in &devices {
            assert!(!d.id.is_empty());
        }
    }
}
