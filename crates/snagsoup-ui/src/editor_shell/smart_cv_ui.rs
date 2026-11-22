//! Smart CV & OCR Interactive UI Controllers (`W3-11`):
//! - Smart Move (`detect_smart_objects`, `lift_smart_object` + hover highlight + auto-inpaint)
//! - Auto-Simplify & `SimplifyUIFloaty` (`auto_simplify`, `SuiDetail` 1..=10, Show Original, `.snagpalette`)
//! - Magic Wand (`magic_wand_select` Contiguous/Global + live Tolerance + marching-ants polylines)
//! - Grab Text & Edit Text (`grab_text` modal + `edit_text_in_session`)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use snagsoup_canvas::{DocumentSession, PixelRect, SceneCompositor};
use snagsoup_cv_ocr::{
    MagicWandMode, SelectionMask, SmartElement, auto_simplify, detect_smart_objects,
    edit_text_in_session, extract_boundary_polylines, grab_text, lift_smart_object,
    magic_wand_select, recognize_rgba,
};
use snagsoup_model::{ColorRgba, PointF, RectF, SnagPalette};

/// Smart Move interactive state (inside Move tool).
#[derive(Debug, Clone, Default)]
pub struct SmartMoveUiState {
    pub enabled: bool,
    pub detected: Vec<SmartElement>,
    pub hovered_idx: Option<usize>,
}

impl SmartMoveUiState {
    /// Scan the active bitmap via `detect_smart_objects` when Smart Move is toggled on.
    pub fn scan(&mut self, session: &DocumentSession) -> usize {
        self.enabled = true;
        self.detected = detect_smart_objects(&session.bitmap);
        self.hovered_idx = None;
        self.detected.len()
    }

    /// Lift a detected Smart Element into a movable `ToolMode::Image` object and inpaint its footprint.
    pub fn lift_element(&mut self, session: &mut DocumentSession, rect: PixelRect) -> Result<usize, String> {
        let (obj_idx, _tier) = lift_smart_object(session, rect)
            .map_err(|e| format!("Smart Move lift failed: {e}"))?;
        Ok(obj_idx)
    }
}

/// Auto-Simplify `SimplifyUIFloaty` state.
#[derive(Debug, Clone)]
pub struct SimplifyUiFloatyState {
    pub enabled: bool,
    pub detail: u8,
    pub show_original: bool,
    pub palette: SnagPalette,
    pub generated_indices: Vec<usize>,
}

impl Default for SimplifyUiFloatyState {
    fn default() -> Self {
        Self {
            enabled: false,
            detail: 5,
            show_original: false,
            palette: SnagPalette {
                id: "snagsoup-default-palette".into(),
                name: "Clean Grayscale".into(),
                palette_colors: vec![
                    ColorRgba::rgb(229, 231, 235),
                    ColorRgba::rgb(156, 163, 175),
                    ColorRgba::rgb(75, 85, 99),
                    ColorRgba::rgb(27, 117, 188),
                ],
                extra: Default::default(),
            },
            generated_indices: Vec::new(),
        }
    }
}

impl SimplifyUiFloatyState {
    /// Run `auto_simplify` with current `detail` (1..=10) & `palette` and add objects to `session`.
    pub fn run_auto_simplify(&mut self, session: &mut DocumentSession) -> usize {
        self.enabled = true;
        let objects = auto_simplify(&session.bitmap, Some(&self.palette), self.detail.clamp(1, 10));
        self.generated_indices.clear();
        for obj in objects {
            let idx = session.add_object(obj);
            self.generated_indices.push(idx);
        }
        self.generated_indices.len()
    }
}

/// Magic Wand interactive UI state (Contiguous vs Global + Tolerance + Marching Ants polylines).
#[derive(Debug, Clone)]
pub struct MagicWandUiState {
    pub mode: MagicWandMode,
    pub tolerance: f32,
    pub active_mask: Option<SelectionMask>,
    pub marching_ants: Vec<Vec<PointF>>,
}

impl Default for MagicWandUiState {
    fn default() -> Self {
        Self {
            mode: MagicWandMode::Contiguous,
            tolerance: 15.0,
            active_mask: None,
            marching_ants: Vec::new(),
        }
    }
}

impl MagicWandUiState {
    /// Select pixels starting at `(x, y)` and extract marching-ants boundary polylines.
    pub fn select_at(&mut self, session: &DocumentSession, x: u32, y: u32) -> usize {
        let mask = magic_wand_select(&session.bitmap, x, y, self.tolerance, self.mode);
        let count = mask.selected_count();
        self.marching_ants = extract_boundary_polylines(&mask);
        self.active_mask = Some(mask);
        count
    }
}

/// Grab Text & Edit Text modal/controller state.
#[derive(Debug, Clone, Default)]
pub struct GrabEditTextUiState {
    pub modal_open: bool,
    pub is_running: bool,
    pub progress: f32,
    pub progress_stage: String,
    pub active_job_id: u64,
    pub cancel_flag: Option<Arc<AtomicBool>>,
    pub grabbed_text: String,
    pub scope_label: String,
    pub copied_feedback: bool,
    pub last_spawned_text_objects: Vec<usize>,
}

impl GrabEditTextUiState {
    /// Cancel any in-flight background OCR job and close or reset running state.
    pub fn cancel_running_job(&mut self) {
        if let Some(flag) = self.cancel_flag.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.active_job_id = self.active_job_id.wrapping_add(1);
        self.is_running = false;
    }

    /// Flatten `session` (and crop `selection` if `>= 4×4 px`) into an RGBA8 buffer + scope label
    /// ready for background or synchronous OCR execution.
    pub fn prepare_grab_text_buffer(
        session: &DocumentSession,
        selection: Option<RectF>,
    ) -> (u32, u32, Vec<u8>, String) {
        let comp = SceneCompositor::new(&session.page, Some(&session.bitmap));
        let (full_w, full_h, full_rgba) = match comp.flatten_to_rgba() {
            Ok(flat) => flat,
            Err(_) => (
                session.bitmap.width(),
                session.bitmap.height(),
                session.bitmap.to_rgba(),
            ),
        };

        if let Some(sel) = selection.filter(|r| r.width >= 4.0 && r.height >= 4.0) {
            let x0 = (sel.x.floor().max(0.0) as u32).min(full_w.saturating_sub(1));
            let y0 = (sel.y.floor().max(0.0) as u32).min(full_h.saturating_sub(1));
            let x1 = (sel.right().ceil().max((x0 + 1) as f64) as u32).min(full_w);
            let y1 = (sel.bottom().ceil().max((y0 + 1) as f64) as u32).min(full_h);
            let rw = (x1 - x0).max(1);
            let rh = (y1 - y0).max(1);

            let mut sub_rgba = vec![0u8; (rw as usize) * (rh as usize) * 4];
            for row in 0..rh as usize {
                let src_off = (((y0 as usize) + row) * (full_w as usize) + (x0 as usize)) * 4;
                let dst_off = row * (rw as usize) * 4;
                sub_rgba[dst_off..(dst_off + (rw as usize) * 4)]
                    .copy_from_slice(&full_rgba[src_off..(src_off + (rw as usize) * 4)]);
            }
            (
                rw,
                rh,
                sub_rgba,
                format!("Selected Region ({rw}×{rh} px)"),
            )
        } else {
            (
                full_w,
                full_h,
                full_rgba,
                format!("Full Canvas ({full_w}×{full_h} px)"),
            )
        }
    }

    /// Run Offline Multilingual Neural OCR `grab_text` on `session` (or just `selection` if a
    /// marquee selection is active) and open the Snagit-style Grab Text Results modal.
    pub fn run_grab_text(&mut self, session: &DocumentSession) -> Result<&str, String> {
        self.run_grab_text_region(session, None)
    }

    /// Region-aware Grab Text: flattens the active canvas (including vector callouts/text) and
    /// extracts text from `selection` if `Some` (`>= 4×4 px`), or the entire canvas otherwise.
    pub fn run_grab_text_region(
        &mut self,
        session: &DocumentSession,
        selection: Option<RectF>,
    ) -> Result<&str, String> {
        self.cancel_running_job();
        let (rw, rh, rgba, scope) = Self::prepare_grab_text_buffer(session, selection);
        let doc = match recognize_rgba(rw, rh, &rgba, None) {
            Ok(d) => d,
            Err(_) if selection.is_none() => {
                let text = grab_text(&session.bitmap, None)
                    .map_err(|e| format!("Grab Text OCR failed: {e}"))?;
                self.grabbed_text = text;
                self.scope_label = scope;
                self.progress = 1.0;
                self.progress_stage = "Completed".to_string();
                self.modal_open = true;
                return Ok(&self.grabbed_text);
            }
            Err(e) => return Err(format!("Grab Text OCR failed: {e}")),
        };
        self.grabbed_text = doc.text;
        self.scope_label = scope;
        self.progress = 1.0;
        self.progress_stage = "Completed".to_string();
        self.modal_open = true;
        Ok(&self.grabbed_text)
    }

    /// Run `edit_text_in_session` to erase bitmap text via inpainting and spawn editable `Text` objects.
    pub fn run_edit_text(&mut self, session: &mut DocumentSession) -> Result<usize, String> {
        let spawned = edit_text_in_session(session, None)
            .map_err(|e| format!("Edit Text OCR failed: {e}"))?;
        let count = spawned.len();
        self.last_spawned_text_objects = spawned;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_canvas::TiledBitmap;
    use snagsoup_model::SnagxPage;

    #[test]
    fn smart_move_magic_wand_and_simplify_ui_controllers() {
        let mut bmp = TiledBitmap::new_filled(120, 80, ColorRgba::WHITE);
        let btn = PixelRect::new(30, 20, 40, 24);
        bmp.mutate_region(btn, |_, _, px| *px = [20, 100, 240, 255]);
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);

        // Magic Wand selects exact button + extracts marching ants polyline
        let mut wand = MagicWandUiState::default();
        let selected = wand.select_at(&session, 40, 30);
        assert_eq!(selected, 40 * 24);
        assert!(!wand.marching_ants.is_empty());

        // Smart Move scans & lifts button
        let mut sm = SmartMoveUiState::default();
        assert!(sm.scan(&session) >= 1);
        let lifted = sm.lift_element(&mut session, btn).unwrap();
        assert!(session.page.capture_objects[lifted].image.is_some());

        // Auto-Simplify floaty
        let mut simp = SimplifyUiFloatyState::default();
        let _ = simp.run_auto_simplify(&mut session);
        assert!(simp.enabled);
    }
}
