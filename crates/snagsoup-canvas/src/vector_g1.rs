use crate::text_render::{render_text_box, HorizontalAlign, TextRenderOptions, VerticalAlign};
use snagsoup_model::{CaptureObject, ColorRgba, PointF, RectF, ToolMode};
use std::f32::consts::PI;
use tiny_skia::{
    BlendMode, Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke,
    StrokeDash, Transform,
};

pub fn to_skia_color(c: ColorRgba, opacity_mul: f32) -> Color {
    let a = ((c.a as f32 / 255.0) * opacity_mul).clamp(0.0, 1.0);
    Color::from_rgba(
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        a,
    )
    .unwrap_or(Color::TRANSPARENT)
}

pub fn make_stroke_dash(dash_type: Option<&str>, stroke_width: f32) -> Option<StrokeDash> {
    let w = stroke_width.max(1.0);
    match dash_type.unwrap_or("Solid") {
        "Dash" | "Dashed" => StrokeDash::new(vec![w * 3.0, w * 2.0], 0.0),
        "Dot" | "Dotted" => StrokeDash::new(vec![w, w * 1.5], 0.0),
        "DashDot" => StrokeDash::new(vec![w * 3.0, w * 1.5, w, w * 1.5], 0.0),
        "DashDotDot" => StrokeDash::new(vec![w * 3.0, w * 1.5, w, w * 1.5, w, w * 1.5], 0.0),
        _ => None,
    }
}

pub fn object_bounds_rectf(obj: &CaptureObject) -> Option<RectF> {
    if let (Some(p1), Some(p2)) = (obj.start_point, obj.end_point) {
        return Some(RectF::from_points(p1, p2));
    }
    if let Some(ref pts) = obj.points_array {
        if !pts.is_empty() {
            let mut min_x = f64::INFINITY;
            let mut min_y = f64::INFINITY;
            let mut max_x = f64::NEG_INFINITY;
            let mut max_y = f64::NEG_INFINITY;
            for p in pts {
                min_x = min_x.min(p.x);
                min_y = min_y.min(p.y);
                max_x = max_x.max(p.x);
                max_y = max_y.max(p.y);
            }
            return Some(RectF::new(
                min_x,
                min_y,
                (max_x - min_x).max(1.0),
                (max_y - min_y).max(1.0),
            ));
        }
    }
    None
}

/// Builds a rotated transform around `(cx, cy)` when `obj.rotation_angle` is non-zero.
pub fn object_transform(obj: &CaptureObject, base: Transform) -> Transform {
    let angle = obj.rotation_angle.unwrap_or(0.0) as f32;
    if angle.abs() < 1e-4 {
        return base;
    }
    if let Some(r) = object_bounds_rectf(obj) {
        let cx = (r.x + r.width * 0.5) as f32;
        let cy = (r.y + r.height * 0.5) as f32;
        return Transform::from_rotate_at(angle, cx, cy).post_concat(base);
    }
    base
}

/// Draws a soft drop shadow for a filled path if `DropShadowEnabled` is true.
pub fn draw_fill_shadow(pixmap: &mut Pixmap, obj: &CaptureObject, path: &Path, transform: Transform) {
    if !obj.drop_shadow_enabled.unwrap_or(false) {
        return;
    }
    let dx = obj.shadow_direction_x.unwrap_or(0.0) as f32;
    // Snagit uses Y-up for ShadowDirectionY (e.g. -4 means 4px downward)
    let dy = -(obj.shadow_direction_y.unwrap_or(-4.0) as f32);
    let shadow_opacity = (obj.shadow_opacity.unwrap_or(50.0) as f32 / 100.0).clamp(0.0, 1.0);
    let obj_opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let shadow_color = obj.shadow_color.unwrap_or(ColorRgba::BLACK);

    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color(to_skia_color(shadow_color, shadow_opacity * obj_opacity));
    let shadow_t = Transform::from_translate(dx, dy).post_concat(transform);
    pixmap.fill_path(path, &paint, FillRule::Winding, shadow_t, None);
}

/// Draws a soft drop shadow for a stroked path if `DropShadowEnabled` is true.
pub fn draw_stroke_shadow(
    pixmap: &mut Pixmap,
    obj: &CaptureObject,
    path: &Path,
    stroke: &Stroke,
    transform: Transform,
) {
    if !obj.drop_shadow_enabled.unwrap_or(false) {
        return;
    }
    let dx = obj.shadow_direction_x.unwrap_or(0.0) as f32;
    let dy = -(obj.shadow_direction_y.unwrap_or(-4.0) as f32);
    let shadow_opacity = (obj.shadow_opacity.unwrap_or(50.0) as f32 / 100.0).clamp(0.0, 1.0);
    let obj_opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let shadow_color = obj.shadow_color.unwrap_or(ColorRgba::BLACK);

    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color(to_skia_color(shadow_color, shadow_opacity * obj_opacity));
    let shadow_t = Transform::from_translate(dx, dy).post_concat(transform);
    pixmap.stroke_path(path, &paint, stroke, shadow_t, None);
}

/// Builds a rounded rectangle path.
pub fn build_rounded_rect_path(rect: Rect, radius: f32) -> Option<Path> {
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5).max(0.0);
    if r <= 0.1 {
        return Some(PathBuilder::from_rect(rect));
    }
    let x = rect.x();
    let y = rect.y();
    let w = rect.width();
    let h = rect.height();
    let k = r * 0.5522848; // Bezier quarter-circle constant

    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Catmull-Rom spline smoothing for Pen / Highlighter freehand point lists.
pub fn catmull_rom_to_path(pts: &[PointF]) -> Option<Path> {
    if pts.len() < 2 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].x as f32, pts[0].y as f32);
    if pts.len() == 2 {
        pb.line_to(pts[1].x as f32, pts[1].y as f32);
        return pb.finish();
    }
    for i in 0..pts.len() - 1 {
        let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = if i + 2 < pts.len() { pts[i + 2] } else { pts[i + 1] };

        let c1x = p1.x + (p2.x - p0.x) / 6.0;
        let c1y = p1.y + (p2.y - p0.y) / 6.0;
        let c2x = p2.x - (p3.x - p1.x) / 6.0;
        let c2y = p2.y - (p3.y - p1.y) / 6.0;
        pb.cubic_to(
            c1x as f32,
            c1y as f32,
            c2x as f32,
            c2y as f32,
            p2.x as f32,
            p2.y as f32,
        );
    }
    pb.finish()
}

/// Renders an Arrow object (straight or quadratic Bezier curve, single/double-headed, 6 arrowhead styles).
pub fn render_arrow(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let fg = obj.foreground_color.unwrap_or(ColorRgba::rgb(252, 66, 66));
    let stroke_w = obj.stroke_width.unwrap_or(6.0).max(1.0) as f32;

    let (start, ctrl, end) = extract_arrow_points(obj);
    let Some(start) = start else { return };
    let Some(end) = end else { return };

    let mut pb = PathBuilder::new();
    pb.move_to(start.x as f32, start.y as f32);
    if let Some(c) = ctrl {
        pb.quad_to(c.x as f32, c.y as f32, end.x as f32, end.y as f32);
    } else {
        pb.line_to(end.x as f32, end.y as f32);
    }
    let Some(shaft_path) = pb.finish() else { return };

    let stroke = Stroke {
        width: stroke_w,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        dash: make_stroke_dash(obj.dash_type.as_deref(), stroke_w),
        ..Stroke::default()
    };

    let t = object_transform(obj, transform);
    draw_stroke_shadow(pixmap, obj, &shaft_path, &stroke, t);

    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color(to_skia_color(fg, opacity));
    pixmap.stroke_path(&shaft_path, &paint, &stroke, t, None);

    // Arrowheads
    let end_style = obj
        .extra
        .get("ArrowEnd")
        .and_then(|v| v.as_str())
        .unwrap_or("EquilateralArrow");
    let start_style = obj
        .extra
        .get("ArrowStart")
        .and_then(|v| v.as_str())
        .unwrap_or("Flat");
    let head_scale = obj
        .extra
        .get("ArrowEndWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(3.0) as f32;

    let end_dir_from = ctrl.unwrap_or(start);
    draw_arrowhead(pixmap, obj, end_dir_from, end, end_style, stroke_w * head_scale, fg, opacity, t);

    if start_style != "Flat" && start_style != "Round" && start_style != "None" {
        let start_dir_from = ctrl.unwrap_or(end);
        draw_arrowhead(
            pixmap,
            obj,
            start_dir_from,
            start,
            start_style,
            stroke_w * head_scale,
            fg,
            opacity,
            t,
        );
    }
}

fn extract_arrow_points(obj: &CaptureObject) -> (Option<PointF>, Option<PointF>, Option<PointF>) {
    if let Some(ref pts) = obj.points_array {
        if pts.len() >= 3 {
            return (Some(pts[0]), Some(pts[1]), Some(pts[pts.len() - 1]));
        } else if pts.len() == 2 {
            return (Some(pts[0]), None, Some(pts[1]));
        }
    }
    (obj.start_point, None, obj.end_point)
}

#[allow(clippy::too_many_arguments)]
fn draw_arrowhead(
    pixmap: &mut Pixmap,
    obj: &CaptureObject,
    from: PointF,
    tip: PointF,
    style: &str,
    size: f32,
    color: ColorRgba,
    opacity: f32,
    transform: Transform,
) {
    if style == "Flat" || style == "None" || size < 1.0 {
        return;
    }
    let dx = (tip.x - from.x) as f32;
    let dy = (tip.y - from.y) as f32;
    let len = dx.hypot(dy);
    if len < 1e-3 {
        return;
    }
    let ux = dx / len;
    let uy = dy / len;
    let px = -uy;
    let py = ux;
    let tx = tip.x as f32;
    let ty = tip.y as f32;

    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color(to_skia_color(color, opacity));

    match style {
        "Circle" => {
            if let Some(p) = PathBuilder::from_circle(tx, ty, size * 0.45) {
                draw_fill_shadow(pixmap, obj, &p, transform);
                pixmap.fill_path(&p, &paint, FillRule::Winding, transform, None);
            }
        }
        "Diamond" => {
            let half = size * 0.5;
            let mut pb = PathBuilder::new();
            pb.move_to(tx + ux * half, ty + uy * half);
            pb.line_to(tx + px * half * 0.7, ty + py * half * 0.7);
            pb.line_to(tx - ux * half, ty - uy * half);
            pb.line_to(tx - px * half * 0.7, ty - py * half * 0.7);
            pb.close();
            if let Some(p) = pb.finish() {
                draw_fill_shadow(pixmap, obj, &p, transform);
                pixmap.fill_path(&p, &paint, FillRule::Winding, transform, None);
            }
        }
        "OpenArrow" => {
            let back_x = tx - ux * size;
            let back_y = ty - uy * size;
            let mut pb = PathBuilder::new();
            pb.move_to(back_x + px * size * 0.55, back_y + py * size * 0.55);
            pb.line_to(tx, ty);
            pb.line_to(back_x - px * size * 0.55, back_y - py * size * 0.55);
            if let Some(p) = pb.finish() {
                let stroke = Stroke {
                    width: (size * 0.22).max(2.0),
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                };
                draw_stroke_shadow(pixmap, obj, &p, &stroke, transform);
                pixmap.stroke_path(&p, &paint, &stroke, transform, None);
            }
        }
        "FilledBarbed" => {
            let back_x = tx - ux * size * 1.15;
            let back_y = ty - uy * size * 1.15;
            let notch_x = tx - ux * size * 0.75;
            let notch_y = ty - uy * size * 0.75;
            let mut pb = PathBuilder::new();
            pb.move_to(tx, ty);
            pb.line_to(back_x + px * size * 0.6, back_y + py * size * 0.6);
            pb.line_to(notch_x, notch_y);
            pb.line_to(back_x - px * size * 0.6, back_y - py * size * 0.6);
            pb.close();
            if let Some(p) = pb.finish() {
                draw_fill_shadow(pixmap, obj, &p, transform);
                pixmap.fill_path(&p, &paint, FillRule::Winding, transform, None);
            }
        }
        _ => {
            // EquilateralArrow (default)
            let back_x = tx - ux * size;
            let back_y = ty - uy * size;
            let mut pb = PathBuilder::new();
            pb.move_to(tx, ty);
            pb.line_to(back_x + px * size * 0.58, back_y + py * size * 0.58);
            pb.line_to(back_x - px * size * 0.58, back_y - py * size * 0.58);
            pb.close();
            if let Some(p) = pb.finish() {
                draw_fill_shadow(pixmap, obj, &p, transform);
                pixmap.fill_path(&p, &paint, FillRule::Winding, transform, None);
            }
        }
    }
}

/// Renders Line, Pen, and Highlighter objects.
pub fn render_stroke_tool(pixmap: &mut Pixmap, obj: &CaptureObject, mode: &ToolMode, transform: Transform) {
    let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let fg = obj.foreground_color.unwrap_or(ColorRgba::rgb(252, 66, 66));
    let stroke_w = obj.stroke_width.unwrap_or(4.0).max(1.0) as f32;

    let mut pts: Vec<PointF> = Vec::new();
    if let Some(ref arr) = obj.points_array {
        pts.extend_from_slice(arr);
    } else if let (Some(s), Some(e)) = (obj.start_point, obj.end_point) {
        pts.push(s);
        pts.push(e);
    }
    if pts.len() < 2 {
        return;
    }

    let path = if matches!(mode, ToolMode::Pen | ToolMode::Highlighter) && pts.len() > 2 {
        catmull_rom_to_path(&pts)
    } else {
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].x as f32, pts[0].y as f32);
        for p in &pts[1..] {
            pb.line_to(p.x as f32, p.y as f32);
        }
        pb.finish()
    };
    let Some(path) = path else { return };

    let cap = match obj.extra.get("LineStartCap").and_then(|v| v.as_str()) {
        Some("Flat") => LineCap::Butt,
        Some("Square") => LineCap::Square,
        _ => LineCap::Round,
    };

    let stroke = Stroke {
        width: stroke_w,
        line_cap: cap,
        line_join: LineJoin::Round,
        dash: make_stroke_dash(obj.dash_type.as_deref(), stroke_w),
        ..Stroke::default()
    };

    let t = object_transform(obj, transform);
    if *mode != ToolMode::Highlighter {
        draw_stroke_shadow(pixmap, obj, &path, &stroke, t);
    }

    let mut paint = Paint::default();
    paint.anti_alias = true;
    if *mode == ToolMode::Highlighter {
        paint.blend_mode = BlendMode::Multiply;
        paint.set_color(to_skia_color(fg, opacity));
    } else {
        paint.set_color(to_skia_color(fg, opacity));
    }
    pixmap.stroke_path(&path, &paint, &stroke, t, None);
}

/// Renders Shape objects (Rectangle, RoundedRectangle, Ellipse, Triangle, Polygon, Star).
pub fn render_shape(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let Some(r) = object_bounds_rectf(obj) else { return };
    let Some(rect) = Rect::from_xywh(r.x as f32, r.y as f32, (r.width as f32).max(1.0), (r.height as f32).max(1.0)) else {
        return;
    };

    let shape_kind = obj.tool_shape.as_deref().unwrap_or("Rectangle");
    let path = build_shape_path(rect, shape_kind, obj.corner_radius_ratio.unwrap_or(0.0) as f32);
    let Some(path) = path else { return };

    let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let fg = obj.foreground_color.unwrap_or(ColorRgba::rgb(252, 66, 66));
    let bg = obj.background_color.unwrap_or(ColorRgba::TRANSPARENT);
    let stroke_w = obj.stroke_width.unwrap_or(2.0).max(0.0) as f32;
    let t = object_transform(obj, transform);

    if bg.a > 0 {
        draw_fill_shadow(pixmap, obj, &path, t);
        let mut fill = Paint::default();
        fill.anti_alias = true;
        fill.set_color(to_skia_color(bg, opacity));
        pixmap.fill_path(&path, &fill, FillRule::Winding, t, None);
    }

    if stroke_w > 0.0 && fg.a > 0 {
        let stroke = Stroke {
            width: stroke_w,
            line_join: LineJoin::Round,
            dash: make_stroke_dash(obj.dash_type.as_deref(), stroke_w),
            ..Stroke::default()
        };
        if bg.a == 0 {
            draw_stroke_shadow(pixmap, obj, &path, &stroke, t);
        }
        let mut stroke_paint = Paint::default();
        stroke_paint.anti_alias = true;
        stroke_paint.set_color(to_skia_color(fg, opacity));
        pixmap.stroke_path(&path, &stroke_paint, &stroke, t, None);
    }
}

pub fn build_shape_path(rect: Rect, kind: &str, corner_ratio: f32) -> Option<Path> {
    let cx = rect.x() + rect.width() * 0.5;
    let cy = rect.y() + rect.height() * 0.5;
    let rx = rect.width() * 0.5;
    let ry = rect.height() * 0.5;

    match kind {
        "Ellipse" | "Oval" | "Circle" => PathBuilder::from_oval(rect),
        "RoundedRectangle" => {
            let rad = rect.width().min(rect.height()) * corner_ratio.clamp(0.05, 0.5);
            build_rounded_rect_path(rect, rad)
        }
        "Triangle" => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, rect.y());
            pb.line_to(rect.right(), rect.bottom());
            pb.line_to(rect.x(), rect.bottom());
            pb.close();
            pb.finish()
        }
        "Diamond" => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, rect.y());
            pb.line_to(rect.right(), cy);
            pb.line_to(cx, rect.bottom());
            pb.line_to(rect.x(), cy);
            pb.close();
            pb.finish()
        }
        "Polygon" => {
            let sides = 6;
            let mut pb = PathBuilder::new();
            for i in 0..sides {
                let a = (i as f32 / sides as f32) * 2.0 * PI - PI * 0.5;
                let x = cx + rx * a.cos();
                let y = cy + ry * a.sin();
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            pb.close();
            pb.finish()
        }
        "Star" => {
            let points = 5;
            let mut pb = PathBuilder::new();
            for i in 0..(points * 2) {
                let a = (i as f32 / (points * 2) as f32) * 2.0 * PI - PI * 0.5;
                let mul = if i % 2 == 0 { 1.0 } else { 0.42 };
                let x = cx + rx * mul * a.cos();
                let y = cy + ry * mul * a.sin();
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            pb.close();
            pb.finish()
        }
        _ => {
            if corner_ratio > 0.01 {
                let rad = rect.width().min(rect.height()) * corner_ratio.clamp(0.0, 0.5);
                build_rounded_rect_path(rect, rad)
            } else {
                Some(PathBuilder::from_rect(rect))
            }
        }
    }
}

/// Renders Step badges (`Circle`, `Square`, `Circle-Pointer`, `Square-Pointer`) with centered label.
pub fn render_step(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let Some(r) = object_bounds_rectf(obj) else { return };
    let Some(rect) = Rect::from_xywh(r.x as f32, r.y as f32, (r.width as f32).max(12.0), (r.height as f32).max(12.0)) else {
        return;
    };

    let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let bg = obj
        .background_color
        .filter(|c| c.a > 0)
        .or(obj.foreground_color)
        .unwrap_or(ColorRgba::rgb(252, 66, 66));
    let text_col = obj
        .extra
        .get("TextSelectionColor")
        .and_then(|v| v.as_str())
        .and_then(|s| ColorRgba::from_hex(s).ok())
        .unwrap_or(ColorRgba::WHITE);
    let style = obj.step_style.as_deref().unwrap_or("Circle");
    let t = object_transform(obj, transform);

    let path = match style {
        "Square" => build_rounded_rect_path(rect, rect.width().min(rect.height()) * 0.15),
        "Square-Pointer" => {
            let mut pb = PathBuilder::new();
            let cx = rect.x() + rect.width() * 0.5;
            pb.move_to(rect.x(), rect.y());
            pb.line_to(rect.right(), rect.y());
            pb.line_to(rect.right(), rect.bottom() - rect.height() * 0.2);
            pb.line_to(cx + rect.width() * 0.15, rect.bottom() - rect.height() * 0.2);
            pb.line_to(cx, rect.bottom());
            pb.line_to(cx - rect.width() * 0.15, rect.bottom() - rect.height() * 0.2);
            pb.line_to(rect.x(), rect.bottom() - rect.height() * 0.2);
            pb.close();
            pb.finish()
        }
        "Circle-Pointer" => {
            let cx = rect.x() + rect.width() * 0.5;
            let cy = rect.y() + rect.height() * 0.42;
            let rad = rect.width().min(rect.height()) * 0.42;
            let mut pb = PathBuilder::new();
            pb.move_to(cx, rect.bottom());
            pb.line_to(cx - rad * 0.65, cy + rad * 0.55);
            pb.cubic_to(cx - rad, cy - rad * 0.4, cx - rad * 0.5, cy - rad, cx, cy - rad);
            pb.cubic_to(cx + rad * 0.5, cy - rad, cx + rad, cy - rad * 0.4, cx + rad * 0.65, cy + rad * 0.55);
            pb.close();
            pb.finish()
        }
        _ => PathBuilder::from_oval(rect),
    };

    if let Some(path) = path {
        draw_fill_shadow(pixmap, obj, &path, t);
        let mut fill = Paint::default();
        fill.anti_alias = true;
        fill.set_color(to_skia_color(bg, opacity));
        pixmap.fill_path(&path, &fill, FillRule::Winding, t, None);
    }

    let label = obj.step_value.as_deref().unwrap_or("1");
    let font_size = (rect.width().min(rect.height()) * 0.52).max(10.0);
    let opts = TextRenderOptions {
        font_family: obj.font_family.clone().unwrap_or_else(|| "Arial".to_string()),
        font_size,
        color: text_col,
        bold: true,
        h_align: HorizontalAlign::Center,
        v_align: VerticalAlign::Center,
        opacity,
        ..TextRenderOptions::default()
    };
    render_text_box(
        pixmap,
        label,
        rect.x(),
        rect.y(),
        rect.width(),
        rect.height(),
        &opts,
        t,
    );
}

/// Renders categorized vector Stamp objects (`sym-check`, `sym-cross`, `sym-warn`, `sym-info`,
/// `sym-question`, `sym-star`, `sym-heart`, `sym-ban`, `sym-pin`, `sym-flag`, `sym-bug`,
/// `sym-lock`, `ui-cursor`, `ui-hand`, `badge-thumbsup`, `badge-bolt`).
pub fn render_stamp(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let Some(r) = object_bounds_rectf(obj) else {
        return;
    };
    let w = (r.width as f32).max(16.0);
    let h = (r.height as f32).max(16.0);
    let Some(rect) = Rect::from_xywh(r.x as f32, r.y as f32, w, h) else {
        return;
    };

    let stamp_id = obj
        .extra
        .get("StampId")
        .and_then(|v| v.as_str())
        .or(obj.tool_shape.as_deref())
        .unwrap_or("sym-check");
    let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let t = object_transform(obj, transform);
    let x0 = rect.x();
    let y0 = rect.y();
    let cx = x0 + w * 0.5;
    let min_d = w.min(h);

    let fill_path_color = |pm: &mut Pixmap, path: &Path, col: ColorRgba| {
        let mut p = Paint::default();
        p.anti_alias = true;
        p.set_color(to_skia_color(col, opacity));
        pm.fill_path(path, &p, FillRule::Winding, t, None);
    };
    let stroke_path_color = |pm: &mut Pixmap, path: &Path, col: ColorRgba, sw: f32| {
        let mut p = Paint::default();
        p.anti_alias = true;
        p.set_color(to_skia_color(col, opacity));
        let st = Stroke {
            width: sw.max(1.5),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pm.stroke_path(path, &p, &st, t, None);
    };

    match stamp_id {
        "sym-check" => {
            if let Some(circle) = PathBuilder::from_oval(rect) {
                draw_fill_shadow(pixmap, obj, &circle, t);
                fill_path_color(pixmap, &circle, ColorRgba::rgb(22, 163, 74));
                stroke_path_color(pixmap, &circle, ColorRgba::WHITE, min_d * 0.05);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(x0 + w * 0.27, y0 + h * 0.52);
            pb.line_to(x0 + w * 0.44, y0 + h * 0.69);
            pb.line_to(x0 + w * 0.74, y0 + h * 0.33);
            if let Some(check) = pb.finish() {
                stroke_path_color(pixmap, &check, ColorRgba::WHITE, min_d * 0.12);
            }
        }
        "sym-cross" => {
            if let Some(circle) = PathBuilder::from_oval(rect) {
                draw_fill_shadow(pixmap, obj, &circle, t);
                fill_path_color(pixmap, &circle, ColorRgba::rgb(220, 38, 38));
                stroke_path_color(pixmap, &circle, ColorRgba::WHITE, min_d * 0.05);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(x0 + w * 0.32, y0 + h * 0.32);
            pb.line_to(x0 + w * 0.68, y0 + h * 0.68);
            pb.move_to(x0 + w * 0.68, y0 + h * 0.32);
            pb.line_to(x0 + w * 0.32, y0 + h * 0.68);
            if let Some(x_path) = pb.finish() {
                stroke_path_color(pixmap, &x_path, ColorRgba::WHITE, min_d * 0.12);
            }
        }
        "sym-warn" => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, y0 + h * 0.08);
            pb.line_to(x0 + w * 0.92, y0 + h * 0.88);
            pb.line_to(x0 + w * 0.08, y0 + h * 0.88);
            pb.close();
            if let Some(tri) = pb.finish() {
                draw_fill_shadow(pixmap, obj, &tri, t);
                fill_path_color(pixmap, &tri, ColorRgba::rgb(245, 158, 11));
                stroke_path_color(pixmap, &tri, ColorRgba::rgb(120, 53, 15), min_d * 0.05);
            }
            let opts = TextRenderOptions {
                font_family: "Arial".to_string(),
                font_size: (min_d * 0.50).max(12.0),
                color: ColorRgba::rgb(17, 24, 39),
                bold: true,
                h_align: HorizontalAlign::Center,
                v_align: VerticalAlign::Center,
                opacity,
                ..TextRenderOptions::default()
            };
            render_text_box(pixmap, "!", x0, y0 + h * 0.18, w, h * 0.72, &opts, t);
        }
        "sym-info" | "sym-question" | "badge-thumbsup" | "sym-bug" => {
            let (bg, glyph) = match stamp_id {
                "sym-info" => (ColorRgba::rgb(37, 99, 235), "i"),
                "sym-question" => (ColorRgba::rgb(124, 58, 237), "?"),
                "badge-thumbsup" => (ColorRgba::rgb(5, 150, 105), "OK"),
                _ => (ColorRgba::rgb(190, 18, 60), "BUG"),
            };
            if let Some(circle) = PathBuilder::from_oval(rect) {
                draw_fill_shadow(pixmap, obj, &circle, t);
                fill_path_color(pixmap, &circle, bg);
                stroke_path_color(pixmap, &circle, ColorRgba::WHITE, min_d * 0.05);
            }
            let scale = if glyph.len() > 1 { 0.30 } else { 0.55 };
            let opts = TextRenderOptions {
                font_family: "Arial".to_string(),
                font_size: (min_d * scale).max(10.0),
                color: ColorRgba::WHITE,
                bold: true,
                h_align: HorizontalAlign::Center,
                v_align: VerticalAlign::Center,
                opacity,
                ..TextRenderOptions::default()
            };
            render_text_box(pixmap, glyph, x0, y0, w, h, &opts, t);
        }
        "sym-star" => {
            if let Some(star) = build_shape_path(rect, "Star", 0.0) {
                draw_fill_shadow(pixmap, obj, &star, t);
                fill_path_color(pixmap, &star, ColorRgba::rgb(250, 204, 21));
                stroke_path_color(pixmap, &star, ColorRgba::rgb(180, 83, 9), min_d * 0.05);
            }
        }
        "sym-heart" => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, y0 + h * 0.86);
            pb.cubic_to(
                x0 + w * 0.06,
                y0 + h * 0.56,
                x0 + w * 0.08,
                y0 + h * 0.16,
                x0 + w * 0.30,
                y0 + h * 0.16,
            );
            pb.cubic_to(
                x0 + w * 0.43,
                y0 + h * 0.16,
                cx,
                y0 + h * 0.28,
                cx,
                y0 + h * 0.34,
            );
            pb.cubic_to(
                cx,
                y0 + h * 0.28,
                x0 + w * 0.57,
                y0 + h * 0.16,
                x0 + w * 0.70,
                y0 + h * 0.16,
            );
            pb.cubic_to(
                x0 + w * 0.92,
                y0 + h * 0.16,
                x0 + w * 0.94,
                y0 + h * 0.56,
                cx,
                y0 + h * 0.86,
            );
            pb.close();
            if let Some(heart) = pb.finish() {
                draw_fill_shadow(pixmap, obj, &heart, t);
                fill_path_color(pixmap, &heart, ColorRgba::rgb(225, 29, 72));
                stroke_path_color(pixmap, &heart, ColorRgba::WHITE, min_d * 0.04);
            }
        }
        "sym-ban" => {
            let inset = min_d * 0.08;
            if let Some(inner_r) = Rect::from_xywh(x0 + inset, y0 + inset, w - inset * 2.0, h - inset * 2.0) {
                if let Some(circle) = PathBuilder::from_oval(inner_r) {
                    stroke_path_color(pixmap, &circle, ColorRgba::rgb(220, 38, 38), min_d * 0.12);
                }
            }
            let mut pb = PathBuilder::new();
            pb.move_to(x0 + w * 0.24, y0 + h * 0.24);
            pb.line_to(x0 + w * 0.76, y0 + h * 0.76);
            if let Some(slash) = pb.finish() {
                stroke_path_color(pixmap, &slash, ColorRgba::rgb(220, 38, 38), min_d * 0.12);
            }
        }
        "sym-pin" => {
            let rad = min_d * 0.34;
            let py = y0 + h * 0.38;
            let mut pb = PathBuilder::new();
            pb.move_to(cx, y0 + h * 0.92);
            pb.line_to(cx - rad * 0.75, py + rad * 0.45);
            pb.cubic_to(cx - rad * 1.1, py - rad * 0.5, cx - rad * 0.55, py - rad, cx, py - rad);
            pb.cubic_to(cx + rad * 0.55, py - rad, cx + rad * 1.1, py - rad * 0.5, cx + rad * 0.75, py + rad * 0.45);
            pb.close();
            if let Some(pin) = pb.finish() {
                draw_fill_shadow(pixmap, obj, &pin, t);
                fill_path_color(pixmap, &pin, ColorRgba::rgb(239, 68, 68));
                stroke_path_color(pixmap, &pin, ColorRgba::WHITE, min_d * 0.04);
            }
            if let Some(dot_r) = Rect::from_xywh(cx - rad * 0.35, py - rad * 0.35, rad * 0.7, rad * 0.7) {
                if let Some(dot) = PathBuilder::from_oval(dot_r) {
                    fill_path_color(pixmap, &dot, ColorRgba::WHITE);
                }
            }
        }
        "sym-flag" => {
            let mut pole = PathBuilder::new();
            pole.move_to(x0 + w * 0.22, y0 + h * 0.12);
            pole.line_to(x0 + w * 0.22, y0 + h * 0.90);
            if let Some(p) = pole.finish() {
                stroke_path_color(pixmap, &p, ColorRgba::rgb(55, 65, 81), min_d * 0.08);
            }
            let mut banner = PathBuilder::new();
            banner.move_to(x0 + w * 0.22, y0 + h * 0.14);
            banner.line_to(x0 + w * 0.84, y0 + h * 0.28);
            banner.line_to(x0 + w * 0.22, y0 + h * 0.54);
            banner.close();
            if let Some(b) = banner.finish() {
                fill_path_color(pixmap, &b, ColorRgba::rgb(234, 88, 12));
            }
        }
        "sym-lock" => {
            let mut shackle = PathBuilder::new();
            shackle.move_to(x0 + w * 0.32, y0 + h * 0.46);
            shackle.cubic_to(
                x0 + w * 0.32,
                y0 + h * 0.14,
                x0 + w * 0.68,
                y0 + h * 0.14,
                x0 + w * 0.68,
                y0 + h * 0.46,
            );
            if let Some(s) = shackle.finish() {
                stroke_path_color(pixmap, &s, ColorRgba::rgb(51, 65, 85), min_d * 0.09);
            }
            if let Some(body_r) = Rect::from_xywh(x0 + w * 0.20, y0 + h * 0.44, w * 0.60, h * 0.46) {
                if let Some(body) = build_rounded_rect_path(body_r, min_d * 0.08) {
                    fill_path_color(pixmap, &body, ColorRgba::rgb(15, 118, 110));
                    stroke_path_color(pixmap, &body, ColorRgba::WHITE, min_d * 0.04);
                }
            }
        }
        "ui-cursor" => {
            let mut pb = PathBuilder::new();
            pb.move_to(x0 + w * 0.22, y0 + h * 0.10);
            pb.line_to(x0 + w * 0.22, y0 + h * 0.80);
            pb.line_to(x0 + w * 0.42, y0 + h * 0.62);
            pb.line_to(x0 + w * 0.56, y0 + h * 0.88);
            pb.line_to(x0 + w * 0.68, y0 + h * 0.82);
            pb.line_to(x0 + w * 0.53, y0 + h * 0.56);
            pb.line_to(x0 + w * 0.78, y0 + h * 0.56);
            pb.close();
            if let Some(cur) = pb.finish() {
                draw_fill_shadow(pixmap, obj, &cur, t);
                fill_path_color(pixmap, &cur, ColorRgba::WHITE);
                stroke_path_color(pixmap, &cur, ColorRgba::rgb(17, 24, 39), min_d * 0.05);
            }
        }
        "ui-hand" => {
            if let Some(ring_r) = Rect::from_xywh(x0 + w * 0.15, y0 + h * 0.10, w * 0.50, h * 0.50) {
                if let Some(ring) = PathBuilder::from_oval(ring_r) {
                    stroke_path_color(pixmap, &ring, ColorRgba::rgb(14, 165, 233), min_d * 0.06);
                }
            }
            let mut pb = PathBuilder::new();
            pb.move_to(x0 + w * 0.38, y0 + h * 0.30);
            pb.line_to(x0 + w * 0.38, y0 + h * 0.86);
            pb.line_to(x0 + w * 0.78, y0 + h * 0.86);
            pb.line_to(x0 + w * 0.78, y0 + h * 0.54);
            pb.line_to(x0 + w * 0.48, y0 + h * 0.54);
            pb.line_to(x0 + w * 0.48, y0 + h * 0.30);
            pb.close();
            if let Some(hand) = pb.finish() {
                fill_path_color(pixmap, &hand, ColorRgba::WHITE);
                stroke_path_color(pixmap, &hand, ColorRgba::rgb(17, 24, 39), min_d * 0.05);
            }
        }
        "badge-bolt" => {
            let mut pb = PathBuilder::new();
            pb.move_to(x0 + w * 0.56, y0 + h * 0.06);
            pb.line_to(x0 + w * 0.22, y0 + h * 0.54);
            pb.line_to(x0 + w * 0.48, y0 + h * 0.54);
            pb.line_to(x0 + w * 0.40, y0 + h * 0.94);
            pb.line_to(x0 + w * 0.78, y0 + h * 0.44);
            pb.line_to(x0 + w * 0.52, y0 + h * 0.44);
            pb.close();
            if let Some(bolt) = pb.finish() {
                draw_fill_shadow(pixmap, obj, &bolt, t);
                fill_path_color(pixmap, &bolt, ColorRgba::rgb(250, 204, 21));
                stroke_path_color(pixmap, &bolt, ColorRgba::rgb(113, 63, 18), min_d * 0.04);
            }
        }
        _ => {
            if let Some(circle) = PathBuilder::from_oval(rect) {
                fill_path_color(
                    pixmap,
                    &circle,
                    obj.foreground_color.unwrap_or(ColorRgba::rgb(37, 99, 235)),
                );
                stroke_path_color(pixmap, &circle, ColorRgba::WHITE, min_d * 0.05);
            }
        }
    }
}

/// Exact geometric `hit_test(point, tolerance)` for all vector object types (including curved Arrows & rotated Shapes).
pub fn hit_test_object(obj: &CaptureObject, point: PointF, tolerance: f64) -> bool {
    let tol = tolerance.max(2.0);
    let mode = obj.tool_mode.clone().unwrap_or(ToolMode::Shape);

    // Apply inverse rotation around object center if rotated
    let test_pt = if let (Some(angle), Some(r)) = (obj.rotation_angle, object_bounds_rectf(obj)) {
        if angle.abs() > 1e-4 {
            let cx = r.x + r.width * 0.5;
            let cy = r.y + r.height * 0.5;
            let rad = (-angle).to_radians();
            let dx = point.x - cx;
            let dy = point.y - cy;
            PointF::new(
                cx + dx * rad.cos() - dy * rad.sin(),
                cy + dx * rad.sin() + dy * rad.cos(),
            )
        } else {
            point
        }
    } else {
        point
    };

    match mode {
        ToolMode::Arrow => {
            let stroke_half = obj.stroke_width.unwrap_or(6.0) * 0.5 + tol;
            let (Some(start), ctrl, Some(end)) = extract_arrow_points(obj) else {
                return false;
            };
            if let Some(c) = ctrl {
                distance_to_quad_bezier(test_pt, start, c, end) <= stroke_half
            } else {
                distance_to_segment(test_pt, start, end) <= stroke_half
            }
        }
        ToolMode::Line | ToolMode::Pen | ToolMode::Highlighter => {
            let stroke_half = obj.stroke_width.unwrap_or(4.0) * 0.5 + tol;
            if let Some(ref pts) = obj.points_array {
                for win in pts.windows(2) {
                    if distance_to_segment(test_pt, win[0], win[1]) <= stroke_half {
                        return true;
                    }
                }
                false
            } else if let (Some(s), Some(e)) = (obj.start_point, obj.end_point) {
                distance_to_segment(test_pt, s, e) <= stroke_half
            } else {
                false
            }
        }
        ToolMode::Shape => {
            let Some(r) = object_bounds_rectf(obj) else { return false };
            let shape_kind = obj.tool_shape.as_deref().unwrap_or("Rectangle");
            if matches!(shape_kind, "Ellipse" | "Oval" | "Circle") {
                let cx = r.x + r.width * 0.5;
                let cy = r.y + r.height * 0.5;
                let rx = (r.width * 0.5 + tol).max(1.0);
                let ry = (r.height * 0.5 + tol).max(1.0);
                let nx = (test_pt.x - cx) / rx;
                let ny = (test_pt.y - cy) / ry;
                nx * nx + ny * ny <= 1.0
            } else {
                RectF::new(r.x - tol, r.y - tol, r.width + tol * 2.0, r.height + tol * 2.0)
                    .contains(test_pt)
            }
        }
        _ => {
            let Some(r) = object_bounds_rectf(obj) else { return false };
            RectF::new(r.x - tol, r.y - tol, r.width + tol * 2.0, r.height + tol * 2.0)
                .contains(test_pt)
        }
    }
}

pub fn distance_to_segment(p: PointF, a: PointF, b: PointF) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-9 {
        return (p.x - a.x).hypot(p.y - a.y);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    let proj_x = a.x + t * dx;
    let proj_y = a.y + t * dy;
    (p.x - proj_x).hypot(p.y - proj_y)
}

pub fn distance_to_quad_bezier(p: PointF, p0: PointF, p1: PointF, p2: PointF) -> f64 {
    let steps = 24;
    let mut min_d = f64::INFINITY;
    let mut prev = p0;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let inv = 1.0 - t;
        let cur = PointF::new(
            inv * inv * p0.x + 2.0 * inv * t * p1.x + t * t * p2.x,
            inv * inv * p0.y + 2.0 * inv * t * p1.y + t * t * p2.y,
        );
        min_d = min_d.min(distance_to_segment(p, prev, cur));
        prev = cur;
    }
    min_d
}
