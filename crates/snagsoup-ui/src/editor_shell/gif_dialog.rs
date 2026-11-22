//! `CreateGifDialog` (`W3-12` Part B) wired to `snagsoup_library_export::AnimatedGifExporter`
//! supporting all 4 Snagit GIF presets (`ScreenVideo`, `HighMotionVideo`, `ReducedFileSize`,
//! `Custom`) + `Fade to black` transition frames.

use std::path::Path;

use snagsoup_library_export::{AnimatedGifExporter, GifExportReport, GifFrameInput};
use snagsoup_media::decode_video_frames_range;
use snagsoup_model::GifExportPreset;

/// Preset choice in Snagit's Create Animated GIF dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GifPresetChoice {
    #[default]
    ScreenVideo,
    HighMotionVideo,
    ReducedFileSize,
    Custom,
}

/// State and export action for `CreateGifDialog`.
#[derive(Debug, Clone)]
pub struct CreateGifDialogState {
    pub open: bool,
    pub choice: GifPresetChoice,
    pub preset: GifExportPreset,
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Default for CreateGifDialogState {
    fn default() -> Self {
        Self {
            open: false,
            choice: GifPresetChoice::ScreenVideo,
            preset: GifExportPreset::screen_video(),
            start_ms: 0,
            end_ms: 2000,
        }
    }
}

impl CreateGifDialogState {
    /// Switch to one of Snagit's 4 GIF optimization presets.
    pub fn select_preset(&mut self, choice: GifPresetChoice) {
        self.choice = choice;
        self.preset = match choice {
            GifPresetChoice::ScreenVideo => GifExportPreset::screen_video(),
            GifPresetChoice::HighMotionVideo => GifExportPreset::high_motion_video(),
            GifPresetChoice::ReducedFileSize => GifExportPreset::reduced_file_size(),
            GifPresetChoice::Custom => self.preset.clone(),
        };
    }

    pub fn set_fade_to_black(&mut self, enabled: bool) {
        self.preset.insert_transition_frames = enabled;
    }

    /// Decode `[start_ms, end_ms]` from `mp4_path` and export an optimized Animated GIF to `output_gif`.
    pub fn export_from_mp4(
        &self,
        mp4_path: &Path,
        output_gif: &Path,
    ) -> Result<GifExportReport, String> {
        let target_fps = self.preset.fps.max(1);
        let decoded = decode_video_frames_range(mp4_path, self.start_ms, self.end_ms, target_fps)
            .map_err(|e| format!("decode_video_frames_range failed: {e}"))?;

        if decoded.is_empty() {
            return Err("no video frames decoded in selected range".to_string());
        }

        let delay_cs = (100u16 / target_fps as u16).max(2);
        let gif_frames: Vec<GifFrameInput> = decoded
            .into_iter()
            .map(|f| GifFrameInput {
                width: f.width,
                height: f.height,
                rgba: f.rgba,
                delay_cs,
            })
            .collect();

        let report = AnimatedGifExporter::export(&gif_frames, &self.preset)
            .map_err(|e| format!("AnimatedGifExporter::export failed: {e}"))?;
        std::fs::write(output_gif, &report.bytes)
            .map_err(|e| format!("write GIF failed: {e}"))?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_dialog_presets_and_in_memory_export() {
        let mut dlg = CreateGifDialogState::default();
        dlg.select_preset(GifPresetChoice::ReducedFileSize);
        dlg.set_fade_to_black(true);
        assert!(dlg.preset.insert_transition_frames);

        dlg.select_preset(GifPresetChoice::ScreenVideo);
        dlg.set_fade_to_black(false);

        let frames: Vec<GifFrameInput> = (0..6u8)
            .map(|i| GifFrameInput {
                width: 16,
                height: 16,
                rgba: vec![i * 40, 100, 200, 255].repeat(16 * 16),
                delay_cs: 10,
            })
            .collect();
        let report = AnimatedGifExporter::export(&frames, &dlg.preset).unwrap();
        assert!(report.bytes.starts_with(b"GIF89a"));
    }
}
