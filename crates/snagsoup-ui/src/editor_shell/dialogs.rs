//! Editor Dialogs (`W3-10` Part B):
//! - Resize Image (`px`, `%`, `in`, `cm` unit conversion + aspect lock + DPI)
//! - Resize Canvas & Canvas Color
//! - Rotate Custom Angle
//! - Capture Details (`SnagxMetadata` editor)

use snagsoup_canvas::DocumentSession;
use snagsoup_model::{ColorRgba, EffectConfig, SnagxMetadata};

/// Measurement unit supported in the Resize Image dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeUnit {
    #[default]
    Pixels,
    Percent,
    Inches,
    Centimeters,
}

impl SizeUnit {
    pub fn to_pixels(self, value: f64, original_px: u32, dpi: u32) -> u32 {
        let dpi_f = dpi.max(1) as f64;
        let px = match self {
            Self::Pixels => value,
            Self::Percent => (original_px as f64) * (value / 100.0),
            Self::Inches => value * dpi_f,
            Self::Centimeters => (value / 2.54) * dpi_f,
        };
        px.round().max(1.0) as u32
    }

    pub fn from_pixels(self, px: u32, original_px: u32, dpi: u32) -> f64 {
        let dpi_f = dpi.max(1) as f64;
        let px_f = px as f64;
        match self {
            Self::Pixels => px_f,
            Self::Percent => (px_f / original_px.max(1) as f64) * 100.0,
            Self::Inches => px_f / dpi_f,
            Self::Centimeters => (px_f / dpi_f) * 2.54,
        }
    }
}

/// Resize Image dialog state (`px`, `%`, `in`, `cm` + aspect ratio lock + DPI).
#[derive(Debug, Clone)]
pub struct ResizeImageDialogState {
    pub orig_width: u32,
    pub orig_height: u32,
    pub target_width_px: u32,
    pub target_height_px: u32,
    pub unit: SizeUnit,
    pub dpi: u32,
    pub lock_aspect: bool,
}

impl ResizeImageDialogState {
    pub fn from_session(session: &DocumentSession) -> Self {
        let w = session.bitmap.width();
        let h = session.bitmap.height();
        Self {
            orig_width: w,
            orig_height: h,
            target_width_px: w,
            target_height_px: h,
            unit: SizeUnit::Pixels,
            dpi: session.page.dpi.unwrap_or(96.0).round().max(72.0) as u32,
            lock_aspect: true,
        }
    }

    pub fn set_width_in_unit(&mut self, val: f64) {
        let new_w = self.unit.to_pixels(val, self.orig_width, self.dpi);
        self.target_width_px = new_w;
        if self.lock_aspect && self.orig_width > 0 {
            let ratio = self.orig_height as f64 / self.orig_width as f64;
            self.target_height_px = ((new_w as f64) * ratio).round().max(1.0) as u32;
        }
    }

    pub fn set_height_in_unit(&mut self, val: f64) {
        let new_h = self.unit.to_pixels(val, self.orig_height, self.dpi);
        self.target_height_px = new_h;
        if self.lock_aspect && self.orig_height > 0 {
            let ratio = self.orig_width as f64 / self.orig_height as f64;
            self.target_width_px = ((new_h as f64) * ratio).round().max(1.0) as u32;
        }
    }

    pub fn apply(&self, session: &mut DocumentSession) -> bool {
        session.apply_effect(&EffectConfig::Scale {
            width: self.target_width_px,
            height: self.target_height_px,
            keep_aspect_ratio: self.lock_aspect,
            dpi: self.dpi,
        })
    }
}

/// Resize Canvas & Canvas Color dialog state.
#[derive(Debug, Clone)]
pub struct ResizeCanvasDialogState {
    pub new_width: u32,
    pub new_height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub canvas_color: ColorRgba,
}

impl ResizeCanvasDialogState {
    pub fn apply(&self, session: &mut DocumentSession) {
        let resized = session.bitmap.resize(
            self.new_width.max(1),
            self.new_height.max(1),
            self.offset_x,
            self.offset_y,
            self.canvas_color,
        );
        session.bitmap = resized;
        session.page.capture_canvas_width = Some(self.new_width.max(1) as f64);
        session.page.capture_canvas_height = Some(self.new_height.max(1) as f64);
        session.page.capture_background_color = Some(self.canvas_color);
        session.dirty.mark_all(self.new_width.max(1), self.new_height.max(1));
    }
}

/// Rotate Custom Angle dialog state.
#[derive(Debug, Clone, Copy)]
pub struct RotateCustomDialogState {
    pub angle_degrees: f32,
    pub background_color: ColorRgba,
}

impl Default for RotateCustomDialogState {
    fn default() -> Self {
        Self {
            angle_degrees: 0.0,
            background_color: ColorRgba::TRANSPARENT,
        }
    }
}

impl RotateCustomDialogState {
    pub fn apply(&self, session: &mut DocumentSession) -> bool {
        session.apply_effect(&EffectConfig::Rotate {
            angle_degrees: self.angle_degrees,
            background_color: self.background_color,
        })
    }
}

/// Capture Details (`SnagxMetadata`) dialog state.
#[derive(Debug, Clone, Default)]
pub struct CaptureDetailsDialogState {
    pub metadata: SnagxMetadata,
}

impl CaptureDetailsDialogState {
    pub fn new(metadata: SnagxMetadata) -> Self {
        Self { metadata }
    }

    pub fn set_tags_csv(&mut self, csv: &str) {
        let tags: Vec<String> = csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.metadata.tags = if tags.is_empty() { None } else { Some(tags) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_canvas::TiledBitmap;
    use snagsoup_model::SnagxPage;

    #[test]
    fn resize_image_units_aspect_lock_and_metadata_details() {
        let bmp = TiledBitmap::new_filled(1920, 1080, ColorRgba::WHITE);
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);

        let mut dlg = ResizeImageDialogState::from_session(&session);
        dlg.unit = SizeUnit::Percent;
        dlg.set_width_in_unit(50.0);
        assert_eq!((dlg.target_width_px, dlg.target_height_px), (960, 540));

        dlg.unit = SizeUnit::Inches;
        dlg.dpi = 96;
        dlg.set_width_in_unit(2.0);
        assert_eq!(dlg.target_width_px, 192);
        assert_eq!(dlg.target_height_px, 108);

        assert!(dlg.apply(&mut session));
        assert_eq!((session.bitmap.width(), session.bitmap.height()), (192, 108));

        let mut details = CaptureDetailsDialogState::default();
        details.set_tags_csv("docs, rust, snagsoup");
        assert_eq!(details.metadata.tags.as_ref().unwrap().len(), 3);
    }
}
