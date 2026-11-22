//! Interactive Raster Tool UI Controllers (`W3-09` Part A):
//! - Eraser (brush preview radius -> CoW `eraser_stroke`)
//! - Flood Fill (tolerance 0..100 + antialiasing -> `flood_fill`)
//! - Non-destructive Blur / Pixelate & Magnify object placement
//! - Crop (`CropFloatyState` W×H bar -> `crop_document`)
//! - Cut Out (horizontal/vertical preview strip -> `cut_out_strip`)
//! - Selection (Rectangle, Ellipse, Lasso, Polygon)

use snagsoup_canvas::{
    CutOutOrientation, DocumentSession, PixelRect, crop_document, cut_out_strip, eraser_stroke,
    flood_fill,
};
use snagsoup_model::{CaptureObject, ColorRgba, PointF, RectF, ToolMode};

/// Raster selection tool sub-shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RasterSelectionKind {
    #[default]
    Rectangle,
    Ellipse,
    Lasso,
    Polygon,
}

/// Floating `CropFloaty` dimensions & state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropFloatyState {
    pub active: bool,
    pub rect: PixelRect,
    pub lock_aspect: bool,
}

impl Default for CropFloatyState {
    fn default() -> Self {
        Self {
            active: false,
            rect: PixelRect::new(0, 0, 800, 600),
            lock_aspect: false,
        }
    }
}

/// Interactive Cut Out strip preview state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutOutPreviewState {
    pub orientation: CutOutOrientation,
    pub start: u32,
    pub end: u32,
    pub style_id: i32,
}

impl Default for CutOutPreviewState {
    fn default() -> Self {
        Self {
            orientation: CutOutOrientation::Horizontal,
            start: 100,
            end: 140,
            style_id: 12,
        }
    }
}

/// Interactive controller wiring Raster UI tools directly to `snagsoup-canvas` engines.
#[derive(Debug, Clone)]
pub struct RasterToolController {
    pub eraser_radius: f32,
    pub fill_tolerance: f32,
    pub fill_antialiasing: bool,
    pub blur_intensity: f32,
    pub pixelate_mode: bool,
    pub magnify_scale: f32,
    pub crop: CropFloatyState,
    pub cutout: CutOutPreviewState,
    pub selection_kind: RasterSelectionKind,
    pub selection_points: Vec<PointF>,
}

impl Default for RasterToolController {
    fn default() -> Self {
        Self {
            eraser_radius: 14.0,
            fill_tolerance: 15.0,
            fill_antialiasing: true,
            blur_intensity: 20.0,
            pixelate_mode: false,
            magnify_scale: 2.0,
            crop: CropFloatyState::default(),
            cutout: CutOutPreviewState::default(),
            selection_kind: RasterSelectionKind::Rectangle,
            selection_points: Vec::new(),
        }
    }
}

impl RasterToolController {
    /// Apply a continuous Eraser brush stroke onto `DocumentSession` (with CoW Undo).
    pub fn apply_eraser_stroke(
        &self,
        session: &mut DocumentSession,
        points: &[PointF],
        fill_color: ColorRgba,
    ) {
        if points.is_empty() {
            return;
        }
        let mut next_bmp = session.bitmap.clone();
        eraser_stroke(&mut next_bmp, points, self.eraser_radius, fill_color);
        let (w, h) = (next_bmp.width(), next_bmp.height());
        session.bitmap = next_bmp;
        session.dirty.mark_all(w, h);
    }

    /// Execute Flood Fill at `(x, y)` on `DocumentSession`.
    pub fn apply_flood_fill(
        &self,
        session: &mut DocumentSession,
        x: u32,
        y: u32,
        fill_color: ColorRgba,
    ) -> bool {
        if flood_fill(
            &mut session.bitmap,
            x,
            y,
            fill_color,
            self.fill_tolerance,
            self.fill_antialiasing,
        )
        .is_some()
        {
            let (w, h) = (session.bitmap.width(), session.bitmap.height());
            session.dirty.mark_all(w, h);
            true
        } else {
            false
        }
    }

    /// Place a non-destructive `Blur` or `Pixelate` region `CaptureObject`.
    pub fn place_blur_region(
        &self,
        session: &mut DocumentSession,
        rect: RectF,
    ) -> usize {
        let mut obj = CaptureObject {
            tool_mode: Some(ToolMode::Blur),
            start_point: Some(PointF::new(rect.x, rect.y)),
            end_point: Some(PointF::new(rect.x + rect.width, rect.y + rect.height)),
            opacity: Some(100.0),
            ..CaptureObject::default()
        };
        obj.extra.insert(
            "BlurIntensity".to_string(),
            serde_json::json!(self.blur_intensity),
        );
        if self.pixelate_mode {
            obj.extra
                .insert("BlurStyle".to_string(), serde_json::json!("Pixelate"));
        }
        session.add_object(obj)
    }

    /// Place a non-destructive `Magnify` loupe `CaptureObject`.
    pub fn place_magnify_region(&self, session: &mut DocumentSession, rect: RectF) -> usize {
        let mut obj = CaptureObject {
            tool_mode: Some(ToolMode::Magnify),
            start_point: Some(PointF::new(rect.x, rect.y)),
            end_point: Some(PointF::new(rect.x + rect.width, rect.y + rect.height)),
            opacity: Some(100.0),
            ..CaptureObject::default()
        };
        obj.extra.insert(
            "MagnifyScale".to_string(),
            serde_json::json!(self.magnify_scale),
        );
        session.add_object(obj)
    }

    /// Apply the active `CropFloatyState` to `DocumentSession` (shifting all annotations + Undo).
    pub fn apply_crop(&mut self, session: &mut DocumentSession) -> bool {
        let ok = crop_document(session, self.crop.rect);
        if ok {
            self.crop.active = false;
        }
        ok
    }

    /// Apply the active `CutOutPreviewState` strip removal to `DocumentSession`.
    pub fn apply_cut_out(&self, session: &mut DocumentSession) -> bool {
        if self.cutout.end <= self.cutout.start {
            return false;
        }
        let next = cut_out_strip(
            &session.bitmap,
            self.cutout.orientation,
            self.cutout.start,
            self.cutout.end,
            self.cutout.style_id,
            42,
        );
        let (w, h) = (next.width(), next.height());
        session.bitmap = next;
        session.dirty.mark_all(w, h);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_canvas::TiledBitmap;
    use snagsoup_model::SnagxPage;

    #[test]
    fn raster_controller_crop_fill_blur_and_cutout() {
        let bmp = TiledBitmap::new_filled(200, 200, ColorRgba::WHITE);
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);
        let mut rc = RasterToolController::default();

        assert!(rc.apply_flood_fill(&mut session, 10, 10, ColorRgba::rgb(20, 40, 60)));
        assert_eq!(session.bitmap.get_pixel(10, 10), Some([20, 40, 60, 255]));

        let blur_idx = rc.place_blur_region(&mut session, RectF::new(20.0, 20.0, 60.0, 60.0));
        assert_eq!(session.page.capture_objects[blur_idx].tool_mode, Some(ToolMode::Blur));

        rc.crop.rect = PixelRect::new(10, 10, 100, 100);
        assert!(rc.apply_crop(&mut session));
        assert_eq!((session.bitmap.width(), session.bitmap.height()), (100, 100));

        rc.cutout = CutOutPreviewState {
            orientation: CutOutOrientation::Horizontal,
            start: 20,
            end: 40,
            style_id: 12,
        };
        assert!(rc.apply_cut_out(&mut session));
        assert_eq!(session.bitmap.height(), 80);
    }
}
