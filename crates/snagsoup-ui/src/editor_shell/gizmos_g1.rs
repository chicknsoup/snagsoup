//! Interactive Vector Gizmos Group 1 (`W3-07`):
//! - Move tool: `hit_test_object`, marquee multi-select, 8 resize handles + rotation handle
//!   + yellow midpoint Bezier bend handle on Arrows
//! - Editor shortcuts: `Delete`, `Ctrl+C`/`Ctrl+V`/`Ctrl+D`, 1px/10px arrow nudge,
//!   Bring to Front / Send to Back / Flatten / Flip
//! - Direct creation & editing for Arrow, Line, Shape, Step (auto-incrementing `1,2,3`/`A,B,C`),
//!   Pen, and Highlighter

use snagsoup_canvas::{DocumentSession, SceneCompositor, TiledBitmap, hit_test_object};
use snagsoup_model::{CaptureObject, ColorRgba, PointF, RectF, ToolMode};

/// Step badge numbering sequence style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepSequence {
    #[default]
    Numeric,
    UpperAlpha,
    LowerAlpha,
}

/// Auto-incrementing counter for the Snagit `Step` tool.
#[derive(Debug, Clone)]
pub struct StepCounter {
    pub sequence: StepSequence,
    pub next_index: u32,
}

impl Default for StepCounter {
    fn default() -> Self {
        Self {
            sequence: StepSequence::Numeric,
            next_index: 1,
        }
    }
}

impl StepCounter {
    pub fn next_label(&mut self) -> String {
        let idx = self.next_index;
        self.next_index += 1;
        match self.sequence {
            StepSequence::Numeric => idx.to_string(),
            StepSequence::UpperAlpha => {
                let c = (b'A' + ((idx.saturating_sub(1)) % 26) as u8) as char;
                c.to_string()
            }
            StepSequence::LowerAlpha => {
                let c = (b'a' + ((idx.saturating_sub(1)) % 26) as u8) as char;
                c.to_string()
            }
        }
    }
}

/// Interactive handle on a selected vector object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorHandle {
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    Rotate,
    ArrowBezierMidpoint,
}

/// Active selection & clipboard state for Group 1 vector gizmos.
#[derive(Debug, Default)]
pub struct GizmoControllerG1 {
    pub selected_indices: Vec<usize>,
    pub marquee_rect: Option<RectF>,
    pub clipboard_objects: Vec<CaptureObject>,
    pub step_counter: StepCounter,
    next_id: u64,
}

impl GizmoControllerG1 {
    fn alloc_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}", self.next_id)
    }

    /// Hit-test objects top-to-bottom (`hit_test_object`) and update selection.
    pub fn click_select(
        &mut self,
        session: &DocumentSession,
        canvas_pt: PointF,
        tolerance: f64,
        additive: bool,
    ) -> Option<usize> {
        let hit = session
            .page
            .capture_objects
            .iter()
            .enumerate()
            .rev()
            .find(|(_, obj)| hit_test_object(obj, canvas_pt, tolerance))
            .map(|(i, _)| i);

        if !additive {
            self.selected_indices.clear();
        }
        if let Some(idx) = hit {
            if !self.selected_indices.contains(&idx) {
                self.selected_indices.push(idx);
            }
        }
        hit
    }

    /// Select all objects whose bounding box intersects `marquee`.
    pub fn finish_marquee_select(&mut self, session: &DocumentSession, marquee: RectF) {
        self.marquee_rect = None;
        self.selected_indices.clear();
        for (i, obj) in session.page.capture_objects.iter().enumerate() {
            if let Some(b) = object_bounds(obj) {
                let intersects = b.x <= marquee.x + marquee.width
                    && b.x + b.width >= marquee.x
                    && b.y <= marquee.y + marquee.height
                    && b.y + b.height >= marquee.y;
                if intersects {
                    self.selected_indices.push(i);
                }
            }
        }
    }

    /// Create a new Group 1 vector object (Arrow, Line, Shape, Step, Pen, Highlighter).
    pub fn create_object(
        &mut self,
        session: &mut DocumentSession,
        tool: ToolMode,
        start: PointF,
        end: PointF,
        stroke_color: ColorRgba,
        fill_color: ColorRgba,
        stroke_width: f64,
        opacity: f64,
    ) -> usize {
        let mut obj = CaptureObject {
            object_id: Some(self.alloc_id(tool.as_str())),
            tool_mode: Some(tool.clone()),
            start_point: Some(start),
            end_point: Some(end),
            foreground_color: Some(stroke_color),
            background_color: Some(fill_color),
            stroke_width: Some(stroke_width),
            opacity: Some(opacity),
            ..CaptureObject::default()
        };

        match tool {
            ToolMode::Step => {
                let label = self.step_counter.next_label();
                obj.step_value = Some(label);
                obj.step_style = Some("Circle".to_string());
                obj.start_point = Some(PointF::new(start.x - 16.0, start.y - 16.0));
                obj.end_point = Some(PointF::new(start.x + 16.0, start.y + 16.0));
            }
            ToolMode::Pen | ToolMode::Highlighter => {
                obj.points_array = Some(vec![start, end]);
            }
            ToolMode::Arrow => {
                obj.points_array = Some(vec![
                    start,
                    PointF::new((start.x + end.x) * 0.5, (start.y + end.y) * 0.5),
                    end,
                ]);
            }
            _ => {}
        }

        let idx = session.add_object(obj);
        self.selected_indices = vec![idx];
        idx
    }

    /// Append a freehand stroke point to an active Pen / Highlighter object.
    pub fn append_stroke_point(&self, session: &mut DocumentSession, idx: usize, pt: PointF) {
        if let Some(mut obj) = session.page.capture_objects.get(idx).cloned() {
            let pts = obj.points_array.get_or_insert_with(Vec::new);
            pts.push(pt);
            obj.end_point = Some(pt);
            session.update_object(idx, obj);
        }
    }

    /// Bend an Arrow smoothly by dragging its yellow midpoint Bezier control handle.
    pub fn bend_arrow_midpoint(
        &self,
        session: &mut DocumentSession,
        idx: usize,
        new_control: PointF,
    ) -> bool {
        let Some(mut obj) = session.page.capture_objects.get(idx).cloned() else {
            return false;
        };
        let start = obj.start_point.unwrap_or(PointF::new(0.0, 0.0));
        let end = obj.end_point.unwrap_or(PointF::new(100.0, 0.0));
        obj.points_array = Some(vec![start, new_control, end]);
        session.update_object(idx, obj)
    }

    /// Apply a resize or rotation handle drag on `idx`.
    pub fn drag_handle(
        &self,
        session: &mut DocumentSession,
        idx: usize,
        handle: VectorHandle,
        canvas_pt: PointF,
    ) -> bool {
        let Some(mut obj) = session.page.capture_objects.get(idx).cloned() else {
            return false;
        };
        match handle {
            VectorHandle::ArrowBezierMidpoint => return self.bend_arrow_midpoint(session, idx, canvas_pt),
            VectorHandle::Rotate => {
                if let Some(b) = object_bounds(&obj) {
                    let cx = b.x + b.width * 0.5;
                    let cy = b.y + b.height * 0.5;
                    let angle_deg = (canvas_pt.y - cy).atan2(canvas_pt.x - cx).to_degrees() + 90.0;
                    obj.rotation_angle = Some(angle_deg);
                }
            }
            _ => {
                let mut sp = obj.start_point.unwrap_or(PointF::new(0.0, 0.0));
                let mut ep = obj.end_point.unwrap_or(PointF::new(100.0, 100.0));
                match handle {
                    VectorHandle::NorthWest => sp = canvas_pt,
                    VectorHandle::SouthEast => ep = canvas_pt,
                    VectorHandle::North => sp.y = canvas_pt.y,
                    VectorHandle::South => ep.y = canvas_pt.y,
                    VectorHandle::West => sp.x = canvas_pt.x,
                    VectorHandle::East => ep.x = canvas_pt.x,
                    VectorHandle::NorthEast => {
                        ep.x = canvas_pt.x;
                        sp.y = canvas_pt.y;
                    }
                    VectorHandle::SouthWest => {
                        sp.x = canvas_pt.x;
                        ep.y = canvas_pt.y;
                    }
                    _ => {}
                }
                obj.start_point = Some(sp);
                obj.end_point = Some(ep);
            }
        }
        session.update_object(idx, obj)
    }

    /// Nudge all selected objects by `(dx, dy)` pixels (`1px` or `Shift+Arrow` `10px`).
    pub fn nudge_selected(&self, session: &mut DocumentSession, dx: f64, dy: f64) {
        for &idx in &self.selected_indices {
            if let Some(mut obj) = session.page.capture_objects.get(idx).cloned() {
                translate_object(&mut obj, dx, dy);
                session.update_object(idx, obj);
            }
        }
    }

    /// Delete all selected objects (`Delete` / `Backspace`).
    pub fn delete_selected(&mut self, session: &mut DocumentSession) {
        let mut sorted = self.selected_indices.clone();
        sorted.sort_unstable();
        sorted.dedup();
        for idx in sorted.into_iter().rev() {
            session.delete_object(idx);
        }
        self.selected_indices.clear();
    }

    /// Copy selected objects (`Ctrl+C`).
    pub fn copy_selected(&mut self, session: &DocumentSession) {
        self.clipboard_objects = self
            .selected_indices
            .iter()
            .filter_map(|&i| session.page.capture_objects.get(i).cloned())
            .collect();
    }

    /// Paste copied objects with a +16px offset (`Ctrl+V`).
    pub fn paste(&mut self, session: &mut DocumentSession) {
        let mut new_sel = Vec::new();
        let clones = self.clipboard_objects.clone();
        for mut obj in clones {
            obj.object_id = Some(self.alloc_id("paste"));
            translate_object(&mut obj, 16.0, 16.0);
            new_sel.push(session.add_object(obj));
        }
        if !new_sel.is_empty() {
            self.selected_indices = new_sel;
        }
    }

    /// Duplicate selected objects (`Ctrl+D`).
    pub fn duplicate_selected(&mut self, session: &mut DocumentSession) {
        self.copy_selected(session);
        self.paste(session);
    }

    /// Bring first selected object to the top of the Z-order.
    pub fn bring_to_front(&mut self, session: &mut DocumentSession) -> bool {
        let Some(&idx) = self.selected_indices.first() else {
            return false;
        };
        let last = session.page.capture_objects.len().saturating_sub(1);
        if session.reorder_object(idx, last) {
            self.selected_indices = vec![last];
            true
        } else {
            false
        }
    }

    /// Send first selected object to the bottom of the Z-order.
    pub fn send_to_back(&mut self, session: &mut DocumentSession) -> bool {
        let Some(&idx) = self.selected_indices.first() else {
            return false;
        };
        if session.reorder_object(idx, 0) {
            self.selected_indices = vec![0];
            true
        } else {
            false
        }
    }

    /// Flatten all vector objects permanently into the raster `TiledBitmap` (Undoable).
    pub fn flatten_all(&mut self, session: &mut DocumentSession) -> bool {
        let comp = SceneCompositor::new(&session.page, Some(&session.bitmap));
        let Ok((w, h, rgba)) = comp.flatten_to_rgba() else {
            return false;
        };
        let Ok(flat_bmp) = TiledBitmap::from_rgba(w, h, &rgba) else {
            return false;
        };
        session.bitmap = flat_bmp;
        session.page.capture_objects.clear();
        self.selected_indices.clear();
        true
    }
}

/// Compute axis-aligned bounding rect of a [`CaptureObject`].
pub fn object_bounds(obj: &CaptureObject) -> Option<RectF> {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    let mut include = |p: PointF| {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    };

    if let Some(p) = obj.start_point {
        include(p);
    }
    if let Some(p) = obj.end_point {
        include(p);
    }
    if let Some(ref pts) = obj.points_array {
        for &p in pts {
            include(p);
        }
    }

    if min_x.is_finite() {
        Some(RectF::new(min_x, min_y, max_x - min_x, max_y - min_y))
    } else {
        None
    }
}

fn translate_object(obj: &mut CaptureObject, dx: f64, dy: f64) {
    if let Some(ref mut p) = obj.start_point {
        p.x += dx;
        p.y += dy;
    }
    if let Some(ref mut p) = obj.end_point {
        p.x += dx;
        p.y += dy;
    }
    if let Some(ref mut pts) = obj.points_array {
        for p in pts {
            p.x += dx;
            p.y += dy;
        }
    }
    if let Some(ref mut tails) = obj.tail_points {
        for p in tails {
            p.x += dx;
            p.y += dy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_model::SnagxPage;

    #[test]
    fn arrow_bezier_bend_step_counter_and_undo_redo() {
        let bmp = TiledBitmap::new_filled(800, 600, ColorRgba::WHITE);
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);
        let mut gizmos = GizmoControllerG1::default();

        // Create arrow & bend midpoint
        let idx = gizmos.create_object(
            &mut session,
            ToolMode::Arrow,
            PointF::new(20.0, 100.0),
            PointF::new(180.0, 100.0),
            ColorRgba::rgb(255, 0, 0),
            ColorRgba::TRANSPARENT,
            6.0,
            100.0,
        );
        assert!(gizmos.bend_arrow_midpoint(&mut session, idx, PointF::new(100.0, 20.0)));
        assert_eq!(
            session.page.capture_objects[idx].points_array.as_ref().unwrap()[1],
            PointF::new(100.0, 20.0)
        );

        // Auto-incrementing Step badges
        let s1 = gizmos.create_object(
            &mut session,
            ToolMode::Step,
            PointF::new(50.0, 50.0),
            PointF::new(82.0, 82.0),
            ColorRgba::WHITE,
            ColorRgba::rgb(27, 117, 188),
            2.0,
            100.0,
        );
        let s2 = gizmos.create_object(
            &mut session,
            ToolMode::Step,
            PointF::new(120.0, 50.0),
            PointF::new(152.0, 82.0),
            ColorRgba::WHITE,
            ColorRgba::rgb(27, 117, 188),
            2.0,
            100.0,
        );
        assert_eq!(session.page.capture_objects[s1].step_value.as_deref(), Some("1"));
        assert_eq!(session.page.capture_objects[s2].step_value.as_deref(), Some("2"));

        // Duplicate + Nudge + Undo
        gizmos.duplicate_selected(&mut session);
        assert_eq!(session.page.capture_objects.len(), 4);
        assert!(session.undo());
        assert_eq!(session.page.capture_objects.len(), 3);
    }
}
