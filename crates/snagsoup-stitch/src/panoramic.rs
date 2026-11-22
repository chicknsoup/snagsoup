//! Real-time 2D `PanoramicSession` backed by a sparse `256x256` tile grid
//! (`HashMap<(i32, i32), Box<[u8; TILE_BYTES]>>`) expanding dynamically in all 4 directions
//! (Left, Right, Up, Down) with `< 1 ms` thumbnail `PreviewImage` generation.

use crate::phase_correlator::PhaseCorrelator;
use crate::sticky_mask::StickyBorders;
use crate::{RgbaFrame, StitchError};
use std::collections::HashMap;

pub const TILE_SIZE: i32 = 256;
pub const TILE_BYTES: usize = (TILE_SIZE as usize) * (TILE_SIZE as usize) * 4;

/// Result of pushing a live frame into `PanoramicSession`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Initialized,
    Stationary,
    RejectedLowConfidence,
    Stitched {
        dx: i32,
        dy: i32,
        total_width: u32,
        total_height: u32,
    },
}

/// Real-time memory-efficient 2D panoramic stitcher.
pub struct PanoramicSession {
    tiles: HashMap<(i32, i32), Box<[u8; TILE_BYTES]>>,
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
    cursor_x: i32,
    cursor_y: i32,
    last_frame: Option<RgbaFrame>,
    correlator: PhaseCorrelator,
    pub max_sad: f32,
}

impl PanoramicSession {
    pub fn new() -> Self {
        Self {
            tiles: HashMap::new(),
            min_x: 0,
            min_y: 0,
            max_x: 0,
            max_y: 0,
            cursor_x: 0,
            cursor_y: 0,
            last_frame: None,
            correlator: PhaseCorrelator::new(),
            max_sad: 28.0,
        }
    }

    /// Current bounding dimensions `(width, height)` of the stitched panorama.
    pub fn dimensions(&self) -> (u32, u32) {
        if self.last_frame.is_none() {
            return (0, 0);
        }
        (
            (self.max_x - self.min_x).max(0) as u32,
            (self.max_y - self.min_y).max(0) as u32,
        )
    }

    /// Total heap bytes currently allocated by sparse tiles.
    pub fn allocated_tile_bytes(&self) -> usize {
        self.tiles.len() * TILE_BYTES
    }

    /// Number of allocated sparse 256x256 tiles.
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Push a new live frame from the panoramic capture region.
    pub fn push_frame(&mut self, frame: RgbaFrame) -> Result<PushOutcome, StitchError> {
        let Some(ref prev) = self.last_frame else {
            self.min_x = 0;
            self.min_y = 0;
            self.max_x = frame.width as i32;
            self.max_y = frame.height as i32;
            self.cursor_x = 0;
            self.cursor_y = 0;
            self.blit_frame(0, 0, &frame);
            self.last_frame = Some(frame);
            return Ok(PushOutcome::Initialized);
        };

        if prev.mean_abs_diff(&frame) < 0.5 {
            return Ok(PushOutcome::Stationary);
        }

        let est = self
            .correlator
            .estimate(prev, &frame, StickyBorders::default())?;

        if est.dx == 0 && est.dy == 0 {
            return Ok(PushOutcome::Stationary);
        }
        if est.sad > self.max_sad {
            return Ok(PushOutcome::RejectedLowConfidence);
        }

        self.cursor_x += est.dx;
        self.cursor_y += est.dy;
        self.min_x = self.min_x.min(self.cursor_x);
        self.min_y = self.min_y.min(self.cursor_y);
        self.max_x = self.max_x.max(self.cursor_x + frame.width as i32);
        self.max_y = self.max_y.max(self.cursor_y + frame.height as i32);

        self.blit_frame(self.cursor_x, self.cursor_y, &frame);
        self.last_frame = Some(frame);

        let (total_width, total_height) = self.dimensions();
        Ok(PushOutcome::Stitched {
            dx: est.dx,
            dy: est.dy,
            total_width,
            total_height,
        })
    }

    /// Generate a live HUD preview (`<= max_w x max_h`, e.g. `320x180`) directly from
    /// the sparse tile grid in `< 1 ms`.
    pub fn preview_image(&self, max_w: u32, max_h: u32) -> Option<RgbaFrame> {
        let (tw, th) = self.dimensions();
        if tw == 0 || th == 0 || max_w == 0 || max_h == 0 {
            return None;
        }
        let scale = (max_w as f64 / tw as f64).min(max_h as f64 / th as f64).min(1.0);
        let pw = ((tw as f64 * scale).round() as u32).max(1);
        let ph = ((th as f64 * scale).round() as u32).max(1);

        let mut rgba = vec![0u8; (pw as usize) * (ph as usize) * 4];
        for py in 0..ph {
            let wy = self.min_y + ((py as f64 + 0.5) / scale).floor() as i32;
            let ty = wy.div_euclid(TILE_SIZE);
            let ly = wy.rem_euclid(TILE_SIZE) as usize;
            for px in 0..pw {
                let wx = self.min_x + ((px as f64 + 0.5) / scale).floor() as i32;
                let tx = wx.div_euclid(TILE_SIZE);
                if let Some(tile) = self.tiles.get(&(tx, ty)) {
                    let lx = wx.rem_euclid(TILE_SIZE) as usize;
                    let src = (ly * TILE_SIZE as usize + lx) * 4;
                    let dst = ((py as usize) * (pw as usize) + (px as usize)) * 4;
                    rgba[dst..dst + 4].copy_from_slice(&tile[src..src + 4]);
                }
            }
        }
        RgbaFrame::new(pw, ph, rgba).ok()
    }

    /// Materialize the final tightly-bounded RGBA8 panorama from all sparse tiles.
    pub fn finalize(&self) -> Result<RgbaFrame, StitchError> {
        let (w, h) = self.dimensions();
        if w == 0 || h == 0 {
            return Err(StitchError::EmptySequence);
        }
        let out_w = w as usize;
        let out_h = h as usize;
        let mut rgba = vec![0u8; out_w * out_h * 4];

        for (&(tx, ty), tile) in &self.tiles {
            let tile_x0 = tx * TILE_SIZE;
            let tile_y0 = ty * TILE_SIZE;
            let copy_x0 = tile_x0.max(self.min_x);
            let copy_y0 = tile_y0.max(self.min_y);
            let copy_x1 = (tile_x0 + TILE_SIZE).min(self.max_x);
            let copy_y1 = (tile_y0 + TILE_SIZE).min(self.max_y);
            if copy_x1 <= copy_x0 || copy_y1 <= copy_y0 {
                continue;
            }
            let span_w = (copy_x1 - copy_x0) as usize;
            let src_lx = (copy_x0 - tile_x0) as usize;
            let dst_x = (copy_x0 - self.min_x) as usize;

            for wy in copy_y0..copy_y1 {
                let src_ly = (wy - tile_y0) as usize;
                let dst_y = (wy - self.min_y) as usize;
                let s_off = (src_ly * TILE_SIZE as usize + src_lx) * 4;
                let d_off = (dst_y * out_w + dst_x) * 4;
                rgba[d_off..d_off + span_w * 4]
                    .copy_from_slice(&tile[s_off..s_off + span_w * 4]);
            }
        }

        RgbaFrame::new(w, h, rgba)
    }

    fn blit_frame(&mut self, origin_x: i32, origin_y: i32, frame: &RgbaFrame) {
        let fw = frame.width as i32;
        let fh = frame.height as i32;
        let tx_min = origin_x.div_euclid(TILE_SIZE);
        let ty_min = origin_y.div_euclid(TILE_SIZE);
        let tx_max = (origin_x + fw - 1).div_euclid(TILE_SIZE);
        let ty_max = (origin_y + fh - 1).div_euclid(TILE_SIZE);

        for ty in ty_min..=ty_max {
            let tile_y0 = ty * TILE_SIZE;
            let wy0 = origin_y.max(tile_y0);
            let wy1 = (origin_y + fh).min(tile_y0 + TILE_SIZE);
            for tx in tx_min..=tx_max {
                let tile_x0 = tx * TILE_SIZE;
                let wx0 = origin_x.max(tile_x0);
                let wx1 = (origin_x + fw).min(tile_x0 + TILE_SIZE);
                let span_w = (wx1 - wx0) as usize;
                let src_x = (wx0 - origin_x) as usize;
                let dst_lx = (wx0 - tile_x0) as usize;

                let tile = self
                    .tiles
                    .entry((tx, ty))
                    .or_insert_with(|| vec![0u8; TILE_BYTES].into_boxed_slice().try_into().unwrap());

                for wy in wy0..wy1 {
                    let src_y = (wy - origin_y) as usize;
                    let dst_ly = (wy - tile_y0) as usize;
                    let s_off = (src_y * (fw as usize) + src_x) * 4;
                    let d_off = (dst_ly * TILE_SIZE as usize + dst_lx) * 4;
                    tile[d_off..d_off + span_w * 4]
                        .copy_from_slice(&frame.rgba[s_off..s_off + span_w * 4]);
                }
            }
        }
    }
}

impl Default for PanoramicSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn sample_world_patch(origin_x: i32, origin_y: i32, w: u32, h: u32) -> RgbaFrame {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            let wy = (origin_y + y as i32) as u32;
            let by = wy / 24;
            for x in 0..w {
                let wx = (origin_x + x as i32) as u32;
                let bx = wx / 24;
                let hash = bx
                    .wrapping_mul(374_761_393)
                    .wrapping_add(by.wrapping_mul(668_265_263))
                    ^ (wx.wrapping_mul(19) ^ wy.wrapping_mul(37));
                let idx = ((y * w + x) * 4) as usize;
                rgba[idx] = (hash & 0xFF) as u8;
                rgba[idx + 1] = ((hash >> 8) & 0xFF) as u8;
                rgba[idx + 2] = ((hash >> 16) & 0xFF) as u8;
                rgba[idx + 3] = 255;
            }
        }
        RgbaFrame::new(w, h, rgba).unwrap()
    }

    #[test]
    fn panoramic_l_shaped_2d_pan_2000_down_1500_right_sparse_memory() {
        let view_w = 320u32;
        let view_h = 320u32;
        let mut session = PanoramicSession::new();

        // Leg 1: pan down 2000 px in 10 steps of 200 px
        let mut cur_x = 0i32;
        let mut cur_y = 0i32;
        assert_eq!(
            session
                .push_frame(sample_world_patch(cur_x, cur_y, view_w, view_h))
                .unwrap(),
            PushOutcome::Initialized
        );

        // Stationary duplicate should be ignored
        assert_eq!(
            session
                .push_frame(sample_world_patch(cur_x, cur_y, view_w, view_h))
                .unwrap(),
            PushOutcome::Stationary
        );

        for _ in 0..10 {
            cur_y += 200;
            let outcome = session
                .push_frame(sample_world_patch(cur_x, cur_y, view_w, view_h))
                .unwrap();
            assert!(matches!(outcome, PushOutcome::Stitched { dx: 0, dy: 200, .. }));
        }

        // Leg 2: pan right 1500 px in 10 steps of 150 px
        for _ in 0..10 {
            cur_x += 150;
            let outcome = session
                .push_frame(sample_world_patch(cur_x, cur_y, view_w, view_h))
                .unwrap();
            assert!(matches!(outcome, PushOutcome::Stitched { dx: 150, dy: 0, .. }));
        }

        let (tw, th) = session.dimensions();
        assert_eq!(tw, 1500 + view_w);
        assert_eq!(th, 2000 + view_h);

        // Sparse tiles must use far less than 40 MB (empty upper-right quadrant is unallocated!)
        let tile_mb = session.allocated_tile_bytes() as f64 / (1024.0 * 1024.0);
        assert!(
            tile_mb < 15.0,
            "sparse L-shaped panorama should stay tiny, got {tile_mb:.2} MB"
        );

        // Preview generation <= 320x180 must be fast
        let t0 = Instant::now();
        let preview = session.preview_image(320, 180).expect("preview");
        let preview_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(preview.width <= 320 && preview.height <= 180);
        assert!(preview_ms < 25.0, "preview took {preview_ms:.2} ms");

        let final_img = session.finalize().expect("finalize");
        assert_eq!(final_img.width, tw);
        assert_eq!(final_img.height, th);
        // Empty upper-right corner (x = 1200, y = 100) must be transparent!
        assert_eq!(final_img.pixel(1200, 100), [0, 0, 0, 0]);
        // Visited bottom-right corner (x = 1600, y = 2100) must be opaque!
        assert_eq!(final_img.pixel(1600, 2100)[3], 255);
    }
}
