//! Multi-interval MP4 cutter (`IMFSourceReader` -> `IMFSinkWriter`) adjusting timestamps
//! by `PrecedingCutsDuration`.

use crate::MediaError;
use crate::player_decoder::{VideoMetadata, decode_video_frames_range, probe_video_metadata};
use crate::recorder::{HNS_PER_SECOND, Mp4RecorderConfig, Mp4SinkWriter};
use std::path::Path;

/// Half-open time interval `[start_ms, end_ms)` to cut out of a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutInterval {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl CutInterval {
    pub const fn new(start_ms: u64, end_ms: u64) -> Self {
        Self { start_ms, end_ms }
    }
}

/// Sort and merge overlapping or adjacent cut intervals.
pub fn merge_cut_intervals(cuts: &[CutInterval]) -> Vec<CutInterval> {
    let mut valid: Vec<CutInterval> = cuts
        .iter()
        .copied()
        .filter(|c| c.end_ms > c.start_ms)
        .collect();
    valid.sort_by_key(|c| c.start_ms);

    let mut merged: Vec<CutInterval> = Vec::new();
    for c in valid {
        if let Some(last) = merged.last_mut() {
            if c.start_ms <= last.end_ms {
                last.end_ms = last.end_ms.max(c.end_ms);
                continue;
            }
        }
        merged.push(c);
    }
    merged
}

/// Map a source timestamp `ts_ms` through sorted, merged `cuts`:
/// - Returns `None` if `ts_ms` lies inside a cut interval.
/// - Returns `Some(ts_ms - preceding_cuts_ms)` otherwise.
pub fn map_timestamp_with_cuts(ts_ms: u64, merged_cuts: &[CutInterval]) -> Option<u64> {
    let mut preceding = 0u64;
    for c in merged_cuts {
        if ts_ms < c.start_ms {
            break;
        }
        if ts_ms < c.end_ms {
            return None;
        }
        preceding += c.end_ms - c.start_ms;
    }
    Some(ts_ms - preceding)
}

/// Cut out all `cut_ranges_ms` intervals from `input_mp4` and write the shortened MP4 to
/// `output_mp4`, returning the probed metadata of `output_mp4`.
pub fn trim_mp4_intervals(
    input_mp4: &Path,
    cut_ranges_ms: &[CutInterval],
    output_mp4: &Path,
) -> Result<VideoMetadata, MediaError> {
    let meta = probe_video_metadata(input_mp4)?;
    let merged = merge_cut_intervals(cut_ranges_ms);
    let frames = decode_video_frames_range(input_mp4, 0, u64::MAX, 0)?;

    let fps = meta.fps.round().max(1.0) as u32;
    let mut cfg = Mp4RecorderConfig::new(output_mp4, meta.width, meta.height);
    cfg.fps = fps;
    cfg.video_bitrate = 2_000_000;
    let writer = Mp4SinkWriter::new(&cfg)?;

    let mut kept = 0usize;
    for f in &frames {
        if let Some(new_ms) = map_timestamp_with_cuts(f.timestamp_ms, &merged) {
            let new_hns = (new_ms as i64) * (HNS_PER_SECOND / 1000);
            writer.write_video_frame_rgba(&f.rgba, f.width, f.height, new_hns)?;
            kept += 1;
        }
    }
    if kept == 0 {
        return Err(MediaError::InvalidConfig(
            "all frames were removed by cut intervals".into(),
        ));
    }

    writer.finalize()?;
    probe_video_metadata(output_mp4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_decoder::extract_video_frame_rgba;

    #[test]
    fn merge_and_map_cut_intervals_math() {
        let merged = merge_cut_intervals(&[
            CutInterval::new(3000, 4000),
            CutInterval::new(1000, 2000),
            CutInterval::new(1800, 2200),
        ]);
        assert_eq!(
            merged,
            vec![CutInterval::new(1000, 2200), CutInterval::new(3000, 4000)]
        );
        assert_eq!(map_timestamp_with_cuts(500, &merged), Some(500));
        assert_eq!(map_timestamp_with_cuts(1500, &merged), None);
        assert_eq!(map_timestamp_with_cuts(2500, &merged), Some(1300));
        assert_eq!(map_timestamp_with_cuts(4500, &merged), Some(2300));
    }

    #[test]
    fn trim_mp4_removes_two_intervals_and_shortens_duration() {
        let dir = std::env::temp_dir().join(format!("snagsoup_w1_13_trim_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src_mp4 = dir.join("source_4s.mp4");
        let out_mp4 = dir.join("trimmed_2s.mp4");
        let _ = std::fs::remove_file(&src_mp4);
        let _ = std::fs::remove_file(&out_mp4);

        // Record a 4-second 15fps MP4 (60 frames)
        let mut cfg = Mp4RecorderConfig::new(&src_mp4, 160, 120);
        cfg.fps = 15;
        cfg.video_bitrate = 1_000_000;
        let writer = Mp4SinkWriter::new(&cfg).expect("create source writer");
        let frame_dur = HNS_PER_SECOND / 15;
        let mut rgba = vec![0u8; 160 * 120 * 4];
        for i in 0..60i64 {
            for px in rgba.chunks_exact_mut(4) {
                px[0] = (i * 4) as u8;
                px[1] = 150;
                px[2] = 60;
                px[3] = 255;
            }
            writer
                .write_video_frame_rgba(&rgba, 160, 120, i * frame_dur)
                .unwrap();
        }
        writer.finalize().unwrap();

        let src_meta = probe_video_metadata(&src_mp4).expect("probe source");
        assert!(src_meta.duration_ms >= 3500);

        // Cut 2 intervals: [500..1500) and [2500..3500) => ~2 seconds remaining!
        let out_meta = trim_mp4_intervals(
            &src_mp4,
            &[CutInterval::new(500, 1500), CutInterval::new(2500, 3500)],
            &out_mp4,
        )
        .expect("trim_mp4_intervals should succeed");

        assert!(
            out_meta.duration_ms >= 1500 && out_meta.duration_ms <= 2600,
            "expected ~2000ms trimmed duration, got {}ms",
            out_meta.duration_ms
        );

        // Also verify extract_video_frame_rgba works on the trimmed clip!
        let frame = extract_video_frame_rgba(&out_mp4, 500).expect("extract frame");
        assert_eq!(frame.width, 160);
        assert_eq!(frame.height, 120);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
