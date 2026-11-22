//! `VideoFromImagesSession`: narrate over dynamically switched still images from the
//! Editor tray with live drawn annotation overlays, optional `WebcamOverlayMode`, and
//! optional `LockFreeAudioMixer` output to MP4.

use crate::MediaError;
use crate::audio_mixer::{LockFreeAudioMixer, TARGET_SAMPLE_RATE};
use crate::nv12::even_dimensions;
use crate::recorder::{HNS_PER_SECOND, Mp4RecorderConfig, Mp4SinkWriter};
use crate::webcam::{WebcamOverlayMode, composite_webcam_pip};
use std::path::PathBuf;

/// Session recording a narrated MP4 video from a sequence of still images + live overlays.
pub struct VideoFromImagesSession {
    writer: Mp4SinkWriter,
    width: u32,
    height: u32,
    fps: u32,
    frame_index: i64,
    audio_enabled: bool,
    pub webcam_mode: WebcamOverlayMode,
}

impl VideoFromImagesSession {
    pub fn start(
        output_path: impl Into<PathBuf>,
        width: u32,
        height: u32,
        fps: u32,
        record_audio: bool,
    ) -> Result<Self, MediaError> {
        let (ew, eh) = even_dimensions(width, height);
        let fps = fps.max(1);
        let mut cfg = Mp4RecorderConfig::new(output_path, ew, eh);
        cfg.fps = fps;
        cfg.record_audio = record_audio;
        let writer = Mp4SinkWriter::new(&cfg)?;
        Ok(Self {
            writer,
            width: ew,
            height: eh,
            fps,
            frame_index: 0,
            audio_enabled: record_audio,
            webcam_mode: WebcamOverlayMode::Hidden,
        })
    }

    /// Push a slide image (letterboxed/scaled to the session dimensions), optionally blending
    /// a live drawn RGBA overlay and webcam PiP, for `repeat_frames` frames.
    pub fn push_slide(
        &mut self,
        slide_rgba: &[u8],
        slide_w: u32,
        slide_h: u32,
        overlay_rgba: Option<&[u8]>,
        webcam_frame: Option<(&[u8], u32, u32)>,
        repeat_frames: u32,
        mut audio_mixer: Option<&mut LockFreeAudioMixer>,
    ) -> Result<(), MediaError> {
        let mut canvas = scale_rgba_nearest(slide_rgba, slide_w, slide_h, self.width, self.height);

        if let Some(ov) = overlay_rgba {
            if ov.len() == canvas.len() {
                alpha_blend_over(&mut canvas, ov);
            }
        }
        if let Some((cam, cw, ch)) = webcam_frame {
            composite_webcam_pip(&mut canvas, self.width, self.height, cam, cw, ch, self.webcam_mode);
        }

        let frame_dur = HNS_PER_SECOND / (self.fps as i64);
        let audio_samples_per_frame = (TARGET_SAMPLE_RATE / self.fps) as usize * 2;
        let mut audio_buf = vec![0.0f32; audio_samples_per_frame];

        for _ in 0..repeat_frames.max(1) {
            let ts = self.frame_index * frame_dur;
            self.writer
                .write_video_frame_rgba(&canvas, self.width, self.height, ts)?;
            if self.audio_enabled {
                if let Some(ref mut mixer) = audio_mixer {
                    mixer.mix_block(&mut audio_buf);
                } else {
                    audio_buf.fill(0.0);
                }
                self.writer
                    .write_audio_f32_stereo(&audio_buf, TARGET_SAMPLE_RATE, ts)?;
            }
            self.frame_index += 1;
        }
        Ok(())
    }

    pub fn finalize(self) -> Result<(), MediaError> {
        self.writer.finalize()
    }
}

fn scale_rgba_nearest(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut dst = vec![0u8; (dw as usize) * (dh as usize) * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return dst;
    }
    let sw_u = sw as usize;
    let sh_u = sh as usize;
    let dw_u = dw as usize;
    let dh_u = dh as usize;
    for y in 0..dh_u {
        let sy = (y * sh_u) / dh_u;
        for x in 0..dw_u {
            let sx = (x * sw_u) / dw_u;
            let s_idx = (sy * sw_u + sx) * 4;
            let d_idx = (y * dw_u + x) * 4;
            dst[d_idx..d_idx + 4].copy_from_slice(&src[s_idx..s_idx + 4]);
        }
    }
    dst
}

fn alpha_blend_over(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let a = s[3] as f32 / 255.0;
        if a <= 0.0 {
            continue;
        }
        if a >= 1.0 {
            d.copy_from_slice(s);
        } else {
            let inv = 1.0 - a;
            for c in 0..3 {
                d[c] = (s[c] as f32 * a + d[c] as f32 * inv).round() as u8;
            }
            d[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_decoder::probe_video_metadata;

    #[test]
    fn video_from_images_creates_valid_mp4_with_multiple_slides() {
        let out = std::env::temp_dir().join("snagsoup_w1_13_slides.mp4");
        let _ = std::fs::remove_file(&out);

        let mut sess = VideoFromImagesSession::start(&out, 160, 120, 15, false).unwrap();
        let slide1 = vec![220u8; 160 * 120 * 4];
        let slide2 = vec![60u8; 160 * 120 * 4];
        sess.push_slide(&slide1, 160, 120, None, None, 15, None).unwrap();
        sess.push_slide(&slide2, 160, 120, None, None, 15, None).unwrap();
        sess.finalize().unwrap();

        let meta = probe_video_metadata(&out).expect("probe slides mp4");
        assert_eq!(meta.width, 160);
        assert_eq!(meta.height, 120);
        assert!(meta.duration_ms >= 1500);
        let _ = std::fs::remove_file(&out);
    }
}
