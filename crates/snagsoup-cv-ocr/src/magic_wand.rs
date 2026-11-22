use rayon::prelude::*;
use snagsoup_canvas::{PixelRect, TiledBitmap};
use snagsoup_model::PointF;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagicWandMode {
    Contiguous,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionMask {
    pub width: u32,
    pub height: u32,
    /// 255 = selected, 0 = unselected
    pub mask: Vec<u8>,
    pub bounds: Option<PixelRect>,
}

impl SelectionMask {
    pub fn selected_count(&self) -> usize {
        self.mask.iter().filter(|&&v| v > 0).count()
    }

    pub fn is_selected(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.mask[(y as usize) * (self.width as usize) + (x as usize)] > 0
    }
}

/// Magic Wand selection matching `TSCV.Selection.dll` (`Contiguous` BFS & `Global` parallel scan).
pub fn magic_wand_select(
    bmp: &TiledBitmap,
    seed_x: u32,
    seed_y: u32,
    tolerance: f32,
    mode: MagicWandMode,
) -> SelectionMask {
    let w = bmp.width();
    let h = bmp.height();
    let total = (w as usize) * (h as usize);
    if w == 0 || h == 0 || seed_x >= w || seed_y >= h {
        return SelectionMask {
            width: w,
            height: h,
            mask: vec![0; total],
            bounds: None,
        };
    }

    let Some(target) = bmp.get_pixel(seed_x, seed_y) else {
        return SelectionMask {
            width: w,
            height: h,
            mask: vec![0; total],
            bounds: None,
        };
    };

    let rgba = bmp.to_rgba();
    let tol_dist = (tolerance.clamp(0.0, 100.0) / 100.0) * 441.67294;
    let tol_sq = tol_dist * tol_dist;

    let matches = |idx: usize| -> bool {
        let off = idx * 4;
        let dr = rgba[off] as f32 - target[0] as f32;
        let dg = rgba[off + 1] as f32 - target[1] as f32;
        let db = rgba[off + 2] as f32 - target[2] as f32;
        let da = rgba[off + 3] as f32 - target[3] as f32;
        (dr * dr + dg * dg + db * db + da * da) <= tol_sq
    };

    let mut mask = vec![0u8; total];
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;

    match mode {
        MagicWandMode::Global => {
            mask.par_iter_mut().enumerate().for_each(|(idx, slot)| {
                if matches(idx) {
                    *slot = 255;
                }
            });
            for y in 0..h {
                let row = (y as usize) * (w as usize);
                for x in 0..w {
                    if mask[row + x as usize] > 0 {
                        any = true;
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                        min_y = min_y.min(y);
                        max_y = max_y.max(y);
                    }
                }
            }
        }
        MagicWandMode::Contiguous => {
            let mut queue = VecDeque::new();
            queue.push_back((seed_x, seed_y));
            while let Some((x, y)) = queue.pop_front() {
                let row = (y as usize) * (w as usize);
                if mask[row + x as usize] != 0 || !matches(row + x as usize) {
                    continue;
                }
                let mut lx = x;
                while lx > 0 && mask[row + (lx - 1) as usize] == 0 && matches(row + (lx - 1) as usize)
                {
                    lx -= 1;
                }
                let mut rx = x;
                while rx + 1 < w
                    && mask[row + (rx + 1) as usize] == 0
                    && matches(row + (rx + 1) as usize)
                {
                    rx += 1;
                }
                any = true;
                min_x = min_x.min(lx);
                max_x = max_x.max(rx);
                min_y = min_y.min(y);
                max_y = max_y.max(y);

                for cx in lx..=rx {
                    mask[row + cx as usize] = 255;
                    if y > 0 && mask[((y - 1) as usize) * (w as usize) + cx as usize] == 0 {
                        queue.push_back((cx, y - 1));
                    }
                    if y + 1 < h && mask[((y + 1) as usize) * (w as usize) + cx as usize] == 0 {
                        queue.push_back((cx, y + 1));
                    }
                }
            }
        }
    }

    let bounds = if any {
        Some(PixelRect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
    } else {
        None
    };

    SelectionMask {
        width: w,
        height: h,
        mask,
        bounds,
    }
}

/// 3x3 morphological majority filter removing jagged 1px noise along selection boundaries.
pub fn refine_mask(sel: &mut SelectionMask) {
    let w = sel.width as usize;
    let h = sel.height as usize;
    if w < 3 || h < 3 {
        return;
    }
    let src = sel.mask.clone();
    let mut min_x = sel.width;
    let mut min_y = sel.height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;

    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let mut count = 0u8;
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let nx = (x as isize + dx) as usize;
                    let ny = (y as isize + dy) as usize;
                    if src[ny * w + nx] > 0 {
                        count += 1;
                    }
                }
            }
            let v = if count >= 5 { 255 } else { 0 };
            sel.mask[y * w + x] = v;
            if v > 0 {
                any = true;
                min_x = min_x.min(x as u32);
                max_x = max_x.max(x as u32);
                min_y = min_y.min(y as u32);
                max_y = max_y.max(y as u32);
            }
        }
    }
    sel.bounds = if any {
        Some(PixelRect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
    } else {
        None
    };
}

/// Extracts closed marching-squares boundary polylines from `SelectionMask` (for UI marching ants).
pub fn extract_boundary_polylines(sel: &SelectionMask) -> Vec<Vec<PointF>> {
    let Some(b) = sel.bounds else {
        return Vec::new();
    };
    let w = sel.width;
    let h = sel.height;
    let mut edges: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();

    for y in b.y..b.bottom() {
        for x in b.x..b.right() {
            if !sel.is_selected(x, y) {
                continue;
            }
            // Top edge (left -> right)
            if y == 0 || !sel.is_selected(x, y - 1) {
                edges.entry((x, y)).or_default().push((x + 1, y));
            }
            // Right edge (top -> bottom)
            if x + 1 >= w || !sel.is_selected(x + 1, y) {
                edges.entry((x + 1, y)).or_default().push((x + 1, y + 1));
            }
            // Bottom edge (right -> left)
            if y + 1 >= h || !sel.is_selected(x, y + 1) {
                edges.entry((x + 1, y + 1)).or_default().push((x, y + 1));
            }
            // Left edge (bottom -> top)
            if x == 0 || !sel.is_selected(x - 1, y) {
                edges.entry((x, y + 1)).or_default().push((x, y));
            }
        }
    }

    let mut loops = Vec::new();
    while let Some(&start) = edges.keys().next() {
        let mut poly = Vec::new();
        let mut cur = start;
        poly.push(PointF::new(cur.0 as f64, cur.1 as f64));
        loop {
            let next_opt = if let Some(list) = edges.get_mut(&cur) {
                let n = list.pop();
                if list.is_empty() {
                    edges.remove(&cur);
                }
                n
            } else {
                None
            };
            let Some(next) = next_opt else { break };
            cur = next;
            poly.push(PointF::new(cur.0 as f64, cur.1 as f64));
            if cur == start {
                break;
            }
        }
        if poly.len() >= 3 {
            loops.push(poly);
        }
    }
    loops
}
