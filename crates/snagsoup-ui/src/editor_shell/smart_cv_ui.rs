//! Smart CV & OCR Interactive UI Controllers (`W3-11`):
//! - Smart Move (`detect_smart_objects`, `lift_smart_object` + hover highlight + auto-inpaint)
//! - Auto-Simplify & `SimplifyUIFloaty` (`auto_simplify`, `SuiDetail` 1..=10, Show Original, Palettes)
//! - Magic Wand (`magic_wand_select` Contiguous/Global + live Tolerance + pixel-mask overlay + Clear/Fill/Copy)
//! - Grab Text & Edit Text (`grab_text` modal with live progress bar + `edit_text_in_session`)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::RenderImage;
use snagsoup_canvas::{DocumentSession, PixelRect, SceneCompositor};
use snagsoup_cv_ocr::{
    MagicWandMode, SelectionMask, SmartElement, auto_simplify, detect_smart_objects,
    edit_text_in_session, extract_boundary_polylines, grab_text, lift_smart_object,
    magic_wand_select, recognize_rgba,
};
use snagsoup_model::{ColorRgba, PointF, RectF, SnagPalette};

const AUTO_SIMPLIFY_PREFIX: &str = "auto-simplify-";

/// Smart Move interactive state (inside `Move` tool).
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

    /// Turn off Smart Move overlays.
    pub fn disable(&mut self) {
        self.enabled = false;
        self.detected.clear();
        self.hovered_idx = None;
    }

    /// Find the tightest detected `SmartElement` containing canvas point `pt`.
    pub fn hit_test_at(&self, pt: PointF) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        let px_x = pt.x.floor();
        let px_y = pt.y.floor();
        self.detected
            .iter()
            .enumerate()
            .filter(|(_, el)| {
                let r = el.bounds;
                px_x >= r.x as f64
                    && px_x < r.right() as f64
                    && px_y >= r.y as f64
                    && px_y < r.bottom() as f64
            })
            .min_by_key(|(_, el)| (el.bounds.width as u64) * (el.bounds.height as u64))
            .map(|(i, _)| i)
    }

    /// Lift a detected Smart Element by index into a movable `ToolMode::Image` object,
    /// automatically inpainting its original background footprint and removing its detection box.
    pub fn lift_detected_index(
        &mut self,
        session: &mut DocumentSession,
        det_idx: usize,
    ) -> Result<usize, String> {
        let Some(elem) = self.detected.get(det_idx).cloned() else {
            return Err("Smart Move element index out of range".into());
        };
        let obj_idx = self.lift_element(session, elem.bounds)?;
        self.detected.remove(det_idx);
        self.hovered_idx = None;
        Ok(obj_idx)
    }

    /// Lift a detected Smart Element into a movable `ToolMode::Image` object and inpaint its footprint.
    pub fn lift_element(
        &mut self,
        session: &mut DocumentSession,
        rect: PixelRect,
    ) -> Result<usize, String> {
        let (obj_idx, _tier) =
            lift_smart_object(session, rect).map_err(|e| format!("Smart Move lift failed: {e}"))?;
        Ok(obj_idx)
    }
}

/// Auto-Simplify `SimplifyUIFloaty` state.
#[derive(Debug, Clone)]
pub struct SimplifyUiFloatyState {
    pub enabled: bool,
    pub detail: u8,
    pub show_original: bool,
    pub active_palette_idx: usize,
    pub palette: SnagPalette,
    pub generated_indices: Vec<usize>,
}

impl Default for SimplifyUiFloatyState {
    fn default() -> Self {
        let presets = Self::preset_palettes();
        Self {
            enabled: false,
            detail: 5,
            show_original: false,
            active_palette_idx: 0,
            palette: presets[0].clone(),
            generated_indices: Vec::new(),
        }
    }
}

impl SimplifyUiFloatyState {
    /// Built-in Snagit-style SUI wireframe color palettes.
    pub fn preset_palettes() -> [SnagPalette; 4] {
        [
            SnagPalette {
                id: "snagsoup-grayscale".into(),
                name: "Grayscale".into(),
                palette_colors: vec![
                    ColorRgba::rgb(229, 231, 235),
                    ColorRgba::rgb(156, 163, 175),
                    ColorRgba::rgb(75, 85, 99),
                    ColorRgba::rgb(27, 117, 188),
                ],
                extra: Default::default(),
            },
            SnagPalette {
                id: "snagsoup-blueprint".into(),
                name: "Blueprint".into(),
                palette_colors: vec![
                    ColorRgba::rgb(219, 234, 254),
                    ColorRgba::rgb(147, 197, 253),
                    ColorRgba::rgb(59, 130, 246),
                    ColorRgba::rgb(29, 78, 216),
                ],
                extra: Default::default(),
            },
            SnagPalette {
                id: "snagsoup-dark-wire".into(),
                name: "Slate".into(),
                palette_colors: vec![
                    ColorRgba::rgb(203, 213, 225),
                    ColorRgba::rgb(100, 116, 139),
                    ColorRgba::rgb(51, 65, 85),
                    ColorRgba::rgb(15, 23, 42),
                ],
                extra: Default::default(),
            },
            SnagPalette {
                id: "snagsoup-warm".into(),
                name: "Warm".into(),
                palette_colors: vec![
                    ColorRgba::rgb(254, 226, 226),
                    ColorRgba::rgb(252, 165, 165),
                    ColorRgba::rgb(239, 68, 68),
                    ColorRgba::rgb(245, 158, 11),
                ],
                extra: Default::default(),
            },
        ]
    }

    /// Select one of the built-in preset palettes (`0..4`), refreshing active Auto-Simplify blocks if enabled.
    pub fn select_palette(&mut self, palette_idx: usize, session: &mut DocumentSession) {
        let palettes = Self::preset_palettes();
        let idx = palette_idx.min(palettes.len() - 1);
        self.active_palette_idx = idx;
        self.palette = palettes[idx].clone();
        if self.enabled {
            let _ = self.run_auto_simplify(session);
        }
    }

    /// Remove all previously auto-generated Simplify blocks (without touching user annotations or
    /// manually drawn Simplify blocks).
    pub fn clear_auto_simplify(&mut self, session: &mut DocumentSession) -> usize {
        let to_remove: Vec<usize> = session
            .page
            .capture_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                o.object_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with(AUTO_SIMPLIFY_PREFIX))
            })
            .map(|(i, _)| i)
            .collect();
        for &idx in to_remove.iter().rev() {
            let _ = session.delete_object(idx);
        }
        self.generated_indices.clear();
        self.enabled = false;
        self.show_original = false;
        to_remove.len()
    }

    /// Run `auto_simplify` with current `detail` (`1..=10`) & `palette`, cleanly replacing any
    /// previous auto-generated Simplify blocks instead of stacking duplicates.
    pub fn run_auto_simplify(&mut self, session: &mut DocumentSession) -> usize {
        let _ = self.clear_auto_simplify(session);
        self.enabled = true;
        self.show_original = false;
        let objects = auto_simplify(&session.bitmap, Some(&self.palette), self.detail.clamp(1, 10));
        self.generated_indices.clear();
        for (k, mut obj) in objects.into_iter().enumerate() {
            obj.object_id = Some(format!("{AUTO_SIMPLIFY_PREFIX}{k}"));
            let idx = session.add_object(obj);
            self.generated_indices.push(idx);
        }
        self.generated_indices.len()
    }

    /// Toggle `Show Original` preview by setting opacity `0%` vs `100%` on auto-generated Simplify blocks.
    pub fn set_show_original(&mut self, show_orig: bool, session: &mut DocumentSession) {
        self.show_original = show_orig;
        let target_opacity = if show_orig { 0.0 } else { 100.0 };
        let indices: Vec<usize> = session
            .page
            .capture_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                o.object_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with(AUTO_SIMPLIFY_PREFIX))
            })
            .map(|(i, _)| i)
            .collect();
        for idx in indices {
            if let Some(mut updated) = session.page.capture_objects.get(idx).cloned() {
                updated.opacity = Some(target_opacity);
                let _ = session.update_object(idx, updated);
            }
        }
    }
}

/// Magic Wand interactive UI state (Contiguous vs Global + Tolerance + Pixel-Accurate Overlay).
#[derive(Clone)]
pub struct MagicWandUiState {
    pub mode: MagicWandMode,
    pub tolerance: f32,
    pub last_seed: Option<(u32, u32)>,
    pub active_mask: Option<SelectionMask>,
    pub marching_ants: Vec<Vec<PointF>>,
    /// Cropped pixel-accurate cyan selection tint + bright edge contour `(canvas_rect, render_image)`.
    pub mask_overlay: Option<(RectF, Arc<RenderImage>)>,
}

impl std::fmt::Debug for MagicWandUiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MagicWandUiState")
            .field("mode", &self.mode)
            .field("tolerance", &self.tolerance)
            .field("last_seed", &self.last_seed)
            .field("has_mask", &self.active_mask.is_some())
            .finish()
    }
}

impl Default for MagicWandUiState {
    fn default() -> Self {
        Self {
            mode: MagicWandMode::Contiguous,
            tolerance: 20.0,
            last_seed: None,
            active_mask: None,
            marching_ants: Vec::new(),
            mask_overlay: None,
        }
    }
}

impl MagicWandUiState {
    /// Clear active Magic Wand selection mask & overlay.
    pub fn clear_selection(&mut self) {
        self.last_seed = None;
        self.active_mask = None;
        self.marching_ants.clear();
        self.mask_overlay = None;
    }

    /// Number of currently selected pixels (`0` if none).
    pub fn selected_pixel_count(&self) -> usize {
        self.active_mask
            .as_ref()
            .map_or(0, SelectionMask::selected_count)
    }

    /// Select pixels starting at `(x, y)` and build the visual contour/tint overlay.
    pub fn select_at(&mut self, session: &DocumentSession, x: u32, y: u32) -> usize {
        self.last_seed = Some((x, y));
        let mask = magic_wand_select(&session.bitmap, x, y, self.tolerance, self.mode);
        let count = mask.selected_count();
        if count == 0 {
            self.active_mask = None;
            self.marching_ants.clear();
            self.mask_overlay = None;
            return 0;
        }
        self.marching_ants = extract_boundary_polylines(&mask);
        self.mask_overlay = build_magic_wand_overlay_image(&mask);
        self.active_mask = Some(mask);
        count
    }

    /// Re-run selection at `last_seed` when the user adjusts `Tolerance` or switches `Contiguous`/`Global`.
    pub fn refresh_if_seeded(&mut self, session: &DocumentSession) -> Option<usize> {
        let (sx, sy) = self.last_seed?;
        Some(self.select_at(session, sx, sy))
    }

    /// Erase all selected Magic Wand pixels to transparent (`[0, 0, 0, 0]`) as an undoable CoW edit!
    pub fn delete_selected_pixels(&mut self, session: &mut DocumentSession) -> usize {
        let Some(mask) = self.active_mask.take() else {
            return 0;
        };
        let Some(bounds) = mask.bounds else {
            self.clear_selection();
            return 0;
        };
        let count = mask.selected_count();
        session.mutate_raster_region(bounds, |x, y, px| {
            if mask.is_selected(x, y) {
                *px = [0, 0, 0, 0];
            }
        });
        self.clear_selection();
        count
    }

    /// Fill all selected Magic Wand pixels with `color` as an undoable CoW edit!
    pub fn fill_selected_pixels(
        &mut self,
        session: &mut DocumentSession,
        color: ColorRgba,
    ) -> usize {
        let Some(mask) = self.active_mask.as_ref().cloned() else {
            return 0;
        };
        let Some(bounds) = mask.bounds else {
            return 0;
        };
        let count = mask.selected_count();
        let fill_px = [color.r, color.g, color.b, color.a];
        session.mutate_raster_region(bounds, |x, y, px| {
            if mask.is_selected(x, y) {
                *px = fill_px;
            }
        });
        count
    }

    /// Extract selected pixels inside `mask.bounds` as an `RGBA8` cutout (unselected pixels transparent).
    pub fn copy_selected_pixels_rgba(&self, session: &DocumentSession) -> Option<(u32, u32, Vec<u8>)> {
        let mask = self.active_mask.as_ref()?;
        let b = mask.bounds?;
        if b.width == 0 || b.height == 0 {
            return None;
        }
        let mut out = vec![0u8; (b.width as usize) * (b.height as usize) * 4];
        for ly in 0..b.height {
            for lx in 0..b.width {
                let gx = b.x + lx;
                let gy = b.y + ly;
                if mask.is_selected(gx, gy) {
                    if let Some(px) = session.bitmap.get_pixel(gx, gy) {
                        let off = ((ly as usize) * (b.width as usize) + (lx as usize)) * 4;
                        out[off..off + 4].copy_from_slice(&px);
                    }
                }
            }
        }
        Some((b.width, b.height, out))
    }
}

fn build_magic_wand_overlay_image(mask: &SelectionMask) -> Option<(RectF, Arc<RenderImage>)> {
    let b = mask.bounds?;
    if b.width == 0 || b.height == 0 {
        return None;
    }
    let rw = b.width as usize;
    let rh = b.height as usize;
    // Direct BGRA8 buffer for GPUI RenderImage
    let mut bgra = vec![0u8; rw * rh * 4];

    for ly in 0..b.height {
        for lx in 0..b.width {
            let gx = b.x + lx;
            let gy = b.y + ly;
            if !mask.is_selected(gx, gy) {
                continue;
            }
            let is_boundary = gx == 0
                || gy == 0
                || gx + 1 >= mask.width
                || gy + 1 >= mask.height
                || !mask.is_selected(gx - 1, gy)
                || !mask.is_selected(gx + 1, gy)
                || !mask.is_selected(gx, gy - 1)
                || !mask.is_selected(gx, gy + 1);

            let off = ((ly as usize) * rw + (lx as usize)) * 4;
            if is_boundary {
                // High-contrast marching-ants dashed boundary (alternating bright cyan #00D8FF and white)
                let dash = ((gx + gy) / 4) % 2 == 0;
                if dash {
                    bgra[off] = 0xFF;     // B
                    bgra[off + 1] = 0xB8; // G
                    bgra[off + 2] = 0x00; // R
                    bgra[off + 3] = 0xF2; // A
                } else {
                    bgra[off] = 0xFF;
                    bgra[off + 1] = 0xFF;
                    bgra[off + 2] = 0xFF;
                    bgra[off + 3] = 0xEE;
                }
            } else {
                // Subtle diagonal stripe cyan highlight inside selected area so user sees exact mask
                let stripe = ((gx + gy) % 8) < 2;
                bgra[off] = 0xE4;     // B
                bgra[off + 1] = 0xA4; // G
                bgra[off + 2] = 0x00; // R
                bgra[off + 3] = if stripe { 0x58 } else { 0x2E };
            }
        }
    }

    let render_img = crate::bgra_vec_to_render_image(b.width, b.height, bgra)?;
    let rect = RectF::new(b.x as f64, b.y as f64, b.width as f64, b.height as f64);
    Some((rect, render_img))
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

        // 1. Magic Wand selects exact button, builds visual overlay, deletes to transparent, and supports Undo/Redo!
        let mut wand = MagicWandUiState::default();
        let selected = wand.select_at(&session, 40, 30);
        assert_eq!(selected, 40 * 24);
        assert!(wand.mask_overlay.is_some());
        let erased = wand.delete_selected_pixels(&mut session);
        assert_eq!(erased, 40 * 24);
        assert_eq!(session.bitmap.get_pixel(40, 30), Some([0, 0, 0, 0]));
        assert!(session.undo());
        assert_eq!(session.bitmap.get_pixel(40, 30), Some([20, 100, 240, 255]));
        assert!(session.redo());
        assert_eq!(session.bitmap.get_pixel(40, 30), Some([0, 0, 0, 0]));
        assert!(session.undo()); // restore button for Smart Move test

        // 2. Smart Move scans, hit-tests, & lifts button cleanly
        let mut sm = SmartMoveUiState::default();
        assert!(sm.scan(&session) >= 1);
        let hit = sm.hit_test_at(PointF::new(45.0, 30.0)).expect("hit smart button");
        let lifted = sm.lift_detected_index(&mut session, hit).unwrap();
        assert!(session.page.capture_objects[lifted].image.is_some());

        // 3. Auto-Simplify replaces previous blocks instead of stacking duplicates
        let mut simp = SimplifyUiFloatyState::default();
        let c1 = simp.run_auto_simplify(&mut session);
        let obj_len_after_1 = session.page.capture_objects.len();
        simp.detail = 8;
        let c2 = simp.run_auto_simplify(&mut session);
        assert_eq!(
            session.page.capture_objects.len(),
            obj_len_after_1 - c1 + c2,
            "re-running Auto-Simplify must replace old auto-simplify blocks without stacking"
        );
        simp.set_show_original(true, &mut session);
        assert!(simp.show_original);
        let removed = simp.clear_auto_simplify(&mut session);
        assert_eq!(removed, c2);
    }
}
