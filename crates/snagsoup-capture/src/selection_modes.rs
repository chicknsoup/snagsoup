//! All 8 Snagit selection modes and pure pixel-extraction engine:
//! `Region` (+ Guidelines), `FixedRegion`, `Window` (+ Win11 rounded corners),
//! `Fullscreen`, `Freehand` (lasso polygon alpha mask), `MultipleArea` (minimal shared
//! transparent bounding canvas), `Menu`, and `Clipboard`.

use crate::{CaptureError, CapturedFrame};
use snagsoup_model::SelectionMode;
use snagsoup_win_platform::clipboard::read_image_from_clipboard;
use snagsoup_win_platform::dpi::{DisplayTopology, PhysicalPoint, PhysicalRect};

/// Aspect-ratio or fixed-resolution guidelines for `Region` selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuidelineResolution {
    #[default]
    Free,
    Aspect16x9,
    Aspect4x3,
    Aspect1x1,
    Fixed1080p,
    Fixed720p,
}

impl GuidelineResolution {
    /// Constrain a dragged rectangle from `anchor` to `current` according to this guideline.
    pub fn constrain_rect(self, anchor: PhysicalPoint, current: PhysicalPoint) -> PhysicalRect {
        match self {
            Self::Fixed1080p => {
                PhysicalRect::new(anchor.x, anchor.y, anchor.x + 1920, anchor.y + 1080)
            }
            Self::Fixed720p => {
                PhysicalRect::new(anchor.x, anchor.y, anchor.x + 1280, anchor.y + 720)
            }
            Self::Free => normalize_rect(anchor, current),
            Self::Aspect16x9 | Self::Aspect4x3 | Self::Aspect1x1 => {
                let (rw, rh) = match self {
                    Self::Aspect16x9 => (16.0f64, 9.0f64),
                    Self::Aspect4x3 => (4.0, 3.0),
                    Self::Aspect1x1 => (1.0, 1.0),
                    _ => unreachable!(),
                };
                let dx = current.x - anchor.x;
                let dy = current.y - anchor.y;
                let w = dx.abs().max(1);
                let h = ((w as f64) * rh / rw).round().max(1.0) as i32;
                let end_x = if dx >= 0 { anchor.x + w } else { anchor.x - w };
                let end_y = if dy >= 0 { anchor.y + h } else { anchor.y - h };
                normalize_rect(anchor, PhysicalPoint { x: end_x, y: end_y })
            }
        }
    }
}

fn normalize_rect(a: PhysicalPoint, b: PhysicalPoint) -> PhysicalRect {
    PhysicalRect::new(
        a.x.min(b.x),
        a.y.min(b.y),
        a.x.max(b.x),
        a.y.max(b.y),
    )
}

/// Concrete selection target to extract from a `CapturedFrame` or Clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionTarget {
    Region {
        rect: PhysicalRect,
        guideline: GuidelineResolution,
    },
    FixedRegion {
        origin: PhysicalPoint,
        width: u32,
        height: u32,
    },
    Window {
        rect: PhysicalRect,
        /// Optional Windows 11 rounded-corner radius (e.g. `Some(8)`) to mask corner pixels transparent.
        round_corner_radius: Option<u32>,
    },
    Fullscreen {
        /// `None` = entire virtual desktop; `Some(idx)` = specific monitor in `DisplayTopology`.
        monitor_index: Option<usize>,
    },
    Freehand {
        /// Closed polygon vertices in virtual-desktop physical coordinates.
        polygon: Vec<PhysicalPoint>,
    },
    MultipleArea {
        /// 1..N disjoint rectangles composited onto a minimal shared transparent bounding canvas.
        rects: Vec<PhysicalRect>,
    },
    Menu {
        rect: PhysicalRect,
    },
    Clipboard,
}

/// Extracted RGBA8 image ready for the SnagSoup Editor or direct export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionResult {
    pub bounds: PhysicalRect,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Stateful selection session used by the Selection Overlay.
#[derive(Debug, Clone, Default)]
pub struct SelectionSession {
    pub mode: SelectionMode,
    pub guideline: GuidelineResolution,
    pub multi_rects: Vec<PhysicalRect>,
    pub freehand_points: Vec<PhysicalPoint>,
}

impl SelectionSession {
    pub fn new(mode: SelectionMode) -> Self {
        Self {
            mode,
            guideline: GuidelineResolution::Free,
            multi_rects: Vec::new(),
            freehand_points: Vec::new(),
        }
    }

    pub fn add_multi_rect(&mut self, rect: PhysicalRect) {
        if rect.width() > 0 && rect.height() > 0 {
            self.multi_rects.push(rect);
        }
    }

    pub fn push_freehand_point(&mut self, pt: PhysicalPoint) {
        self.freehand_points.push(pt);
    }

    /// Extract pixels for `target` from `frame` (and optional `topology` for `Fullscreen(Some(idx))`).
    pub fn extract(
        frame: &CapturedFrame,
        topology: Option<&DisplayTopology>,
        target: &SelectionTarget,
    ) -> Result<SelectionResult, CaptureError> {
        match target {
            SelectionTarget::Region { rect, .. } | SelectionTarget::Menu { rect } => {
                crop_rect(frame, *rect)
            }
            SelectionTarget::FixedRegion {
                origin,
                width,
                height,
            } => {
                if *width == 0 || *height == 0 {
                    return Err(CaptureError::InvalidBounds);
                }
                let rect = PhysicalRect::new(
                    origin.x,
                    origin.y,
                    origin.x + *width as i32,
                    origin.y + *height as i32,
                );
                crop_rect(frame, rect)
            }
            SelectionTarget::Window {
                rect,
                round_corner_radius,
            } => {
                let mut res = crop_rect(frame, *rect)?;
                if let Some(radius) = *round_corner_radius {
                    apply_rounded_corner_mask(&mut res, radius);
                }
                Ok(res)
            }
            SelectionTarget::Fullscreen { monitor_index } => {
                let rect = match (monitor_index, topology) {
                    (Some(idx), Some(topo)) => topo
                        .monitors
                        .get(*idx)
                        .ok_or(CaptureError::InvalidBounds)?
                        .physical_rect,
                    _ => frame.bounds,
                };
                crop_rect(frame, rect)
            }
            SelectionTarget::Freehand { polygon } => extract_freehand(frame, polygon),
            SelectionTarget::MultipleArea { rects } => extract_multiple_area(frame, rects),
            SelectionTarget::Clipboard => extract_from_clipboard()?.ok_or(CaptureError::InvalidBounds),
        }
    }
}

/// Read an image directly from the Windows Clipboard (`Capture from Clipboard`).
pub fn extract_from_clipboard() -> Result<Option<SelectionResult>, CaptureError> {
    let Some(img) = read_image_from_clipboard()? else {
        return Ok(None);
    };
    Ok(Some(SelectionResult {
        bounds: PhysicalRect::new(0, 0, img.width as i32, img.height as i32),
        width: img.width,
        height: img.height,
        rgba: img.data,
    }))
}

fn clamp_rect_to_frame(frame: &CapturedFrame, rect: PhysicalRect) -> Option<PhysicalRect> {
    let left = rect.left.max(frame.bounds.left);
    let top = rect.top.max(frame.bounds.top);
    let right = rect.right.min(frame.bounds.right);
    let bottom = rect.bottom.min(frame.bounds.bottom);
    if right > left && bottom > top {
        Some(PhysicalRect::new(left, top, right, bottom))
    } else {
        None
    }
}

fn crop_rect(frame: &CapturedFrame, rect: PhysicalRect) -> Result<SelectionResult, CaptureError> {
    let clipped = clamp_rect_to_frame(frame, rect).ok_or(CaptureError::InvalidBounds)?;
    let w = clipped.width() as usize;
    let h = clipped.height() as usize;
    let src_w = frame.width as usize;
    let off_x = (clipped.left - frame.bounds.left) as usize;
    let off_y = (clipped.top - frame.bounds.top) as usize;

    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let src_start = ((off_y + y) * src_w + off_x) * 4;
        let dst_start = y * w * 4;
        rgba[dst_start..dst_start + w * 4]
            .copy_from_slice(&frame.rgba[src_start..src_start + w * 4]);
    }

    Ok(SelectionResult {
        bounds: clipped,
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

fn apply_rounded_corner_mask(res: &mut SelectionResult, radius: u32) {
    let r = radius.min(res.width / 2).min(res.height / 2) as f32;
    if r <= 0.0 {
        return;
    }
    let w = res.width as usize;
    let h = res.height as usize;
    let r2 = r * r;

    for y in 0..h {
        for x in 0..w {
            let cx = if (x as f32) < r {
                r - 0.5
            } else if (x as f32) >= (w as f32 - r) {
                w as f32 - r - 0.5
            } else {
                continue;
            };
            let cy = if (y as f32) < r {
                r - 0.5
            } else if (y as f32) >= (h as f32 - r) {
                h as f32 - r - 0.5
            } else {
                continue;
            };
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy > r2 {
                let idx = (y * w + x) * 4;
                res.rgba[idx..idx + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
}

/// Even-odd ray-casting point-in-polygon test.
pub fn point_in_polygon(px: f64, py: f64, polygon: &[PhysicalPoint]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let xi = polygon[i].x as f64;
        let yi = polygon[i].y as f64;
        let xj = polygon[j].x as f64;
        let yj = polygon[j].y as f64;

        let intersects = ((yi > py) != (yj > py))
            && (px < (xj - xi) * (py - yi) / ((yj - yi) + 1e-12) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn extract_freehand(
    frame: &CapturedFrame,
    polygon: &[PhysicalPoint],
) -> Result<SelectionResult, CaptureError> {
    if polygon.len() < 3 {
        return Err(CaptureError::InvalidBounds);
    }
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for p in polygon {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    let bbox = PhysicalRect::new(min_x, min_y, max_x, max_y);
    let mut res = crop_rect(frame, bbox)?;
    let w = res.width as usize;
    let h = res.height as usize;

    for y in 0..h {
        let wy = (res.bounds.top + y as i32) as f64 + 0.5;
        for x in 0..w {
            let wx = (res.bounds.left + x as i32) as f64 + 0.5;
            if !point_in_polygon(wx, wy, polygon) {
                let idx = (y * w + x) * 4;
                res.rgba[idx..idx + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    Ok(res)
}

fn extract_multiple_area(
    frame: &CapturedFrame,
    rects: &[PhysicalRect],
) -> Result<SelectionResult, CaptureError> {
    let clipped: Vec<PhysicalRect> = rects
        .iter()
        .filter_map(|&r| clamp_rect_to_frame(frame, r))
        .collect();
    if clipped.is_empty() {
        return Err(CaptureError::InvalidBounds);
    }

    let mut union_bounds = clipped[0];
    for r in &clipped[1..] {
        union_bounds = union_bounds.union(r);
    }

    let canvas_w = union_bounds.width() as usize;
    let canvas_h = union_bounds.height() as usize;
    let src_w = frame.width as usize;
    let mut rgba = vec![0u8; canvas_w * canvas_h * 4]; // transparent background

    for r in &clipped {
        let rw = r.width() as usize;
        let rh = r.height() as usize;
        let src_x = (r.left - frame.bounds.left) as usize;
        let src_y = (r.top - frame.bounds.top) as usize;
        let dst_x = (r.left - union_bounds.left) as usize;
        let dst_y = (r.top - union_bounds.top) as usize;

        for y in 0..rh {
            let s_off = ((src_y + y) * src_w + src_x) * 4;
            let d_off = ((dst_y + y) * canvas_w + dst_x) * 4;
            rgba[d_off..d_off + rw * 4].copy_from_slice(&frame.rgba[s_off..s_off + rw * 4]);
        }
    }

    Ok(SelectionResult {
        bounds: union_bounds,
        width: canvas_w as u32,
        height: canvas_h as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CaptureBackend;

    fn synthetic_frame(w: u32, h: u32) -> CapturedFrame {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let idx = ((y * w + x) * 4) as usize;
                rgba[idx] = (x % 256) as u8;
                rgba[idx + 1] = (y % 256) as u8;
                rgba[idx + 2] = 200;
                rgba[idx + 3] = 255;
            }
        }
        CapturedFrame {
            bounds: PhysicalRect::new(0, 0, w as i32, h as i32),
            width: w,
            height: h,
            rgba,
            backend: CaptureBackend::GdiFallback,
            elapsed_ms: 0.5,
        }
    }

    #[test]
    fn guidelines_constrain_aspect_ratios_and_fixed_resolutions() {
        let anchor = PhysicalPoint { x: 100, y: 100 };
        let r16_9 = GuidelineResolution::Aspect16x9
            .constrain_rect(anchor, PhysicalPoint { x: 260, y: 120 });
        assert_eq!(r16_9.width(), 160);
        assert_eq!(r16_9.height(), 90);

        let r1_1 = GuidelineResolution::Aspect1x1
            .constrain_rect(anchor, PhysicalPoint { x: 150, y: 300 });
        assert_eq!(r1_1.width(), 50);
        assert_eq!(r1_1.height(), 50);

        let r1080 = GuidelineResolution::Fixed1080p
            .constrain_rect(anchor, PhysicalPoint { x: 101, y: 101 });
        assert_eq!(r1080.width(), 1920);
        assert_eq!(r1080.height(), 1080);
    }

    #[test]
    fn freehand_lasso_masks_outside_pixels_transparent() {
        let frame = synthetic_frame(100, 100);
        // Triangle covering (20,20), (80,20), (50,80)
        let poly = vec![
            PhysicalPoint { x: 20, y: 20 },
            PhysicalPoint { x: 80, y: 20 },
            PhysicalPoint { x: 50, y: 80 },
        ];
        let res = SelectionSession::extract(
            &frame,
            None,
            &SelectionTarget::Freehand { polygon: poly },
        )
        .expect("freehand extract");

        assert_eq!(res.width, 60);
        assert_eq!(res.height, 60);
        // Center of triangle (local 30, 20) must be opaque
        let center_idx = (20 * 60 + 30) * 4;
        assert_eq!(res.rgba[center_idx + 3], 255);
        // Bottom-left corner of bounding box (local 2, 55) is outside triangle -> transparent
        let outside_idx = (55 * 60 + 2) * 4;
        assert_eq!(&res.rgba[outside_idx..outside_idx + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn multiple_area_composites_onto_shared_transparent_canvas() {
        let frame = synthetic_frame(200, 200);
        let r1 = PhysicalRect::new(10, 10, 30, 30); // 20x20
        let r2 = PhysicalRect::new(60, 40, 90, 70); // 30x30
        let res = SelectionSession::extract(
            &frame,
            None,
            &SelectionTarget::MultipleArea {
                rects: vec![r1, r2],
            },
        )
        .expect("multi-area extract");

        // Bounding union is [10..90] x [10..70] => 80x60
        assert_eq!(res.bounds, PhysicalRect::new(10, 10, 90, 70));
        assert_eq!(res.width, 80);
        assert_eq!(res.height, 60);

        // Inside r1 (local 5, 5) -> opaque
        let idx_r1 = (5 * 80 + 5) * 4;
        assert_eq!(res.rgba[idx_r1 + 3], 255);
        // Gap between r1 and r2 (local 35, 15) -> transparent
        let idx_gap = (15 * 80 + 35) * 4;
        assert_eq!(&res.rgba[idx_gap..idx_gap + 4], &[0, 0, 0, 0]);
        // Inside r2 (local 60, 40) -> opaque
        let idx_r2 = (40 * 80 + 60) * 4;
        assert_eq!(res.rgba[idx_r2 + 3], 255);
    }

    #[test]
    fn window_rounded_corners_masks_outer_corner_pixel() {
        let frame = synthetic_frame(100, 100);
        let res = SelectionSession::extract(
            &frame,
            None,
            &SelectionTarget::Window {
                rect: PhysicalRect::new(10, 10, 60, 60),
                round_corner_radius: Some(8),
            },
        )
        .expect("window extract");

        assert_eq!(res.width, 50);
        assert_eq!(res.height, 50);
        // Corner (0,0) should be transparent
        assert_eq!(&res.rgba[0..4], &[0, 0, 0, 0]);
        // Center (25,25) should be opaque
        let mid = (25 * 50 + 25) * 4;
        assert_eq!(res.rgba[mid + 3], 255);
    }
}
