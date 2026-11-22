use crate::tile_bitmap::{PixelRect, TileCoord, TiledBitmap, TILE_BYTES, TILE_SIZE};
use snagsoup_model::{CaptureObject, SnagxPage};
use std::collections::{BTreeSet, HashSet, VecDeque};

/// Tracks which pixel region and tile coordinates have changed since the last GPU upload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtyRegion {
    pub bounds: Option<PixelRect>,
    pub tiles: BTreeSet<TileCoord>,
}

impl DirtyRegion {
    pub fn is_dirty(&self) -> bool {
        self.bounds.is_some() || !self.tiles.is_empty()
    }

    pub fn mark_rect(&mut self, rect: PixelRect) {
        if rect.is_empty() {
            return;
        }
        self.bounds = Some(match self.bounds {
            Some(prev) => prev.union(rect),
            None => rect,
        });
        let tx0 = rect.x / TILE_SIZE;
        let ty0 = rect.y / TILE_SIZE;
        let tx1 = (rect.right() - 1) / TILE_SIZE;
        let ty1 = (rect.bottom() - 1) / TILE_SIZE;
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                self.tiles.insert(TileCoord::new(tx, ty));
            }
        }
    }

    pub fn mark_tiles(&mut self, coords: impl IntoIterator<Item = TileCoord>) {
        for c in coords {
            self.tiles.insert(c);
        }
    }

    pub fn mark_all(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.mark_rect(PixelRect::new(0, 0, width, height));
        }
    }

    pub fn take(&mut self) -> DirtyRegion {
        std::mem::take(self)
    }
}

/// Undoable document command (vector deltas are lightweight; raster edits share CoW `Arc` tiles).
#[derive(Debug, Clone)]
pub enum HistoryCommand {
    AddObject {
        index: usize,
        object: CaptureObject,
    },
    UpdateObject {
        index: usize,
        before: CaptureObject,
        after: CaptureObject,
    },
    DeleteObject {
        index: usize,
        object: CaptureObject,
    },
    ReorderObject {
        from: usize,
        to: usize,
    },
    RasterSnapshot {
        before: TiledBitmap,
        after: TiledBitmap,
        dirty_rect: PixelRect,
    },
    CanvasResize {
        before_bitmap: TiledBitmap,
        after_bitmap: TiledBitmap,
        before_objects: Vec<CaptureObject>,
        after_objects: Vec<CaptureObject>,
    },
}

/// Bounded Undo/Redo stack (default 50 steps) with CoW tile memory accounting.
#[derive(Debug, Clone)]
pub struct CommandHistory {
    max_steps: usize,
    undo_stack: VecDeque<HistoryCommand>,
    redo_stack: Vec<HistoryCommand>,
}

impl Default for CommandHistory {
    fn default() -> Self {
        Self::new(50)
    }
}

impl CommandHistory {
    pub fn new(max_steps: usize) -> Self {
        Self {
            max_steps: max_steps.max(1),
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn max_steps(&self) -> usize {
        self.max_steps
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn push(&mut self, cmd: HistoryCommand) {
        self.redo_stack.clear();
        if self.undo_stack.len() >= self.max_steps {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(cmd);
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Computes total unique tile bytes across `current` plus all raster snapshots in history.
    pub fn total_unique_tile_bytes(&self, current: &TiledBitmap) -> usize {
        let mut ptrs = HashSet::new();
        current.collect_unique_tile_ptrs(&mut ptrs);
        for cmd in self.undo_stack.iter().chain(self.redo_stack.iter()) {
            match cmd {
                HistoryCommand::RasterSnapshot { before, after, .. } => {
                    before.collect_unique_tile_ptrs(&mut ptrs);
                    after.collect_unique_tile_ptrs(&mut ptrs);
                }
                HistoryCommand::CanvasResize {
                    before_bitmap,
                    after_bitmap,
                    ..
                } => {
                    before_bitmap.collect_unique_tile_ptrs(&mut ptrs);
                    after_bitmap.collect_unique_tile_ptrs(&mut ptrs);
                }
                _ => {}
            }
        }
        ptrs.len() * TILE_BYTES
    }

    /// Computes ONLY the incremental tile bytes held by history beyond `base` (the initial bitmap).
    pub fn incremental_tile_bytes_beyond(&self, base: &TiledBitmap, current: &TiledBitmap) -> usize {
        let mut base_ptrs = HashSet::new();
        base.collect_unique_tile_ptrs(&mut base_ptrs);

        let mut all_ptrs = HashSet::new();
        current.collect_unique_tile_ptrs(&mut all_ptrs);
        for cmd in self.undo_stack.iter().chain(self.redo_stack.iter()) {
            match cmd {
                HistoryCommand::RasterSnapshot { before, after, .. } => {
                    before.collect_unique_tile_ptrs(&mut all_ptrs);
                    after.collect_unique_tile_ptrs(&mut all_ptrs);
                }
                HistoryCommand::CanvasResize {
                    before_bitmap,
                    after_bitmap,
                    ..
                } => {
                    before_bitmap.collect_unique_tile_ptrs(&mut all_ptrs);
                    after_bitmap.collect_unique_tile_ptrs(&mut all_ptrs);
                }
                _ => {}
            }
        }

        let extra_tiles = all_ptrs.difference(&base_ptrs).count();
        extra_tiles * TILE_BYTES
    }
}

/// Editable document session uniting `SnagxPage`, `TiledBitmap`, `CommandHistory`, and `DirtyRegion`.
#[derive(Debug, Clone)]
pub struct DocumentSession {
    pub page: SnagxPage,
    pub bitmap: TiledBitmap,
    pub history: CommandHistory,
    pub dirty: DirtyRegion,
}

impl DocumentSession {
    pub fn new(page: SnagxPage, bitmap: TiledBitmap) -> Self {
        let mut dirty = DirtyRegion::default();
        dirty.mark_all(bitmap.width(), bitmap.height());
        Self {
            page,
            bitmap,
            history: CommandHistory::default(),
            dirty,
        }
    }

    pub fn with_max_history(mut self, max_steps: usize) -> Self {
        self.history = CommandHistory::new(max_steps);
        self
    }

    pub fn add_object(&mut self, object: CaptureObject) -> usize {
        let index = self.page.capture_objects.len();
        self.page.capture_objects.push(object.clone());
        self.history.push(HistoryCommand::AddObject { index, object });
        self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
        index
    }

    pub fn update_object(&mut self, index: usize, after: CaptureObject) -> bool {
        let Some(slot) = self.page.capture_objects.get_mut(index) else {
            return false;
        };
        let before = std::mem::replace(slot, after.clone());
        self.history.push(HistoryCommand::UpdateObject {
            index,
            before,
            after,
        });
        self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
        true
    }

    pub fn delete_object(&mut self, index: usize) -> Option<CaptureObject> {
        if index >= self.page.capture_objects.len() {
            return None;
        }
        let object = self.page.capture_objects.remove(index);
        self.history.push(HistoryCommand::DeleteObject {
            index,
            object: object.clone(),
        });
        self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
        Some(object)
    }

    pub fn reorder_object(&mut self, from: usize, to: usize) -> bool {
        let len = self.page.capture_objects.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        let obj = self.page.capture_objects.remove(from);
        self.page.capture_objects.insert(to, obj);
        self.history.push(HistoryCommand::ReorderObject { from, to });
        self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
        true
    }

    /// Performs a CoW regional raster edit and records a lightweight `RasterSnapshot` in Undo history.
    pub fn mutate_raster_region<F>(&mut self, rect: PixelRect, f: F) -> Vec<TileCoord>
    where
        F: FnMut(u32, u32, &mut [u8; 4]),
    {
        let Some(clipped) = rect.intersect(self.bitmap.bounds()) else {
            return Vec::new();
        };
        let before = self.bitmap.clone(); // O(tiles) Arc::clone only — zero pixel copies!
        let modified = self.bitmap.mutate_region(clipped, f);
        let after = self.bitmap.clone();
        self.history.push(HistoryCommand::RasterSnapshot {
            before,
            after,
            dirty_rect: clipped,
        });
        self.dirty.mark_rect(clipped);
        self.dirty.mark_tiles(modified.iter().copied());
        modified
    }

    pub fn undo(&mut self) -> bool {
        let Some(cmd) = self.history.undo_stack.pop_back() else {
            return false;
        };
        match &cmd {
            HistoryCommand::AddObject { index, .. } => {
                if *index < self.page.capture_objects.len() {
                    self.page.capture_objects.remove(*index);
                }
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
            HistoryCommand::UpdateObject { index, before, .. } => {
                if let Some(slot) = self.page.capture_objects.get_mut(*index) {
                    *slot = before.clone();
                }
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
            HistoryCommand::DeleteObject { index, object } => {
                let idx = (*index).min(self.page.capture_objects.len());
                self.page.capture_objects.insert(idx, object.clone());
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
            HistoryCommand::ReorderObject { from, to } => {
                if *to < self.page.capture_objects.len() {
                    let obj = self.page.capture_objects.remove(*to);
                    let idx = (*from).min(self.page.capture_objects.len());
                    self.page.capture_objects.insert(idx, obj);
                }
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
            HistoryCommand::RasterSnapshot {
                before, dirty_rect, ..
            } => {
                self.bitmap = before.clone();
                self.dirty.mark_rect(*dirty_rect);
            }
            HistoryCommand::CanvasResize {
                before_bitmap,
                before_objects,
                ..
            } => {
                self.bitmap = before_bitmap.clone();
                self.page.capture_objects = before_objects.clone();
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
        }
        self.history.redo_stack.push(cmd);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(cmd) = self.history.redo_stack.pop() else {
            return false;
        };
        match &cmd {
            HistoryCommand::AddObject { index, object } => {
                let idx = (*index).min(self.page.capture_objects.len());
                self.page.capture_objects.insert(idx, object.clone());
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
            HistoryCommand::UpdateObject { index, after, .. } => {
                if let Some(slot) = self.page.capture_objects.get_mut(*index) {
                    *slot = after.clone();
                }
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
            HistoryCommand::DeleteObject { index, .. } => {
                if *index < self.page.capture_objects.len() {
                    self.page.capture_objects.remove(*index);
                }
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
            HistoryCommand::ReorderObject { from, to } => {
                if *from < self.page.capture_objects.len() {
                    let obj = self.page.capture_objects.remove(*from);
                    let idx = (*to).min(self.page.capture_objects.len());
                    self.page.capture_objects.insert(idx, obj);
                }
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
            HistoryCommand::RasterSnapshot {
                after, dirty_rect, ..
            } => {
                self.bitmap = after.clone();
                self.dirty.mark_rect(*dirty_rect);
            }
            HistoryCommand::CanvasResize {
                after_bitmap,
                after_objects,
                ..
            } => {
                self.bitmap = after_bitmap.clone();
                self.page.capture_objects = after_objects.clone();
                self.dirty.mark_all(self.bitmap.width(), self.bitmap.height());
            }
        }
        self.history.undo_stack.push_back(cmd);
        true
    }

    pub fn take_dirty(&mut self) -> DirtyRegion {
        self.dirty.take()
    }

    /// Applies a global `EffectConfig` to `self.bitmap` as a single undoable history step.
    pub fn apply_effect(&mut self, effect: &snagsoup_model::EffectConfig) -> bool {
        let w = self.bitmap.width();
        let h = self.bitmap.height();
        if w == 0 || h == 0 {
            return false;
        }
        let before_bitmap = self.bitmap.clone();
        let before_objects = self.page.capture_objects.clone();
        let (nw, nh, out_rgba) =
            crate::effects_geom::apply_effect(w, h, self.bitmap.to_rgba(), effect);
        let Ok(after_bitmap) = TiledBitmap::from_rgba(nw, nh, &out_rgba) else {
            return false;
        };
        self.page.capture_canvas_width = Some(nw as f64);
        self.page.capture_canvas_height = Some(nh as f64);
        self.bitmap = after_bitmap.clone();
        let after_objects = self.page.capture_objects.clone();
        self.history.push(HistoryCommand::CanvasResize {
            before_bitmap,
            after_bitmap,
            before_objects,
            after_objects,
        });
        self.dirty.mark_all(nw, nh);
        true
    }
}
