//! Crash-safe recording & `DisasterRecoveryManager`:
//! Records chunked MP4 segments with an atomic `.recovery.json` journal so if the process
//! crashes or loses power without calling `finish()`, `recover_unfinished_recordings()`
//! stitches all committed segments into a clean playable `.mp4`.

use crate::MediaError;
use crate::player_decoder::decode_video_frames_range;
use crate::recorder::{HNS_PER_SECOND, Mp4RecorderConfig, Mp4SinkWriter};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Atomic JSON journal written alongside crash-safe recording segments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryJournal {
    pub recording_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub final_output: PathBuf,
    pub segments: Vec<PathBuf>,
    pub completed: bool,
}

/// Summary of a recording recovered by `DisasterRecoveryManager`.
#[derive(Debug, Clone)]
pub struct RecoveredRecording {
    pub recording_id: String,
    pub output_path: PathBuf,
    pub recovered_frames: usize,
}

/// Crash-safe recorder that rotates finalized MP4 segments every `segment_frames` frames
/// and updates `.recovery.json` atomically.
pub struct CrashSafeRecorder {
    journal_path: PathBuf,
    journal: RecoveryJournal,
    segment_dir: PathBuf,
    segment_frames: usize,
    frames_in_current_seg: usize,
    current_writer: Option<Mp4SinkWriter>,
}

impl CrashSafeRecorder {
    /// Start a new crash-safe recording under `recovery_root`.
    pub fn start(
        recovery_root: &Path,
        recording_id: impl Into<String>,
        final_output: impl Into<PathBuf>,
        width: u32,
        height: u32,
        fps: u32,
        segment_duration_secs: u32,
    ) -> Result<Self, MediaError> {
        let recording_id = recording_id.into();
        let segment_dir = recovery_root.join(&recording_id);
        fs::create_dir_all(&segment_dir).map_err(|e| MediaError::Io(e.to_string()))?;

        let journal_path = recovery_root.join(format!("{recording_id}.recovery.json"));
        let journal = RecoveryJournal {
            recording_id,
            width,
            height,
            fps: fps.max(1),
            final_output: final_output.into(),
            segments: Vec::new(),
            completed: false,
        };
        let mut rec = Self {
            journal_path,
            journal,
            segment_dir,
            segment_frames: (fps.max(1) * segment_duration_secs.max(1)) as usize,
            frames_in_current_seg: 0,
            current_writer: None,
        };
        rec.write_journal_atomic()?;
        rec.open_next_segment()?;
        Ok(rec)
    }

    /// Write an RGBA8 frame; automatically commits a durable segment boundary every
    /// `segment_duration_secs` seconds.
    pub fn write_frame_rgba(&mut self, rgba: &[u8]) -> Result<(), MediaError> {
        if self.current_writer.is_none() {
            self.open_next_segment()?;
        }
        let frame_dur = HNS_PER_SECOND / (self.journal.fps as i64);
        let ts = (self.frames_in_current_seg as i64) * frame_dur;
        self.current_writer
            .as_ref()
            .unwrap()
            .write_video_frame_rgba(rgba, self.journal.width, self.journal.height, ts)?;
        self.frames_in_current_seg += 1;

        if self.frames_in_current_seg >= self.segment_frames {
            self.commit_current_segment()?;
        }
        Ok(())
    }

    /// Cleanly finalize all segments into `final_output` and mark journal completed.
    pub fn finish(mut self) -> Result<PathBuf, MediaError> {
        self.commit_current_segment()?;
        let out = self.journal.final_output.clone();
        stitch_segments_into_mp4(&self.journal, &out)?;
        self.journal.completed = true;
        self.write_journal_atomic()?;
        let _ = fs::remove_file(&self.journal_path);
        let _ = fs::remove_dir_all(&self.segment_dir);
        Ok(out)
    }

    /// Simulate an abrupt crash: drops the active segment writer WITHOUT calling `finish()`,
    /// leaving `.recovery.json` (`completed: false`) and all previously committed segments intact.
    pub fn abort_simulate_crash(mut self) {
        let _ = self.current_writer.take();
    }

    fn open_next_segment(&mut self) -> Result<(), MediaError> {
        let idx = self.journal.segments.len();
        let seg_path = self.segment_dir.join(format!("seg_{idx:04}.mp4"));
        let mut cfg = Mp4RecorderConfig::new(&seg_path, self.journal.width, self.journal.height);
        cfg.fps = self.journal.fps;
        cfg.video_bitrate = 2_000_000;
        let writer = Mp4SinkWriter::new(&cfg)?;
        self.current_writer = Some(writer);
        self.frames_in_current_seg = 0;
        Ok(())
    }

    fn commit_current_segment(&mut self) -> Result<(), MediaError> {
        if let Some(writer) = self.current_writer.take() {
            if self.frames_in_current_seg > 0 {
                let idx = self.journal.segments.len();
                let seg_path = self.segment_dir.join(format!("seg_{idx:04}.mp4"));
                writer.finalize()?;
                self.journal.segments.push(seg_path);
                self.write_journal_atomic()?;
            }
            self.frames_in_current_seg = 0;
        }
        Ok(())
    }

    fn write_journal_atomic(&self) -> Result<(), MediaError> {
        let tmp = self.journal_path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(&self.journal)
            .map_err(|e| MediaError::Io(e.to_string()))?;
        fs::write(&tmp, data).map_err(|e| MediaError::Io(e.to_string()))?;
        fs::rename(&tmp, &self.journal_path).map_err(|e| MediaError::Io(e.to_string()))?;
        Ok(())
    }
}

/// Scans a recovery folder for unfinished `.recovery.json` journals and reconstructs
/// playable `.mp4` files from all intact segments.
pub struct DisasterRecoveryManager;

impl DisasterRecoveryManager {
    pub fn recover_unfinished_recordings(
        recovery_root: &Path,
    ) -> Result<Vec<RecoveredRecording>, MediaError> {
        let mut recovered = Vec::new();
        let Ok(entries) = fs::read_dir(recovery_root) else {
            return Ok(recovered);
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".recovery.json"))
            {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut journal) = serde_json::from_str::<RecoveryJournal>(&text) else {
                continue;
            };
            if journal.completed || journal.segments.is_empty() {
                continue;
            }

            let out_path = journal.final_output.clone();
            let frame_count = stitch_segments_into_mp4(&journal, &out_path)?;
            journal.completed = true;
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir_all(recovery_root.join(&journal.recording_id));

            recovered.push(RecoveredRecording {
                recording_id: journal.recording_id,
                output_path: out_path,
                recovered_frames: frame_count,
            });
        }

        Ok(recovered)
    }
}

fn stitch_segments_into_mp4(
    journal: &RecoveryJournal,
    output_path: &Path,
) -> Result<usize, MediaError> {
    let mut cfg = Mp4RecorderConfig::new(output_path, journal.width, journal.height);
    cfg.fps = journal.fps;
    cfg.video_bitrate = 2_000_000;
    let writer = Mp4SinkWriter::new(&cfg)?;

    let frame_dur = HNS_PER_SECOND / (journal.fps.max(1) as i64);
    let mut out_idx = 0i64;

    for seg in &journal.segments {
        if !seg.exists() {
            continue;
        }
        if let Ok(frames) = decode_video_frames_range(seg, 0, u64::MAX, 0) {
            for f in frames {
                writer.write_video_frame_rgba(&f.rgba, f.width, f.height, out_idx * frame_dur)?;
                out_idx += 1;
            }
        }
    }

    writer.finalize()?;
    Ok(out_idx as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_decoder::probe_video_metadata;

    #[test]
    fn simulated_crash_recovers_playable_mp4_via_source_reader() {
        let root = std::env::temp_dir().join(format!("snagsoup_w1_12_recovery_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let final_mp4 = root.join("recovered_clip.mp4");

        // Record 2 committed 1-second segments (15 fps * 2 = 30 frames) + 3 uncommitted frames, then crash!
        let mut rec = CrashSafeRecorder::start(&root, "rec-001", &final_mp4, 160, 120, 15, 1)
            .expect("start crash-safe recorder");
        let mut rgba = vec![0u8; 160 * 120 * 4];
        for i in 0..33u8 {
            for px in rgba.chunks_exact_mut(4) {
                px[0] = i.wrapping_mul(7);
                px[1] = 100;
                px[2] = 200;
                px[3] = 255;
            }
            rec.write_frame_rgba(&rgba).expect("write frame");
        }
        rec.abort_simulate_crash();

        // Run DisasterRecoveryManager -> must recover the 2s MP4 and be readable by IMFSourceReader!
        let recovered = DisasterRecoveryManager::recover_unfinished_recordings(&root)
            .expect("recovery should succeed");
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].recovered_frames >= 25);
        assert!(final_mp4.exists());

        let meta = probe_video_metadata(&final_mp4).expect("recovered MP4 must be readable");
        assert_eq!(meta.width, 160);
        assert_eq!(meta.height, 120);
        assert!(meta.duration_ms >= 1500, "recovered duration {} ms", meta.duration_ms);

        let _ = fs::remove_dir_all(&root);
    }
}
