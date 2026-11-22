//! Interactive 15 Image Effects Studio (`W3-10`):
//! - Non-destructive **ON / OFF** toggleable effect stack (`Border`, `Drop Shadow`, `Edges Torn`,
//!   `Edges Wave`, `Page Curl`, `Perspective`, `Filters`, `Color Adjustment`, `Color Replacement`,
//!   `Spotlight & Magnify`, `Caption`) backed by `base_bitmap` so clicking once turns an effect ON,
//!   tweaking sliders/colors updates live without double-stacking, and clicking again turns it OFF!
//! - Interactive On-Canvas Move + 8-Handle Resize support for **`Spotlight & Magnify`**.
//! - One-shot geometric transforms (`Auto Trim`, `Rotate`, `Flip`, `Scale Image`).

use snagsoup_canvas::{
    DocumentSession, HistoryCommand, TiledBitmap, apply_effects, preview_effect_on_rgba,
};
use snagsoup_model::{ColorRgba, EffectConfig, PointF, RectF};

/// Handle/zone hit when dragging the interactive `Spotlight & Magnify` gizmo on the Canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectGizmoPart {
    MoveInside,
    NW,
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
}

/// A single entry in the 15-Effect Studio.
#[derive(Debug, Clone)]
pub struct EffectGalleryEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub toggleable: bool,
    pub enabled: bool,
    pub config: EffectConfig,
}

/// Live mini-thumbnail preview generated via `snagsoup_canvas::preview_effect_on_rgba`.
#[derive(Debug, Clone)]
pub struct EffectPreviewThumbnail {
    pub id: &'static str,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Manages all 15 Snagit Image Effects, non-destructive ON/OFF toggle stack, live parameter
/// updates, interactive `Spotlight & Magnify` canvas bounds, and saved presets.
#[derive(Debug, Clone)]
pub struct EffectsGalleryState {
    pub entries: Vec<EffectGalleryEntry>,
    pub selected_idx: Option<usize>,
    pub base_bitmap: Option<TiledBitmap>,
    pub active_order: Vec<&'static str>,
    pub previews: Vec<EffectPreviewThumbnail>,
    pub saved_presets: Vec<(String, EffectConfig)>,
    pub editing_caption_text: bool,
}

impl Default for EffectsGalleryState {
    fn default() -> Self {
        Self {
            entries: default_15_effects(),
            selected_idx: None,
            base_bitmap: None,
            active_order: Vec::new(),
            previews: Vec::new(),
            saved_presets: Vec::new(),
            editing_caption_text: false,
        }
    }
}

/// All 15 Snagit 2022 Image Effects with sensible defaults.
pub fn default_15_effects() -> Vec<EffectGalleryEntry> {
    vec![
        EffectGalleryEntry {
            id: "border",
            name: "Border",
            category: "Frames",
            toggleable: true,
            enabled: false,
            config: EffectConfig::Border {
                width: 6,
                color: ColorRgba::rgb(27, 117, 188),
                inside: false,
            },
        },
        EffectGalleryEntry {
            id: "shadow",
            name: "Drop Shadow",
            category: "Frames",
            toggleable: true,
            enabled: false,
            config: EffectConfig::Shadow {
                offset_x: 8,
                offset_y: 8,
                blur_radius: 10,
                opacity: 0.55,
                color: ColorRgba::BLACK,
            },
        },
        EffectGalleryEntry {
            id: "edges-torn",
            name: "Edges (Torn)",
            category: "Edges",
            toggleable: true,
            enabled: false,
            config: EffectConfig::Edges {
                style: "Torn".into(),
                size: 10,
                edges_mask: 0x0F,
                seed: 7,
            },
        },
        EffectGalleryEntry {
            id: "edges-wave",
            name: "Edges (Wave)",
            category: "Edges",
            toggleable: true,
            enabled: false,
            config: EffectConfig::Edges {
                style: "Wave".into(),
                size: 8,
                edges_mask: 0x0F,
                seed: 1,
            },
        },
        EffectGalleryEntry {
            id: "page-curl",
            name: "Page Curl",
            category: "3D",
            toggleable: true,
            enabled: false,
            config: EffectConfig::PageCurl {
                corner: "BottomRight".into(),
                radius: 32.0,
                background_color: ColorRgba::TRANSPARENT,
            },
        },
        EffectGalleryEntry {
            id: "perspective",
            name: "Perspective & Reflection",
            category: "3D",
            toggleable: true,
            enabled: false,
            config: EffectConfig::Perspective {
                angle_x: 16.0,
                angle_y: 0.0,
                reflection: true,
                reflection_opacity: 0.40,
            },
        },
        EffectGalleryEntry {
            id: "filters-grayscale",
            name: "Color Filter",
            category: "Color",
            toggleable: true,
            enabled: false,
            config: EffectConfig::Filters {
                kind: "Grayscale".into(),
                intensity: 100.0,
            },
        },
        EffectGalleryEntry {
            id: "color-adjust",
            name: "Color Adjustment",
            category: "Color",
            toggleable: true,
            enabled: false,
            config: EffectConfig::ColorAdjustment {
                brightness: 10.0,
                contrast: 15.0,
                hue: 0.0,
                saturation: 10.0,
                gamma: 1.0,
            },
        },
        EffectGalleryEntry {
            id: "color-replace",
            name: "Color Replacement",
            category: "Color",
            toggleable: true,
            enabled: false,
            config: EffectConfig::ColorReplacement {
                source: ColorRgba::WHITE,
                target: ColorRgba::rgb(240, 248, 255),
                tolerance: 14.0,
                preserve_luminance: true,
            },
        },
        EffectGalleryEntry {
            id: "spotlight",
            name: "Spotlight & Magnify",
            category: "Focus",
            toggleable: true,
            enabled: false,
            config: EffectConfig::SpotlightAndMagnify {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
                dim_opacity: 0.55,
                blur_radius: 4,
                magnify_scale: 1.25,
            },
        },
        EffectGalleryEntry {
            id: "caption",
            name: "Caption",
            category: "Layout",
            toggleable: true,
            enabled: false,
            config: EffectConfig::Caption {
                text: "SnagSoup Capture".into(),
                position: "Bottom".into(),
                font_family: "Segoe UI".into(),
                font_size: 18.0,
                text_color: ColorRgba::WHITE,
                background_color: ColorRgba::rgb(31, 35, 40),
            },
        },
        EffectGalleryEntry {
            id: "trim",
            name: "Auto Trim",
            category: "Transform",
            toggleable: false,
            enabled: false,
            config: EffectConfig::Trim {
                tolerance: 4,
                auto_trim: true,
            },
        },
        EffectGalleryEntry {
            id: "rotate",
            name: "Rotate",
            category: "Transform",
            toggleable: false,
            enabled: false,
            config: EffectConfig::Rotate {
                angle_degrees: 90.0,
                background_color: ColorRgba::TRANSPARENT,
            },
        },
        EffectGalleryEntry {
            id: "flip",
            name: "Flip",
            category: "Transform",
            toggleable: false,
            enabled: false,
            config: EffectConfig::Flip {
                horizontal: true,
                vertical: false,
            },
        },
        EffectGalleryEntry {
            id: "scale",
            name: "Scale Image",
            category: "Transform",
            toggleable: false,
            enabled: false,
            config: EffectConfig::Scale {
                width: 800,
                height: 600,
                keep_aspect_ratio: true,
                dpi: 96,
            },
        },
    ]
}

impl EffectsGalleryState {
    /// Reset toggle states when loading a brand-new capture or file.
    pub fn reset_for_new_document(&mut self, canvas_w: u32, canvas_h: u32) {
        self.base_bitmap = None;
        self.active_order.clear();
        self.editing_caption_text = false;
        for entry in &mut self.entries {
            entry.enabled = false;
            if let EffectConfig::SpotlightAndMagnify {
                x,
                y,
                width,
                height,
                ..
            } = &mut entry.config
            {
                *x = 0.0;
                *y = 0.0;
                *width = 0.0;
                *height = 0.0;
            } else if let EffectConfig::Scale { width, height, .. } = &mut entry.config {
                *width = canvas_w.max(1);
                *height = canvas_h.max(1);
            }
        }
    }

    /// Commit any active toggleable effects into the current bitmap baseline when the user
    /// performs an external destructive raster operation (e.g. Crop, Cut Out, Eraser, Undo).
    pub fn commit_baseline_after_external_edit(&mut self) {
        self.base_bitmap = None;
        self.active_order.clear();
        self.editing_caption_text = false;
        for entry in &mut self.entries {
            entry.enabled = false;
        }
    }

    /// Number of currently active (ON) toggleable effects.
    pub fn active_count(&self) -> usize {
        self.active_order.len()
    }

    /// Reference dimensions `(width, height)` of the unfiltered base bitmap (or current session bitmap).
    pub fn base_dimensions(&self, session: &DocumentSession) -> (u32, u32) {
        if let Some(ref base) = self.base_bitmap {
            (base.width(), base.height())
        } else {
            (session.bitmap.width(), session.bitmap.height())
        }
    }

    /// Returns `(effect_idx, spotlight_rect)` if `Spotlight & Magnify` is currently ON and selected.
    pub fn active_spotlight_rect(&self) -> Option<(usize, RectF)> {
        let sel = self.selected_idx?;
        let entry = self.entries.get(sel)?;
        if entry.id == "spotlight" && entry.enabled {
            if let EffectConfig::SpotlightAndMagnify {
                x,
                y,
                width,
                height,
                ..
            } = entry.config
            {
                if width >= 8.0 && height >= 8.0 {
                    return Some((sel, RectF::new(x, y, width, height)));
                }
            }
        }
        None
    }

    /// Hit-test the 8 resize handles + interior of the active `Spotlight & Magnify` box in canvas coordinates.
    pub fn hit_test_spotlight_gizmo(
        spot: RectF,
        canvas_pt: PointF,
        handle_tol_canvas: f64,
    ) -> Option<EffectGizmoPart> {
        let x0 = spot.x;
        let y0 = spot.y;
        let x1 = spot.right();
        let y1 = spot.bottom();
        let xm = (x0 + x1) * 0.5;
        let ym = (y0 + y1) * 0.5;

        let handles: &[(f64, f64, EffectGizmoPart)] = &[
            (x0, y0, EffectGizmoPart::NW),
            (xm, y0, EffectGizmoPart::N),
            (x1, y0, EffectGizmoPart::NE),
            (x1, ym, EffectGizmoPart::E),
            (x1, y1, EffectGizmoPart::SE),
            (xm, y1, EffectGizmoPart::S),
            (x0, y1, EffectGizmoPart::SW),
            (x0, ym, EffectGizmoPart::W),
        ];
        for &(hx, hy, part) in handles {
            if (canvas_pt.x - hx).abs() <= handle_tol_canvas
                && (canvas_pt.y - hy).abs() <= handle_tol_canvas
            {
                return Some(part);
            }
        }
        if canvas_pt.x >= x0 && canvas_pt.x <= x1 && canvas_pt.y >= y0 && canvas_pt.y <= y1 {
            return Some(EffectGizmoPart::MoveInside);
        }
        None
    }

    /// Compute updated `Spotlight & Magnify` rectangle during a live drag/resize operation on the canvas.
    pub fn drag_spotlight_rect(
        orig: RectF,
        part: EffectGizmoPart,
        dx: f64,
        dy: f64,
        max_w: u32,
        max_h: u32,
    ) -> RectF {
        let cw = (max_w.max(32)) as f64;
        let ch = (max_h.max(32)) as f64;
        let min_s = 24.0f64.min(cw * 0.5).min(ch * 0.5);

        match part {
            EffectGizmoPart::MoveInside => {
                let w = orig.width.clamp(min_s, cw);
                let h = orig.height.clamp(min_s, ch);
                let nx = (orig.x + dx).clamp(0.0, (cw - w).max(0.0));
                let ny = (orig.y + dy).clamp(0.0, (ch - h).max(0.0));
                RectF::new(nx.round(), ny.round(), w.round(), h.round())
            }
            _ => {
                let mut left = orig.x;
                let mut top = orig.y;
                let mut right = orig.right();
                let mut bottom = orig.bottom();

                if matches!(
                    part,
                    EffectGizmoPart::NW | EffectGizmoPart::W | EffectGizmoPart::SW
                ) {
                    left = (orig.x + dx).clamp(0.0, right - min_s);
                }
                if matches!(
                    part,
                    EffectGizmoPart::NE | EffectGizmoPart::E | EffectGizmoPart::SE
                ) {
                    right = (orig.right() + dx).clamp(left + min_s, cw);
                }
                if matches!(
                    part,
                    EffectGizmoPart::NW | EffectGizmoPart::N | EffectGizmoPart::NE
                ) {
                    top = (orig.y + dy).clamp(0.0, bottom - min_s);
                }
                if matches!(
                    part,
                    EffectGizmoPart::SW | EffectGizmoPart::S | EffectGizmoPart::SE
                ) {
                    bottom = (orig.bottom() + dy).clamp(top + min_s, ch);
                }

                RectF::new(
                    left.round(),
                    top.round(),
                    (right - left).max(min_s).round(),
                    (bottom - top).max(min_s).round(),
                )
            }
        }
    }

    /// Toggle an effect ON or OFF (for toggleable effects), or select a one-shot transform row.
    /// Returns `Some(true)` when turned ON, `Some(false)` when turned OFF, or `None` for transform rows.
    pub fn toggle_effect(
        &mut self,
        session: &mut DocumentSession,
        idx: usize,
        selection_hint: Option<RectF>,
    ) -> Option<bool> {
        let is_toggleable = self.entries.get(idx)?.toggleable;
        if !is_toggleable {
            self.selected_idx = Some(idx);
            if let Some(EffectGalleryEntry {
                config: EffectConfig::Scale { width, height, .. },
                ..
            }) = self.entries.get_mut(idx)
            {
                *width = session.bitmap.width().max(1);
                *height = session.bitmap.height().max(1);
            }
            return None;
        }

        let (base_w, base_h) = self.base_dimensions(session);
        let entry = self.entries.get_mut(idx)?;
        let next_enabled = !entry.enabled;
        entry.enabled = next_enabled;
        let entry_id = entry.id;

        if next_enabled {
            // Smart initial region for Spotlight & Magnify (uses active Selection if present, else centered 50% box)
            if entry_id == "spotlight" {
                if let EffectConfig::SpotlightAndMagnify {
                    x,
                    y,
                    width,
                    height,
                    ..
                } = &mut entry.config
                {
                    if let Some(sel) = selection_hint.filter(|r| r.width >= 16.0 && r.height >= 16.0)
                    {
                        *x = sel.x.clamp(0.0, (base_w.saturating_sub(16)) as f64).round();
                        *y = sel.y.clamp(0.0, (base_h.saturating_sub(16)) as f64).round();
                        *width = sel.width.clamp(16.0, base_w as f64 - *x).round();
                        *height = sel.height.clamp(16.0, base_h as f64 - *y).round();
                    } else if *width < 16.0
                        || *height < 16.0
                        || *x + *width > base_w as f64
                        || *y + *height > base_h as f64
                    {
                        let sw = ((base_w as f64) * 0.50).clamp(24.0, base_w as f64).round();
                        let sh = ((base_h as f64) * 0.50).clamp(24.0, base_h as f64).round();
                        *x = (((base_w as f64) - sw) * 0.5).max(0.0).round();
                        *y = (((base_h as f64) - sh) * 0.5).max(0.0).round();
                        *width = sw;
                        *height = sh;
                    }
                }
            }

            if self.base_bitmap.is_none() {
                self.base_bitmap = Some(session.bitmap.clone());
            }
            if !self.active_order.contains(&entry_id) {
                self.active_order.push(entry_id);
            }
            self.selected_idx = Some(idx);
        } else {
            self.active_order.retain(|&id| id != entry_id);
            if entry_id == "caption" {
                self.editing_caption_text = false;
            }
        }

        self.recompute_toggle_stack(session, true);
        if self.active_order.is_empty() {
            self.base_bitmap = None;
        }
        Some(next_enabled)
    }

    /// Update `entries[idx].config` and immediately recompute the non-destructive toggle stack.
    pub fn update_effect_config(
        &mut self,
        session: &mut DocumentSession,
        idx: usize,
        new_config: EffectConfig,
        record_history: bool,
    ) {
        let Some(entry) = self.entries.get_mut(idx) else {
            return;
        };
        entry.config = new_config;
        let is_toggleable = entry.toggleable;
        let entry_id = entry.id;
        let was_enabled = entry.enabled;

        if is_toggleable {
            if !was_enabled {
                entry.enabled = true;
                if self.base_bitmap.is_none() {
                    self.base_bitmap = Some(session.bitmap.clone());
                }
                if !self.active_order.contains(&entry_id) {
                    self.active_order.push(entry_id);
                }
            }
            self.selected_idx = Some(idx);
            self.recompute_toggle_stack(session, record_history);
        }
    }

    /// Turn OFF all active toggleable effects and restore the clean base bitmap.
    pub fn clear_all_active_effects(&mut self, session: &mut DocumentSession) -> bool {
        if self.active_order.is_empty() && self.base_bitmap.is_none() {
            return false;
        }
        for entry in &mut self.entries {
            entry.enabled = false;
        }
        self.active_order.clear();
        self.editing_caption_text = false;
        self.recompute_toggle_stack(session, true);
        self.base_bitmap = None;
        true
    }

    /// Apply a one-shot geometric transform (`Auto Trim`, `Rotate`, `Flip`, `Scale Image`),
    /// baking any currently-enabled toggleable effects into the new baseline first.
    pub fn apply_one_shot_transform(
        &mut self,
        session: &mut DocumentSession,
        config: &EffectConfig,
    ) -> bool {
        self.commit_baseline_after_external_edit();
        let ok = session.apply_effect(config);
        if ok {
            let nw = session.bitmap.width().max(1);
            let nh = session.bitmap.height().max(1);
            for entry in &mut self.entries {
                if let EffectConfig::Scale { width, height, .. } = &mut entry.config {
                    *width = nw;
                    *height = nh;
                }
            }
        }
        ok
    }

    fn recompute_toggle_stack(&self, session: &mut DocumentSession, record_history: bool) {
        let Some(ref base) = self.base_bitmap else {
            return;
        };
        let before_bitmap = session.bitmap.clone();
        let before_objects = session.page.capture_objects.clone();

        let after_bitmap = if self.active_order.is_empty() {
            base.clone()
        } else {
            let mut active_configs: Vec<EffectConfig> = Vec::with_capacity(self.active_order.len());
            for &id in &self.active_order {
                if let Some(e) = self.entries.iter().find(|e| e.id == id && e.enabled) {
                    active_configs.push(e.config.clone());
                }
            }
            let (nw, nh, out_rgba) =
                apply_effects(base.width(), base.height(), base.to_rgba(), &active_configs);
            match TiledBitmap::from_rgba(nw, nh, &out_rgba) {
                Ok(bmp) => bmp,
                Err(_) => return,
            }
        };

        let nw = after_bitmap.width();
        let nh = after_bitmap.height();
        session.page.capture_canvas_width = Some(nw as f64);
        session.page.capture_canvas_height = Some(nh as f64);
        session.bitmap = after_bitmap.clone();
        session.dirty.mark_all(nw, nh);

        if record_history {
            let after_objects = session.page.capture_objects.clone();
            session.history.push(HistoryCommand::CanvasResize {
                before_bitmap,
                after_bitmap,
                before_objects,
                after_objects,
            });
        }
    }

    /// Regenerate live mini-thumbnails (`<= max_dim px`) for all 15 effects from `session`.
    pub fn refresh_live_thumbnails(&mut self, session: &DocumentSession, max_dim: u32) {
        let base_ref = self.base_bitmap.as_ref().unwrap_or(&session.bitmap);
        let w = base_ref.width();
        let h = base_ref.height();
        let rgba = base_ref.to_rgba();
        self.previews.clear();
        for entry in &self.entries {
            let (pw, ph, out) = preview_effect_on_rgba(w, h, &rgba, &entry.config, max_dim);
            self.previews.push(EffectPreviewThumbnail {
                id: entry.id,
                width: pw,
                height: ph,
                rgba: out,
            });
        }
    }

    /// Apply a sequence of effects directly to `session` (legacy direct helper).
    pub fn apply_selected(&self, session: &mut DocumentSession, idx: usize) -> bool {
        if let Some(entry) = self.entries.get(idx) {
            session.apply_effect(&entry.config)
        } else {
            false
        }
    }

    /// Save a favorite effect preset.
    pub fn save_preset(&mut self, name: impl Into<String>, config: EffectConfig) {
        self.saved_presets.push((name.into(), config));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_model::SnagxPage;

    #[test]
    fn fifteen_effects_live_thumbnails_and_multi_effect_undo() {
        let bmp = TiledBitmap::new_filled(80, 60, ColorRgba::rgb(200, 220, 240));
        let orig = bmp.clone();
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);
        let mut gallery = EffectsGalleryState::default();
        assert_eq!(gallery.entries.len(), 15);

        gallery.refresh_live_thumbnails(&session, 32);
        assert_eq!(gallery.previews.len(), 15);
        assert!(gallery.previews.iter().all(|p| !p.rgba.is_empty()));

        // 1. Click Border once -> ON (expands 80x60 -> 92x72)
        assert_eq!(gallery.toggle_effect(&mut session, 0, None), Some(true));
        assert_eq!(gallery.active_count(), 1);
        assert_eq!((session.bitmap.width(), session.bitmap.height()), (92, 72));

        // 2. Adjust Border width from 6 -> 10 while ON -> recomputes cleanly from 80x60 -> 100x80 (NOT 92+20=112!)
        gallery.update_effect_config(
            &mut session,
            0,
            EffectConfig::Border {
                width: 10,
                color: ColorRgba::rgb(217, 43, 43),
                inside: false,
            },
            true,
        );
        assert_eq!((session.bitmap.width(), session.bitmap.height()), (100, 80));

        // 3. Click Spotlight & Magnify (idx 9) -> ON + initializes centered spotlight rect & interactive gizmo
        assert_eq!(gallery.toggle_effect(&mut session, 9, None), Some(true));
        assert_eq!(gallery.active_count(), 2);
        let (spot_idx, spot_rect) = gallery.active_spotlight_rect().expect("active spotlight");
        assert_eq!(spot_idx, 9);
        assert_eq!(spot_rect, RectF::new(20.0, 15.0, 40.0, 30.0));

        // Drag spotlight inside & resize SE corner handle
        let moved = EffectsGalleryState::drag_spotlight_rect(
            spot_rect,
            EffectGizmoPart::MoveInside,
            10.0,
            5.0,
            80,
            60,
        );
        assert_eq!(moved, RectF::new(30.0, 20.0, 40.0, 30.0));
        let resized = EffectsGalleryState::drag_spotlight_rect(
            moved,
            EffectGizmoPart::SE,
            8.0,
            6.0,
            80,
            60,
        );
        assert_eq!(resized, RectF::new(30.0, 20.0, 48.0, 36.0));

        // 4. Click Border again -> OFF, then click Spotlight again -> OFF -> exact original 80x60 bitmap restored!
        assert_eq!(gallery.toggle_effect(&mut session, 0, None), Some(false));
        assert_eq!(gallery.toggle_effect(&mut session, 9, None), Some(false));
        assert_eq!(gallery.active_count(), 0);
        assert_eq!(session.bitmap, orig);
    }
}
