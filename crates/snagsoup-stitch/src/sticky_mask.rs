//! Automatic detection of stationary sticky headers, sticky footers, and scrollbars
//! across consecutive scrolled frames.

use crate::RgbaFrame;

/// Pixel thickness of stationary UI chrome on each edge of a scrolled viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StickyBorders {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

impl StickyBorders {
    /// Return the unmasked interior `(x0, y0, x1, y1)` for a frame of size `(width, height)`.
    pub fn inner_rect(&self, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
        let x0 = self.left.min(width);
        let y0 = self.top.min(height);
        let x1 = width.saturating_sub(self.right);
        let y1 = height.saturating_sub(self.bottom);
        if x1 > x0 + 8 && y1 > y0 + 8 {
            Some((x0, y0, x1, y1))
        } else {
            None
        }
    }
}

/// Detects stationary sticky UI borders between scrolled frames.
pub struct StickyBorderMasker;

impl StickyBorderMasker {
    /// Detect sticky borders between a single pair of consecutive frames.
    ///
    /// If the two frames are essentially identical (stationary), returns `StickyBorders::default()`.
    pub fn detect_between(
        prev: &RgbaFrame,
        next: &RgbaFrame,
        pixel_diff_threshold: u8,
    ) -> StickyBorders {
        if prev.width != next.width || prev.height != next.height {
            return StickyBorders::default();
        }
        if prev.mean_abs_diff(next) < 1.0 {
            return StickyBorders::default();
        }

        let w = prev.width as usize;
        let h = prev.height as usize;
        let thresh = pixel_diff_threshold as u32;

        // Helper: true if >= 96% of sampled pixels in row `y` are unchanged.
        let row_is_static = |y: usize| -> bool {
            let mut static_px = 0usize;
            let mut total = 0usize;
            for x in (0..w).step_by(2) {
                let idx = (y * w + x) * 4;
                let dr = prev.rgba[idx].abs_diff(next.rgba[idx]) as u32;
                let dg = prev.rgba[idx + 1].abs_diff(next.rgba[idx + 1]) as u32;
                let db = prev.rgba[idx + 2].abs_diff(next.rgba[idx + 2]) as u32;
                if dr <= thresh && dg <= thresh && db <= thresh {
                    static_px += 1;
                }
                total += 1;
            }
            total > 0 && static_px * 100 >= total * 96
        };

        let max_v = (h / 3).max(1);
        let mut top = 0u32;
        for y in 0..max_v {
            if row_is_static(y) {
                top += 1;
            } else {
                break;
            }
        }

        let mut bottom = 0u32;
        for y in (h - max_v..h).rev() {
            if row_is_static(y) {
                bottom += 1;
            } else {
                break;
            }
        }

        // Now scan left/right columns within the non-sticky vertical band for scrollbars.
        let y_start = top as usize;
        let y_end = h.saturating_sub(bottom as usize);
        let col_is_static = |x: usize| -> bool {
            let mut static_px = 0usize;
            let mut total = 0usize;
            for y in (y_start..y_end).step_by(2) {
                let idx = (y * w + x) * 4;
                let dr = prev.rgba[idx].abs_diff(next.rgba[idx]) as u32;
                let dg = prev.rgba[idx + 1].abs_diff(next.rgba[idx + 1]) as u32;
                let db = prev.rgba[idx + 2].abs_diff(next.rgba[idx + 2]) as u32;
                if dr <= thresh && dg <= thresh && db <= thresh {
                    static_px += 1;
                }
                total += 1;
            }
            total > 0 && static_px * 100 >= total * 96
        };

        let max_h = (w / 4).min(48);
        let mut left = 0u32;
        for x in 0..max_h {
            if col_is_static(x) {
                left += 1;
            } else {
                break;
            }
        }

        let mut right = 0u32;
        for x in (w.saturating_sub(max_h)..w).rev() {
            if col_is_static(x) {
                right += 1;
            } else {
                break;
            }
        }

        StickyBorders {
            top,
            bottom,
            left,
            right,
        }
    }

    /// Detect consistent sticky borders across an entire sequence of scrolled frames
    /// by taking the minimum across all moving consecutive pairs.
    pub fn detect_sequence(frames: &[RgbaFrame], pixel_diff_threshold: u8) -> StickyBorders {
        let mut acc: Option<StickyBorders> = None;
        for pair in frames.windows(2) {
            if pair[0].mean_abs_diff(&pair[1]) < 1.0 {
                continue;
            }
            let b = Self::detect_between(&pair[0], &pair[1], pixel_diff_threshold);
            acc = Some(match acc {
                None => b,
                Some(prev) => StickyBorders {
                    top: prev.top.min(b.top),
                    bottom: prev.bottom.min(b.bottom),
                    left: prev.left.min(b.left),
                    right: prev.right.min(b.right),
                },
            });
        }
        acc.unwrap_or_default()
    }
}
