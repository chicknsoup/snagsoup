use crate::tile_bitmap::{encode_rgba_to_png, PixelRect, TiledBitmap};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use snagsoup_model::{
    CaptureObject, ColorRgba, PointF, Section, SnagTemplate, SnagxPage, ToolMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombineLayout {
    Horizontal,
    Vertical,
    Grid { columns: u32 },
}

#[derive(Debug, Clone)]
pub struct CombineOptions {
    pub layout: CombineLayout,
    pub spacing: u32,
    pub padding: u32,
    pub background: ColorRgba,
}

impl Default for CombineOptions {
    fn default() -> Self {
        Self {
            layout: CombineLayout::Horizontal,
            spacing: 16,
            padding: 20,
            background: ColorRgba::WHITE,
        }
    }
}

/// Combines multiple `TiledBitmap`s into a single stitched `TiledBitmap` using Horizontal, Vertical, or Grid layout.
pub fn combine_images(images: &[&TiledBitmap], opts: &CombineOptions) -> TiledBitmap {
    if images.is_empty() {
        return TiledBitmap::new_empty(0, 0);
    }
    let cols = match opts.layout {
        CombineLayout::Horizontal => images.len() as u32,
        CombineLayout::Vertical => 1,
        CombineLayout::Grid { columns } => columns.max(1).min(images.len() as u32),
    };
    let rows = (images.len() as u32).div_ceil(cols);

    let mut col_widths = vec![0u32; cols as usize];
    let mut row_heights = vec![0u32; rows as usize];
    for (i, img) in images.iter().enumerate() {
        let c = i % (cols as usize);
        let r = i / (cols as usize);
        col_widths[c] = col_widths[c].max(img.width());
        row_heights[r] = row_heights[r].max(img.height());
    }

    let total_w = opts.padding * 2
        + col_widths.iter().sum::<u32>()
        + opts.spacing * cols.saturating_sub(1);
    let total_h = opts.padding * 2
        + row_heights.iter().sum::<u32>()
        + opts.spacing * rows.saturating_sub(1);

    let mut out = TiledBitmap::new_filled(total_w, total_h, opts.background);

    let mut y_off = opts.padding;
    for r in 0..rows as usize {
        let mut x_off = opts.padding;
        for c in 0..cols as usize {
            let idx = r * (cols as usize) + c;
            if let Some(img) = images.get(idx) {
                // Center image inside its cell
                let cx = x_off + (col_widths[c] - img.width()) / 2;
                let cy = y_off + (row_heights[r] - img.height()) / 2;
                let src = img.to_rgba();
                let sw = img.width() as usize;
                let dest = PixelRect::new(cx, cy, img.width(), img.height());
                out.mutate_region(dest, |x, y, px| {
                    let sx = (x - cx) as usize;
                    let sy = (y - cy) as usize;
                    let off = (sy * sw + sx) * 4;
                    px.copy_from_slice(&src[off..off + 4]);
                });
            }
            x_off += col_widths[c] + opts.spacing;
        }
        y_off += row_heights[r] + opts.spacing;
    }

    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZoneFitMode {
    Fill,
    Fit,
}

/// Interactive `.snagt` Template Engine: assigns images to `DropZone`s, swaps `DropZone`s,
/// duplicates/deletes/reorders `Sections`, and auto-renumbers `Step` badges (`1, 2, 3...`).
pub struct TemplateEngine {
    pub page: SnagxPage,
}

impl TemplateEngine {
    pub fn from_template(template: SnagTemplate) -> Self {
        Self { page: template }
    }

    /// Collects mutable references to all `DropZone` objects across `sections` and `capture_objects`.
    fn all_dropzones_mut(&mut self) -> Vec<&mut CaptureObject> {
        let mut list = Vec::new();
        if let Some(ref mut sections) = self.page.sections {
            for sec in sections {
                if let Some(ref mut graphics) = sec.section_graphics {
                    for obj in graphics {
                        if obj.tool_mode == Some(ToolMode::DropZone)
                            || obj.drop_zone_group_id.unwrap_or(0) > 0
                        {
                            list.push(obj);
                        }
                    }
                }
            }
        }
        for obj in &mut self.page.capture_objects {
            if obj.tool_mode == Some(ToolMode::DropZone)
                || obj.drop_zone_group_id.unwrap_or(0) > 0
            {
                list.push(obj);
            }
        }
        list
    }

    pub fn dropzone_count(&mut self) -> usize {
        self.all_dropzones_mut().len()
    }

    /// Assigns an RGBA8 `TiledBitmap` into the `dz_index`-th `DropZone` with `Fill` or `Fit` scaling + pan offset.
    pub fn assign_image_to_dropzone(
        &mut self,
        dz_index: usize,
        bmp: &TiledBitmap,
        fit_mode: DropZoneFitMode,
        pan_offset: PointF,
    ) -> crate::Result<bool> {
        let png = encode_rgba_to_png(bmp.width(), bmp.height(), &bmp.to_rgba())?;
        let b64 = BASE64.encode(&png);
        let mut zones = self.all_dropzones_mut();
        let Some(dz) = zones.get_mut(dz_index) else {
            return Ok(false);
        };
        dz.image = Some(b64);
        dz.image_size = Some(PointF::new(bmp.width() as f64, bmp.height() as f64));
        dz.image_position = Some(pan_offset);
        dz.extra.insert(
            "ImageFitMode".to_string(),
            serde_json::Value::String(match fit_mode {
                DropZoneFitMode::Fill => "Fill".to_string(),
                DropZoneFitMode::Fit => "Fit".to_string(),
            }),
        );
        Ok(true)
    }

    /// Swaps the assigned images & positions between two `DropZone`s.
    pub fn swap_dropzones(&mut self, idx_a: usize, idx_b: usize) -> bool {
        if idx_a == idx_b {
            return true;
        }
        let mut zones = self.all_dropzones_mut();
        if idx_a >= zones.len() || idx_b >= zones.len() {
            return false;
        }
        let img_a = zones[idx_a].image.clone();
        let sz_a = zones[idx_a].image_size;
        let pos_a = zones[idx_a].image_position;

        zones[idx_a].image = zones[idx_b].image.clone();
        zones[idx_a].image_size = zones[idx_b].image_size;
        zones[idx_a].image_position = zones[idx_b].image_position;

        zones[idx_b].image = img_a;
        zones[idx_b].image_size = sz_a;
        zones[idx_b].image_position = pos_a;
        true
    }

    /// Duplicates a `Section` at `section_idx` and appends it right after, auto-renumbering `Step` badges.
    pub fn duplicate_section(&mut self, section_idx: usize) -> bool {
        let Some(ref mut sections) = self.page.sections else {
            return false;
        };
        let Some(clone) = sections.get(section_idx).cloned() else {
            return false;
        };
        sections.insert(section_idx + 1, clone);
        self.renumber_steps();
        true
    }

    /// Deletes a `Section` at `section_idx` and auto-renumbers remaining `Step` badges.
    pub fn delete_section(&mut self, section_idx: usize) -> Option<Section> {
        let sections = self.page.sections.as_mut()?;
        if section_idx >= sections.len() {
            return None;
        }
        let removed = sections.remove(section_idx);
        self.renumber_steps();
        Some(removed)
    }

    /// Reorders a `Section` from `from` to `to` and auto-renumbers `Step` badges.
    pub fn reorder_section(&mut self, from: usize, to: usize) -> bool {
        let Some(ref mut sections) = self.page.sections else {
            return false;
        };
        if from >= sections.len() || to >= sections.len() || from == to {
            return false;
        }
        let sec = sections.remove(from);
        sections.insert(to, sec);
        self.renumber_steps();
        true
    }

    /// Renumbers all `ToolMode::Step` objects across sections & capture objects sequentially (`1, 2, 3...`).
    pub fn renumber_steps(&mut self) {
        let mut next = 1u32;
        if let Some(ref mut sections) = self.page.sections {
            for sec in sections {
                if let Some(ref mut graphics) = sec.section_graphics {
                    for obj in graphics {
                        if obj.tool_mode == Some(ToolMode::Step) {
                            obj.step_value = Some(next.to_string());
                            next += 1;
                        }
                    }
                }
            }
        }
        for obj in &mut self.page.capture_objects {
            if obj.tool_mode == Some(ToolMode::Step) {
                obj.step_value = Some(next.to_string());
                next += 1;
            }
        }
        self.page.step_tool_sequence_numeric = Some(next as i64);
    }
}
