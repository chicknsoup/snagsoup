//! Hardware-accelerated H.264 MP4 recorder backed by Windows Media Foundation
//! (`IMFSinkWriter` with `MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS = TRUE` + automatic
//! software H.264 fallback), D3D11 video device (`D3D11_CREATE_DEVICE_VIDEO_SUPPORT`),
//! and seamless `TimestampOffsetter` for Pause / Resume without timestamp gaps.

use crate::MediaError;
use crate::nv12::{even_dimensions, rgba_to_nv12};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFAttributes, IMFDXGIDeviceManager, IMFMediaBuffer, IMFMediaType, IMFSample,
    IMFSinkWriter, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MFAudioFormat_AAC, MFAudioFormat_PCM,
    MFCreate2DMediaBuffer, MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Audio,
    MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::{HSTRING, Interface};

/// 100-nanosecond ticks per second (Windows Media Foundation time unit).
pub const HNS_PER_SECOND: i64 = 10_000_000;

/// Pack two `u32` values (e.g., width/height or numerator/denominator) into a Media Foundation `u64`.
#[inline]
fn pack_u64(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | (lo as u64)
}

/// Tracks recording wall-clock timestamps across arbitrary `Pause` / `Resume` intervals
/// so output MP4 timestamps remain continuous without gaps.
#[derive(Debug, Clone)]
pub struct TimestampOffsetter {
    start_instant: Instant,
    paused_at: Option<Instant>,
    accumulated_pause: Duration,
}

impl TimestampOffsetter {
    pub fn start_now() -> Self {
        Self {
            start_instant: Instant::now(),
            paused_at: None,
            accumulated_pause: Duration::ZERO,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
        }
    }

    pub fn resume(&mut self) {
        if let Some(p) = self.paused_at.take() {
            self.accumulated_pause += p.elapsed();
        }
    }

    /// Active (non-paused) elapsed duration since `start_now()`.
    pub fn active_elapsed(&self) -> Duration {
        let total = self.start_instant.elapsed();
        let current_pause = self
            .paused_at
            .map(|p| p.elapsed())
            .unwrap_or(Duration::ZERO);
        total.saturating_sub(self.accumulated_pause + current_pause)
    }

    /// Active elapsed timestamp in Media Foundation 100-nanosecond units.
    pub fn active_hns(&self) -> i64 {
        let d = self.active_elapsed();
        (d.as_secs() as i64) * HNS_PER_SECOND + (d.subsec_nanos() as i64) / 100
    }
}

/// Configuration for `Mp4SinkWriter`.
#[derive(Debug, Clone)]
pub struct Mp4RecorderConfig {
    pub output_path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_bitrate: u32,
    pub enable_hardware_transforms: bool,
    pub record_audio: bool,
    pub audio_sample_rate: u32,
    pub audio_channels: u32,
    pub audio_bitrate: u32,
}

impl Mp4RecorderConfig {
    pub fn new(output_path: impl Into<PathBuf>, width: u32, height: u32) -> Self {
        let (ew, eh) = even_dimensions(width, height);
        Self {
            output_path: output_path.into(),
            width: ew,
            height: eh,
            fps: 60,
            video_bitrate: 8_000_000,
            enable_hardware_transforms: true,
            record_audio: false,
            audio_sample_rate: 48_000,
            audio_channels: 2,
            audio_bitrate: 192_000,
        }
    }
}

/// Create a D3D11 device with `D3D11_CREATE_DEVICE_VIDEO_SUPPORT` and attach it to a
/// Media Foundation `IMFDXGIDeviceManager` for zero-copy hardware encoding.
pub fn create_d3d11_video_device() -> Result<(ID3D11Device, IMFDXGIDeviceManager), MediaError> {
    let mut device: Option<ID3D11Device> = None;
    let feature_levels = [D3D_FEATURE_LEVEL_11_0];
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }
    .map_err(|e| MediaError::MediaFoundation {
        api: "D3D11CreateDevice(VIDEO_SUPPORT)",
        hresult: e.code().0,
    })?;
    let device = device.ok_or(MediaError::MediaFoundation {
        api: "D3D11CreateDevice",
        hresult: -1,
    })?;

    let mut reset_token = 0u32;
    let mut manager: Option<IMFDXGIDeviceManager> = None;
    unsafe { MFCreateDXGIDeviceManager(&mut reset_token, &mut manager) }.map_err(|e| {
        MediaError::MediaFoundation {
            api: "MFCreateDXGIDeviceManager",
            hresult: e.code().0,
        }
    })?;
    let manager = manager.ok_or(MediaError::MediaFoundation {
        api: "MFCreateDXGIDeviceManager",
        hresult: -1,
    })?;
    unsafe { manager.ResetDevice(&device, reset_token) }.map_err(|e| {
        MediaError::MediaFoundation {
            api: "IMFDXGIDeviceManager::ResetDevice",
            hresult: e.code().0,
        }
    })?;

    Ok((device, manager))
}

/// Media Foundation H.264 + AAC MP4 writer with hardware-transform preference and
/// automatic software H.264 fallback.
pub struct Mp4SinkWriter {
    writer: IMFSinkWriter,
    video_stream_index: u32,
    audio_stream_index: Option<u32>,
    width: u32,
    height: u32,
    frame_duration_hns: i64,
    used_hardware: bool,
    finalized: bool,
    com_initialized: bool,
}

impl Mp4SinkWriter {
    /// Open and start an MP4 `IMFSinkWriter` for `config`.
    pub fn new(config: &Mp4RecorderConfig) -> Result<Self, MediaError> {
        let (width, height) = even_dimensions(config.width, config.height);
        let fps = config.fps.max(1);
        let frame_duration_hns = HNS_PER_SECOND / (fps as i64);

        let com_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let com_initialized = com_hr.is_ok();

        if let Err(e) = crate::ensure_mf_started() {
            if com_initialized {
                unsafe { CoUninitialize() };
            }
            return Err(e);
        }

        // Try hardware transforms first if requested, falling back cleanly to software H.264.
        let mut try_modes = Vec::new();
        if config.enable_hardware_transforms {
            try_modes.push(true);
        }
        try_modes.push(false);

        let mut last_err = None;
        for use_hw in try_modes {
            match Self::try_init_writer(config, width, height, fps, use_hw) {
                Ok((writer, video_stream_index, audio_stream_index)) => {
                    return Ok(Self {
                        writer,
                        video_stream_index,
                        audio_stream_index,
                        width,
                        height,
                        frame_duration_hns,
                        used_hardware: use_hw,
                        finalized: false,
                        com_initialized,
                    });
                }
                Err(e) => last_err = Some(e),
            }
        }

        if com_initialized {
            unsafe { CoUninitialize() };
        }
        Err(last_err.unwrap_or_else(|| MediaError::InvalidConfig("failed to init sink writer".into())))
    }

    fn try_init_writer(
        config: &Mp4RecorderConfig,
        width: u32,
        height: u32,
        fps: u32,
        use_hw: bool,
    ) -> Result<(IMFSinkWriter, u32, Option<u32>), MediaError> {
        let mut attrs: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attrs, 2) }.map_err(|e| MediaError::MediaFoundation {
            api: "MFCreateAttributes",
            hresult: e.code().0,
        })?;
        let attrs = attrs.unwrap();
        unsafe {
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, if use_hw { 1 } else { 0 })
        }
        .map_err(|e| MediaError::MediaFoundation {
            api: "SetUINT32(HARDWARE_TRANSFORMS)",
            hresult: e.code().0,
        })?;

        let url = HSTRING::from(config.output_path.as_os_str());
        let writer = unsafe { MFCreateSinkWriterFromURL(&url, None, &attrs) }.map_err(|e| {
            MediaError::MediaFoundation {
                api: "MFCreateSinkWriterFromURL",
                hresult: e.code().0,
            }
        })?;

        // 1. Configure H.264 output video stream
        let out_video: IMFMediaType =
            unsafe { MFCreateMediaType() }.map_err(|e| MediaError::MediaFoundation {
                api: "MFCreateMediaType(out_video)",
                hresult: e.code().0,
            })?;
        (|| -> windows::core::Result<()> {
            unsafe {
                out_video.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                out_video.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
                out_video.SetUINT32(&MF_MT_AVG_BITRATE, config.video_bitrate)?;
                out_video.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                out_video.SetUINT64(&MF_MT_FRAME_SIZE, pack_u64(width, height))?;
                out_video.SetUINT64(&MF_MT_FRAME_RATE, pack_u64(fps, 1))?;
                out_video.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u64(1, 1))?;
            }
            Ok(())
        })()
        .map_err(|e| MediaError::MediaFoundation {
            api: "configure out_video",
            hresult: e.code().0,
        })?;

        let video_stream_index = unsafe { writer.AddStream(&out_video) }.map_err(|e| {
            MediaError::MediaFoundation {
                api: "AddStream(video)",
                hresult: e.code().0,
            }
        })?;

        // 2. Configure NV12 input video stream
        let in_video: IMFMediaType =
            unsafe { MFCreateMediaType() }.map_err(|e| MediaError::MediaFoundation {
                api: "MFCreateMediaType(in_video)",
                hresult: e.code().0,
            })?;
        (|| -> windows::core::Result<()> {
            unsafe {
                in_video.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                in_video.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                in_video.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                in_video.SetUINT64(&MF_MT_FRAME_SIZE, pack_u64(width, height))?;
                in_video.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
                in_video.SetUINT64(&MF_MT_FRAME_RATE, pack_u64(fps, 1))?;
                in_video.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u64(1, 1))?;
            }
            Ok(())
        })()
        .map_err(|e| MediaError::MediaFoundation {
            api: "configure in_video",
            hresult: e.code().0,
        })?;

        unsafe { writer.SetInputMediaType(video_stream_index, &in_video, None) }.map_err(|e| {
            MediaError::MediaFoundation {
                api: "SetInputMediaType(video)",
                hresult: e.code().0,
            }
        })?;

        // 3. Optional AAC audio stream (48 kHz Stereo, 192 kbps)
        let audio_stream_index = if config.record_audio {
            let out_audio: IMFMediaType =
                unsafe { MFCreateMediaType() }.map_err(|e| MediaError::MediaFoundation {
                    api: "MFCreateMediaType(out_audio)",
                    hresult: e.code().0,
                })?;
            (|| -> windows::core::Result<()> {
                unsafe {
                    out_audio.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                    out_audio.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
                    out_audio.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                    out_audio.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, config.audio_sample_rate)?;
                    out_audio.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, config.audio_channels)?;
                    out_audio.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, config.audio_bitrate / 8)?;
                }
                Ok(())
            })()
            .map_err(|e| MediaError::MediaFoundation {
                api: "configure out_audio",
                hresult: e.code().0,
            })?;

            let stream_idx = unsafe { writer.AddStream(&out_audio) }.map_err(|e| {
                MediaError::MediaFoundation {
                    api: "AddStream(audio)",
                    hresult: e.code().0,
                }
            })?;

            let in_audio: IMFMediaType =
                unsafe { MFCreateMediaType() }.map_err(|e| MediaError::MediaFoundation {
                    api: "MFCreateMediaType(in_audio)",
                    hresult: e.code().0,
                })?;
            (|| -> windows::core::Result<()> {
                unsafe {
                    in_audio.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                    in_audio.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                    in_audio.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                    in_audio.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, config.audio_sample_rate)?;
                    in_audio.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, config.audio_channels)?;
                }
                Ok(())
            })()
            .map_err(|e| MediaError::MediaFoundation {
                api: "configure in_audio",
                hresult: e.code().0,
            })?;

            unsafe { writer.SetInputMediaType(stream_idx, &in_audio, None) }.map_err(|e| {
                MediaError::MediaFoundation {
                    api: "SetInputMediaType(audio)",
                    hresult: e.code().0,
                }
            })?;
            Some(stream_idx)
        } else {
            None
        };

        unsafe { writer.BeginWriting() }.map_err(|e| MediaError::MediaFoundation {
            api: "BeginWriting",
            hresult: e.code().0,
        })?;

        Ok((writer, video_stream_index, audio_stream_index))
    }

    pub fn used_hardware_encoder(&self) -> bool {
        self.used_hardware
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Write a single RGBA8 frame at `timestamp_hns` (converts to NV12 internally).
    pub fn write_video_frame_rgba(
        &self,
        rgba: &[u8],
        src_w: u32,
        src_h: u32,
        timestamp_hns: i64,
    ) -> Result<(), MediaError> {
        let nv12 = rgba_to_nv12(rgba, src_w, src_h);
        self.write_video_frame_nv12(&nv12, timestamp_hns, self.frame_duration_hns)
    }

    /// Write a pre-converted tightly-packed NV12 frame buffer (`width * height * 3 / 2`) into
    /// the H.264 stream using `MFCreate2DMediaBuffer` + `IMF2DBuffer::ContiguousCopyFrom` so
    /// hardware encoders (AMD AMF / Intel QSV / NVENC) receive exact 2D pitch alignment even
    /// when `width` is not a multiple of 16 (e.g. `652 × 422`).
    pub fn write_video_frame_nv12(
        &self,
        nv12: &[u8],
        timestamp_hns: i64,
        duration_hns: i64,
    ) -> Result<(), MediaError> {
        let sample = create_nv12_2d_sample(self.width, self.height, nv12, timestamp_hns, duration_hns)?;
        unsafe { self.writer.WriteSample(self.video_stream_index, &sample) }.map_err(|e| {
            MediaError::MediaFoundation {
                api: "WriteSample(video)",
                hresult: e.code().0,
            }
        })
    }

    /// Write interleaved stereo float32 PCM (`48 kHz`) into the AAC audio stream.
    pub fn write_audio_f32_stereo(
        &self,
        samples_f32: &[f32],
        sample_rate: u32,
        timestamp_hns: i64,
    ) -> Result<(), MediaError> {
        let Some(stream_idx) = self.audio_stream_index else {
            return Ok(());
        };
        let mut pcm_bytes = Vec::with_capacity(samples_f32.len() * 2);
        for &s in samples_f32 {
            let clamped = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            pcm_bytes.extend_from_slice(&clamped.to_le_bytes());
        }
        let frames = (samples_f32.len() / 2) as i64;
        let duration_hns = (frames * HNS_PER_SECOND) / (sample_rate.max(1) as i64);
        let sample = create_mf_sample(&pcm_bytes, timestamp_hns, duration_hns)?;
        unsafe { self.writer.WriteSample(stream_idx, &sample) }.map_err(|e| {
            MediaError::MediaFoundation {
                api: "WriteSample(audio)",
                hresult: e.code().0,
            }
        })
    }

    /// Finalize the MP4 container (`moov` atom).
    pub fn finalize(mut self) -> Result<(), MediaError> {
        self.finalized = true;
        unsafe { self.writer.Finalize() }.map_err(|e| MediaError::MediaFoundation {
            api: "Finalize",
            hresult: e.code().0,
        })
    }
}

impl Drop for Mp4SinkWriter {
    fn drop(&mut self) {
        if !self.finalized {
            let _ = unsafe { self.writer.Finalize() };
        }
        if self.com_initialized {
            unsafe { CoUninitialize() };
        }
    }
}

fn create_nv12_2d_sample(
    width: u32,
    height: u32,
    nv12: &[u8],
    timestamp_hns: i64,
    duration_hns: i64,
) -> Result<IMFSample, MediaError> {
    const FOURCC_NV12: u32 = u32::from_le_bytes(*b"NV12");
    if let Ok(buffer) = unsafe { MFCreate2DMediaBuffer(width, height, FOURCC_NV12, false) } {
        if let Ok(buf2d) = buffer.cast::<IMF2DBuffer>() {
            if unsafe { buf2d.ContiguousCopyFrom(nv12) }.is_ok() {
                let sample: IMFSample =
                    unsafe { MFCreateSample() }.map_err(|e| MediaError::MediaFoundation {
                        api: "MFCreateSample",
                        hresult: e.code().0,
                    })?;
                (|| -> windows::core::Result<()> {
                    unsafe {
                        sample.AddBuffer(&buffer)?;
                        sample.SetSampleTime(timestamp_hns)?;
                        sample.SetSampleDuration(duration_hns)?;
                    }
                    Ok(())
                })()
                .map_err(|e| MediaError::MediaFoundation {
                    api: "configure 2D IMFSample",
                    hresult: e.code().0,
                })?;
                return Ok(sample);
            }
        }
    }
    create_mf_sample(nv12, timestamp_hns, duration_hns)
}

fn create_mf_sample(
    data: &[u8],
    timestamp_hns: i64,
    duration_hns: i64,
) -> Result<IMFSample, MediaError> {
    let buffer: IMFMediaBuffer = unsafe { MFCreateMemoryBuffer(data.len() as u32) }.map_err(
        |e| MediaError::MediaFoundation {
            api: "MFCreateMemoryBuffer",
            hresult: e.code().0,
        },
    )?;

    let mut dst_ptr: *mut u8 = std::ptr::null_mut();
    unsafe { buffer.Lock(&mut dst_ptr, None, None) }.map_err(|e| MediaError::MediaFoundation {
        api: "IMFMediaBuffer::Lock",
        hresult: e.code().0,
    })?;
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), dst_ptr, data.len());
        let _ = buffer.Unlock();
        buffer.SetCurrentLength(data.len() as u32)
    }
    .map_err(|e| MediaError::MediaFoundation {
        api: "IMFMediaBuffer::SetCurrentLength",
        hresult: e.code().0,
    })?;

    let sample: IMFSample = unsafe { MFCreateSample() }.map_err(|e| MediaError::MediaFoundation {
        api: "MFCreateSample",
        hresult: e.code().0,
    })?;
    (|| -> windows::core::Result<()> {
        unsafe {
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(timestamp_hns)?;
            sample.SetSampleDuration(duration_hns)?;
        }
        Ok(())
    })()
    .map_err(|e| MediaError::MediaFoundation {
        api: "configure IMFSample",
        hresult: e.code().0,
    })?;

    Ok(sample)
}

/// Quick MP4 container header validation (`ftyp` box present).
pub fn is_valid_mp4_file(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    bytes.len() >= 64 && bytes.windows(4).any(|w| w == b"ftyp") && bytes.windows(4).any(|w| w == b"moov")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn timestamp_offsetter_excludes_paused_duration() {
        let mut ts = TimestampOffsetter::start_now();
        thread::sleep(Duration::from_millis(60));
        ts.pause();
        let at_pause = ts.active_elapsed();
        thread::sleep(Duration::from_millis(120));
        assert!(ts.is_paused());
        assert!((ts.active_elapsed().as_millis() as i64 - at_pause.as_millis() as i64).abs() <= 10);
        ts.resume();
        thread::sleep(Duration::from_millis(60));
        let total_active = ts.active_elapsed();
        // Should be ~120ms active, NOT ~240ms wall-clock
        assert!(
            total_active < Duration::from_millis(200),
            "active duration {total_active:?} should exclude 120ms pause"
        );
    }

    #[test]
    fn record_mp4_h264_with_pause_produces_valid_mp4() {
        let out_path = std::env::temp_dir().join("snagsoup_w1_10_test.mp4");
        let _ = std::fs::remove_file(&out_path);

        let mut cfg = Mp4RecorderConfig::new(&out_path, 320, 180);
        cfg.fps = 30;
        cfg.video_bitrate = 1_000_000;
        cfg.record_audio = true;

        let writer = Mp4SinkWriter::new(&cfg).expect("Mp4SinkWriter should initialize");
        let mut rgba = vec![0u8; 320 * 180 * 4];
        let frame_dur = HNS_PER_SECOND / 30;

        // Write 30 frames (1s), pause conceptually, then write 30 more frames (total 2.0s MP4).
        for i in 0..60i64 {
            let shade = ((i * 4) % 255) as u8;
            for px in rgba.chunks_exact_mut(4) {
                px[0] = shade;
                px[1] = 128;
                px[2] = 255 - shade;
                px[3] = 255;
            }
            writer
                .write_video_frame_rgba(&rgba, 320, 180, i * frame_dur)
                .expect("write video frame");

            // 1600 stereo samples = 33.33ms @ 48kHz
            let audio_chunk = vec![0.05f32; 1600 * 2];
            writer
                .write_audio_f32_stereo(&audio_chunk, 48_000, i * frame_dur)
                .expect("write audio chunk");
        }

        writer.finalize().expect("finalize MP4");
        assert!(
            is_valid_mp4_file(&out_path),
            "output MP4 should contain valid ftyp + moov boxes"
        );
        let _ = std::fs::remove_file(&out_path);
    }
}
