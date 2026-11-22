//! Media Foundation `IMFSourceReader` video decoder for:
//! - Probing MP4 metadata (`width`, `height`, `duration_ms`, `fps`)
//! - Extracting RGBA8 frames at arbitrary timestamps (Editor Video Player, filmstrip, *Save Frame as PNG*)
//! - Decoding frame ranges at target FPS (for Video-to-GIF export)

use crate::MediaError;
use crate::recorder::HNS_PER_SECOND;
use std::path::Path;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFAttributes, IMFMediaType, IMFSourceReader, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_GEOMETRIC_APERTURE, MF_MT_MAJOR_TYPE,
    MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_SUBTYPE, MF_PD_DURATION,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFVideoArea, MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToInt64;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::{GUID, HSTRING, Interface, PROPVARIANT};

/// Probed metadata of an MP4 video file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub fps: f32,
}

/// A decoded RGBA8 video frame + presentation timestamp.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub timestamp_ms: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct MfGuard {
    com_init: bool,
}

impl MfGuard {
    fn start() -> Result<Self, MediaError> {
        let com_init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        if let Err(e) = crate::ensure_mf_started() {
            if com_init {
                unsafe { CoUninitialize() };
            }
            return Err(e);
        }
        Ok(Self { com_init })
    }
}

impl Drop for MfGuard {
    fn drop(&mut self) {
        if self.com_init {
            unsafe { CoUninitialize() };
        }
    }
}

fn unpack_u64(val: u64) -> (u32, u32) {
    ((val >> 32) as u32, (val & 0xFFFF_FFFF) as u32)
}

/// Read optional `MFVideoArea` aperture (`MF_MT_MINIMUM_DISPLAY_APERTURE` or `MF_MT_GEOMETRIC_APERTURE`)
/// so macroblock-padded H.264 streams (e.g. `656 × 432` coded for `652 × 422` display) report and crop
/// to the true clean display dimensions `(652, 422)`.
fn read_display_aperture(mt: &IMFMediaType) -> Option<(u32, u32)> {
    for key in [&MF_MT_MINIMUM_DISPLAY_APERTURE, &MF_MT_GEOMETRIC_APERTURE] {
        let mut area = MFVideoArea::default();
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                (&mut area as *mut MFVideoArea) as *mut u8,
                std::mem::size_of::<MFVideoArea>(),
            )
        };
        if unsafe { mt.GetBlob(key, bytes, None) }.is_ok() {
            let aw = area.Area.cx;
            let ah = area.Area.cy;
            if aw > 0 && ah > 0 {
                return Some((aw as u32, ah as u32));
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Rgb32Layout {
    pub disp_w: u32,
    pub disp_h: u32,
    pub buf_h: u32,
    pub default_stride: i32,
}

fn query_rgb32_layout(
    reader: &IMFSourceReader,
    stream: u32,
    fallback_disp: (u32, u32),
) -> Rgb32Layout {
    if let Ok(cur) = unsafe { reader.GetCurrentMediaType(stream) } {
        let (bw, bh) = unsafe { cur.GetUINT64(&MF_MT_FRAME_SIZE) }
            .map(unpack_u64)
            .unwrap_or(fallback_disp);
        let (dw, dh) = read_display_aperture(&cur).unwrap_or(fallback_disp);
        let disp_w = dw.min(bw.max(1)).max(1);
        let disp_h = dh.min(bh.max(1)).max(1);
        let default_stride = unsafe { cur.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
            .map(|u| u as i32)
            .unwrap_or((bw.max(disp_w) as i32) * 4);
        Rgb32Layout {
            disp_w,
            disp_h,
            buf_h: bh.max(disp_h),
            default_stride,
        }
    } else {
        Rgb32Layout {
            disp_w: fallback_disp.0.max(1),
            disp_h: fallback_disp.1.max(1),
            buf_h: fallback_disp.1.max(1),
            default_stride: (fallback_disp.0.max(1) as i32) * 4,
        }
    }
}

pub(crate) fn open_rgb32_reader(
    path: &Path,
) -> Result<(IMFSourceReader, VideoMetadata, Rgb32Layout), MediaError> {
    let mut attrs: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attrs, 1) }.map_err(|e| MediaError::MediaFoundation {
        api: "MFCreateAttributes(reader)",
        hresult: e.code().0,
    })?;
    let attrs = attrs.unwrap();
    let _ = unsafe { attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1) };

    let url = HSTRING::from(path.as_os_str());
    let reader = unsafe { MFCreateSourceReaderFromURL(&url, &attrs) }.map_err(|e| {
        MediaError::MediaFoundation {
            api: "MFCreateSourceReaderFromURL",
            hresult: e.code().0,
        }
    })?;

    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let native_type: IMFMediaType = unsafe { reader.GetNativeMediaType(stream, 0) }.map_err(|e| {
        MediaError::MediaFoundation {
            api: "GetNativeMediaType",
            hresult: e.code().0,
        }
    })?;
    let frame_size = unsafe { native_type.GetUINT64(&MF_MT_FRAME_SIZE) }.unwrap_or(0);
    let (native_w, native_h) = unpack_u64(frame_size);
    let (disp_w, disp_h) = read_display_aperture(&native_type).unwrap_or((native_w, native_h));
    let width = disp_w.min(native_w.max(1)).max(1);
    let height = disp_h.min(native_h.max(1)).max(1);

    let fps_packed = unsafe { native_type.GetUINT64(&MF_MT_FRAME_RATE) }.unwrap_or(0);
    let (fps_num, fps_den) = unpack_u64(fps_packed);
    let fps = if fps_den > 0 {
        fps_num as f32 / fps_den as f32
    } else {
        30.0
    };

    let duration_hns: i64 = unsafe {
        reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ok()
            .and_then(|pv| PropVariantToInt64(&pv).ok())
            .unwrap_or(0)
    };
    let duration_ms = ((duration_hns.max(0) as u64) * 1000) / (HNS_PER_SECOND as u64);

    // Request RGB32 output from IMFSourceReader.
    let rgb_type: IMFMediaType =
        unsafe { MFCreateMediaType() }.map_err(|e| MediaError::MediaFoundation {
            api: "MFCreateMediaType(RGB32)",
            hresult: e.code().0,
        })?;
    (|| -> windows::core::Result<()> {
        unsafe {
            rgb_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            rgb_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        }
        Ok(())
    })()
    .map_err(|e| MediaError::MediaFoundation {
        api: "configure RGB32 type",
        hresult: e.code().0,
    })?;
    unsafe { reader.SetCurrentMediaType(stream, None, &rgb_type) }.map_err(|e| {
        MediaError::MediaFoundation {
            api: "SetCurrentMediaType(RGB32)",
            hresult: e.code().0,
        }
    })?;

    let layout = query_rgb32_layout(&reader, stream, (width, height));

    Ok((
        reader,
        VideoMetadata {
            width: layout.disp_w,
            height: layout.disp_h,
            duration_ms,
            fps,
        },
        layout,
    ))
}

/// Stateful sequential MP4 frame decoder keeping a single `IMFSourceReader` open across frames
/// for smooth real-time playback inside the Editor Video Player.
pub struct Mp4StreamDecoder {
    _mf: MfGuard,
    reader: IMFSourceReader,
    meta: VideoMetadata,
    layout: Rgb32Layout,
}

impl Mp4StreamDecoder {
    pub fn open(mp4_path: &Path) -> Result<Self, MediaError> {
        let mf = MfGuard::start()?;
        let (reader, meta, layout) = open_rgb32_reader(mp4_path)?;
        Ok(Self {
            _mf: mf,
            reader,
            meta,
            layout,
        })
    }

    pub fn metadata(&self) -> VideoMetadata {
        self.meta
    }

    pub fn seek_ms(&mut self, timestamp_ms: u64) {
        let pos_hns = (timestamp_ms as i64) * (HNS_PER_SECOND / 1000);
        let pos_var = PROPVARIANT::from(pos_hns);
        let _ = unsafe {
            self.reader
                .SetCurrentPosition(&GUID::zeroed(), &pos_var as *const PROPVARIANT)
        };
    }

    /// Read the next decoded video frame in presentation order (`Ok(None)` at end-of-stream).
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, MediaError> {
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        loop {
            let mut flags = 0u32;
            let mut ts_hns = 0i64;
            let mut sample = None;
            unsafe {
                self.reader.ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts_hns),
                    Some(&mut sample),
                )
            }
            .map_err(|e| MediaError::MediaFoundation {
                api: "IMFSourceReader::ReadSample",
                hresult: e.code().0,
            })?;

            if (flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32) != 0 {
                self.layout = query_rgb32_layout(
                    &self.reader,
                    stream,
                    (self.meta.width, self.meta.height),
                );
            }
            if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                return Ok(None);
            }
            if let Some(s) = sample {
                let rgba = sample_rgb32_to_rgba(&s, self.layout)?;
                return Ok(Some(DecodedFrame {
                    timestamp_ms: ((ts_hns.max(0) as u64) * 1000) / (HNS_PER_SECOND as u64),
                    width: self.layout.disp_w,
                    height: self.layout.disp_h,
                    rgba,
                }));
            }
        }
    }
}

/// Probe `width`, `height`, `duration_ms`, and `fps` of an MP4 file.
pub fn probe_video_metadata(mp4_path: &Path) -> Result<VideoMetadata, MediaError> {
    let _mf = MfGuard::start()?;
    let (_reader, meta, _layout) = open_rgb32_reader(mp4_path)?;
    Ok(meta)
}

/// Extract a single RGBA8 frame at or immediately after `timestamp_ms`.
pub fn extract_video_frame_rgba(
    mp4_path: &Path,
    timestamp_ms: u64,
) -> Result<DecodedFrame, MediaError> {
    let mut dec = Mp4StreamDecoder::open(mp4_path)?;
    if timestamp_ms > 0 {
        dec.seek_ms(timestamp_ms);
    }
    while let Some(frame) = dec.next_frame()? {
        if frame.timestamp_ms + 34 >= timestamp_ms || timestamp_ms == 0 {
            return Ok(frame);
        }
    }
    // Fallback to very first frame if seek landed right at EOF
    dec.seek_ms(0);
    dec.next_frame()?
        .ok_or_else(|| MediaError::Io("reached end of video stream before frame".into()))
}

/// Decode RGBA8 frames across `[start_ms..=end_ms]` sampled at `target_fps` (for Video-to-GIF
/// export or timeline filmstrip thumbnails).
pub fn decode_video_frames_range(
    mp4_path: &Path,
    start_ms: u64,
    end_ms: u64,
    target_fps: u32,
) -> Result<Vec<DecodedFrame>, MediaError> {
    let mut dec = Mp4StreamDecoder::open(mp4_path)?;
    if start_ms > 0 {
        dec.seek_ms(start_ms);
    }

    let step_ms = if target_fps > 0 {
        (1000 / target_fps as u64).max(1)
    } else {
        0
    };
    let mut next_keep_ms = start_ms;
    let mut frames = Vec::new();

    while let Some(frame) = dec.next_frame()? {
        if frame.timestamp_ms > end_ms {
            break;
        }
        if frame.timestamp_ms >= next_keep_ms {
            next_keep_ms = frame.timestamp_ms.saturating_add(step_ms);
            frames.push(frame);
        }
    }

    Ok(frames)
}

pub(crate) fn sample_rgb32_to_rgba(
    sample: &windows::Win32::Media::MediaFoundation::IMFSample,
    layout: Rgb32Layout,
) -> Result<Vec<u8>, MediaError> {
    let w = layout.disp_w as usize;
    let h = layout.disp_h as usize;
    let row_bytes = w * 4;
    let mut rgba = vec![0u8; row_bytes * h];

    // Fast & Pitch-Exact Path 1: `IMF2DBuffer::Lock2D` (respects exact hardware/macroblock 2D stride
    // such as 656*4 = 2624 bytes for a 652px wide video, as well as negative bottom-up DIB strides!).
    if let Ok(buf0) = unsafe { sample.GetBufferByIndex(0) } {
        if let Ok(buf2d) = buf0.cast::<IMF2DBuffer>() {
            let mut scanline0: *mut u8 = std::ptr::null_mut();
            let mut pitch: i32 = 0;
            if unsafe { buf2d.Lock2D(&mut scanline0, &mut pitch) }.is_ok() {
                if !scanline0.is_null() && pitch.unsigned_abs() as usize >= row_bytes {
                    for y in 0..h {
                        let src_row_ptr =
                            unsafe { scanline0.offset((y as isize) * (pitch as isize)) };
                        let src_row = unsafe { std::slice::from_raw_parts(src_row_ptr, row_bytes) };
                        let dst_row = &mut rgba[y * row_bytes..(y + 1) * row_bytes];
                        dst_row.copy_from_slice(src_row);
                    }
                }
                unsafe {
                    let _ = buf2d.Unlock2D();
                }
                for px in rgba.chunks_exact_mut(4) {
                    px.swap(0, 2);
                    px[3] = 255;
                }
                return Ok(rgba);
            }
        }
    }

    // Fallback Path 2: 1D `IMFMediaBuffer::Lock` using `MF_MT_DEFAULT_STRIDE` (or inferred stride
    // `cur_len / buf_h`) row-by-row — NEVER flat `copy_from_slice` when row pitch > `disp_w * 4`!
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(|e| {
        MediaError::MediaFoundation {
            api: "IMFSample::ConvertToContiguousBuffer",
            hresult: e.code().0,
        }
    })?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut cur_len = 0u32;
    unsafe { buffer.Lock(&mut ptr, None, Some(&mut cur_len)) }.map_err(|e| {
        MediaError::MediaFoundation {
            api: "IMFMediaBuffer::Lock",
            hresult: e.code().0,
        }
    })?;

    let total_len = cur_len as usize;
    let buf_h = (layout.buf_h as usize).max(h).max(1);
    let mut stride: isize = layout.default_stride as isize;
    let abs_default = stride.unsigned_abs();
    if abs_default < row_bytes || abs_default * h > total_len {
        let inferred = total_len / buf_h;
        stride = if inferred >= row_bytes {
            if layout.default_stride < 0 {
                -(inferred as isize)
            } else {
                inferred as isize
            }
        } else {
            row_bytes as isize
        };
    }

    let abs_stride = stride.unsigned_abs();
    if !ptr.is_null() && total_len >= row_bytes {
        for y in 0..h {
            let byte_off = if stride >= 0 {
                y * abs_stride
            } else {
                // Bottom-up DIB in 1D contiguous memory: row 0 (top) is near the end of the buffer
                (buf_h.saturating_sub(1).saturating_sub(y)) * abs_stride
            };
            if byte_off + row_bytes <= total_len {
                let src_row = unsafe { std::slice::from_raw_parts(ptr.add(byte_off), row_bytes) };
                rgba[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(src_row);
            }
        }
    }

    unsafe {
        let _ = buffer.Unlock();
    }

    // Swizzle BGRA/BGRX -> opaque RGBA8
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{Mp4RecorderConfig, Mp4SinkWriter};

    #[test]
    fn non_macroblock_652x422_encode_decode_has_zero_diagonal_stride_skew() {
        // Exact user bug dimensions: 652 × 422 (652 % 16 == 12, 422 % 16 == 6)!
        let w = 652u32;
        let h = 422u32;
        let out_mp4 =
            std::env::temp_dir().join(format!("snagsoup_stride_652x422_{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&out_mp4);

        let mut cfg = Mp4RecorderConfig::new(&out_mp4, w, h);
        cfg.fps = 30;
        cfg.video_bitrate = 4_000_000;
        cfg.enable_hardware_transforms = true;
        let writer = Mp4SinkWriter::new(&cfg).expect("Mp4SinkWriter 652x422");

        // Paint crisp vertical RGB bands:
        // x in 0..216   => Pure RED   (240, 20, 20)
        // x in 216..436 => Pure GREEN (20, 220, 20)
        // x in 436..652 => Pure BLUE  (20, 60, 240)
        // If there is ANY row-stride mismatch (like 656 vs 652 = 4px/row), by row y=300 the bands
        // would shift by 300*4 = 1200px (nearly 2 full widths!) and fail completely!
        let mut src_rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let off = ((y * w + x) * 4) as usize;
                let (r, g, b) = if x < 216 {
                    (240, 20, 20)
                } else if x < 436 {
                    (20, 220, 20)
                } else {
                    (20, 60, 240)
                };
                src_rgba[off] = r;
                src_rgba[off + 1] = g;
                src_rgba[off + 2] = b;
                src_rgba[off + 3] = 255;
            }
        }

        let frame_hns = HNS_PER_SECOND / 30;
        for i in 0..10i64 {
            writer
                .write_video_frame_rgba(&src_rgba, w, h, i * frame_hns)
                .expect("write 652x422 frame");
        }
        writer.finalize().expect("finalize 652x422 mp4");

        let meta = probe_video_metadata(&out_mp4).expect("probe 652x422");
        assert_eq!((meta.width, meta.height), (w, h));

        let decoded = extract_video_frame_rgba(&out_mp4, 0).expect("decode 652x422 frame 0");
        assert_eq!((decoded.width, decoded.height), (w, h));

        // Check vertical alignment at top (y=10), middle (y=210), and bottom (y=405)!
        for &y in &[10u32, 210, 405] {
            let px_at = |x: u32| -> (u8, u8, u8) {
                let i = ((y * w + x) * 4) as usize;
                (decoded.rgba[i], decoded.rgba[i + 1], decoded.rgba[i + 2])
            };
            let (r0, g0, b0) = px_at(100); // Must be RED at every single row y!
            assert!(
                r0 > 180 && g0 < 80 && b0 < 80,
                "row y={y} x=100 skewed! expected RED, got ({r0}, {g0}, {b0})"
            );
            let (r1, g1, b1) = px_at(320); // Must be GREEN at every single row y!
            assert!(
                g1 > 160 && r1 < 80 && b1 < 80,
                "row y={y} x=320 skewed! expected GREEN, got ({r1}, {g1}, {b1})"
            );
            let (r2, g2, b2) = px_at(550); // Must be BLUE at every single row y!
            assert!(
                b2 > 170 && r2 < 80 && g2 < 110,
                "row y={y} x=550 skewed! expected BLUE, got ({r2}, {g2}, {b2})"
            );
        }

        let _ = std::fs::remove_file(&out_mp4);
    }
}
