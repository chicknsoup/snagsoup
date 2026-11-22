use crate::text_render::{render_text_box, HorizontalAlign, TextRenderOptions, VerticalAlign};
use crate::vector_g1::{
    build_rounded_rect_path, draw_fill_shadow, draw_stroke_shadow, make_stroke_dash,
    object_bounds_rectf, object_transform, to_skia_color,
};
use snagsoup_model::{
    decode_rtf_base64, extract_plain_text, parse_rtf_spans, CaptureObject, ColorRgba, PointF,
};
use tiny_skia::{
    FillRule, GradientStop, LineJoin, LinearGradient, Paint, Path, PathBuilder, Pixmap,
    PixmapPaint, Point, Rect, SpreadMode, Stroke, Transform,
};

/// Extracts displayable text and RTF style overrides from a `CaptureObject`.
pub fn extract_object_text(obj: &CaptureObject) -> (String, TextRenderOptions) {
    let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let mut opts = TextRenderOptions {
        font_family: obj.font_family.clone().unwrap_or_else(|| "Arial".to_string()),
        font_size: obj.font_size.unwrap_or(18.0).max(8.0) as f32,
        color: obj
            .extra
            .get("TextSelectionColor")
            .and_then(|v| v.as_str())
            .and_then(|s| ColorRgba::from_hex(s).ok())
            .filter(|c| c.a > 0)
            .or(obj.foreground_color)
            .unwrap_or(ColorRgba::BLACK),
        outline_color: obj.text_outline_color.unwrap_or(ColorRgba::WHITE),
        outline_width: obj.text_outline_width.unwrap_or(0.0) as f32,
        bold: obj
            .extra
            .get("TextSelectionBold")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        italic: obj
            .extra
            .get("TextSelectionItalic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        underline: obj
            .extra
            .get("TextSelectionUnderline")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        strikethrough: obj
            .extra
            .get("TextSelectionStrikethrough")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        h_align: HorizontalAlign::from_snagit_str(
            obj.extra.get("ToolHorizontalAlign").and_then(|v| v.as_str()),
        ),
        v_align: VerticalAlign::from_snagit_str(
            obj.extra.get("ToolVerticalAlign").and_then(|v| v.as_str()),
        ),
        opacity,
    };

    if let Some(ref b64) = obj.rtf_encoded_text {
        if let Ok(rtf) = decode_rtf_base64(b64) {
            let spans = parse_rtf_spans(&rtf);
            if let Some(first) = spans.first() {
                if let Some(ref f) = first.style.font_family {
                    opts.font_family = f.clone();
                }
                if let Some(sz) = first.style.font_size_pt {
                    opts.font_size = sz;
                }
                if let Some(c) = first.style.color {
                    opts.color = c;
                }
                opts.bold |= first.style.bold;
                opts.italic |= first.style.italic;
                opts.underline |= first.style.underline;
            }
            let plain = extract_plain_text(&rtf);
            if !plain.trim().is_empty() {
                return (plain, opts);
            }
        }
    }

    let text = obj.placeholder_text.clone().unwrap_or_default();
    (text, opts)
}

/// Builds a unified callout outline merging the body rectangle with 1..N `TailPoints`
/// so the outer border wraps seamlessly around the body + tail(s) with no seam across the tail base!
pub fn build_unified_callout_path(rect: Rect, tails: &[PointF], shape: &str) -> Option<Path> {
    if shape.starts_with("CTThoughtCloud") {
        return PathBuilder::from_oval(rect);
    }
    if tails.is_empty() {
        let rad = if shape.contains("Rounded") || shape.contains("Balloon") || shape.contains("Speech") {
            rect.width().min(rect.height()) * 0.15
        } else {
            0.0
        };
        return build_rounded_rect_path(rect, rad);
    }

    // Build a single continuous perimeter polygon inserting tail notches into the closest edge
    // Edges: 0 = Top (L->R), 1 = Right (T->B), 2 = Bottom (R->L), 3 = Left (B->T)
    let x0 = rect.x();
    let y0 = rect.y();
    let x1 = rect.right();
    let y1 = rect.bottom();
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let half_base = (rect.width().min(rect.height()) * 0.12).clamp(6.0, 24.0);

    #[derive(Clone, Copy)]
    struct EdgeTail {
        edge: u8,
        t: f32, // 0..1 along edge direction
        tip_x: f32,
        tip_y: f32,
    }

    let mut edge_tails: Vec<EdgeTail> = Vec::new();
    for tail in tails {
        let tx = tail.x as f32;
        let ty = tail.y as f32;
        let dx = tx - cx;
        let dy = ty - cy;
        let (edge, t) = if dy.abs() * rect.width() >= dx.abs() * rect.height() {
            if dy < 0.0 {
                (0u8, ((tx - x0) / rect.width()).clamp(0.15, 0.85))
            } else {
                (2u8, ((x1 - tx) / rect.width()).clamp(0.15, 0.85))
            }
        } else if dx >= 0.0 {
            (1u8, ((ty - y0) / rect.height()).clamp(0.15, 0.85))
        } else {
            (3u8, ((y1 - ty) / rect.height()).clamp(0.15, 0.85))
        };
        edge_tails.push(EdgeTail {
            edge,
            t,
            tip_x: tx,
            tip_y: ty,
        });
    }

    let mut pb = PathBuilder::new();
    pb.move_to(x0, y0);

    for edge in 0u8..4u8 {
        let mut on_edge: Vec<EdgeTail> = edge_tails.iter().copied().filter(|e| e.edge == edge).collect();
        on_edge.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));

        for et in on_edge {
            match edge {
                0 => {
                    let bx = x0 + et.t * rect.width();
                    pb.line_to((bx - half_base).max(x0), y0);
                    pb.line_to(et.tip_x, et.tip_y);
                    pb.line_to((bx + half_base).min(x1), y0);
                }
                1 => {
                    let by = y0 + et.t * rect.height();
                    pb.line_to(x1, (by - half_base).max(y0));
                    pb.line_to(et.tip_x, et.tip_y);
                    pb.line_to(x1, (by + half_base).min(y1));
                }
                2 => {
                    let bx = x1 - et.t * rect.width();
                    pb.line_to((bx + half_base).min(x1), y1);
                    pb.line_to(et.tip_x, et.tip_y);
                    pb.line_to((bx - half_base).max(x0), y1);
                }
                _ => {
                    let by = y1 - et.t * rect.height();
                    pb.line_to(x0, (by + half_base).min(y1));
                    pb.line_to(et.tip_x, et.tip_y);
                    pb.line_to(x0, (by - half_base).max(y0));
                }
            }
        }

        match edge {
            0 => pb.line_to(x1, y0),
            1 => pb.line_to(x1, y1),
            2 => pb.line_to(x0, y1),
            _ => pb.close(),
        }
    }

    pb.finish()
}

/// Renders a Snagit Callout (unified body + 1..N tails, optional gradient, drop shadow, and internal text).
pub fn render_callout(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let Some(r) = object_bounds_rectf(obj) else { return };
    let Some(rect) = Rect::from_xywh(r.x as f32, r.y as f32, (r.width as f32).max(4.0), (r.height as f32).max(4.0)) else {
        return;
    };

    let shape = obj
        .callout_shape
        .as_deref()
        .unwrap_or("CTRoundedRectWithArrow");
    let tails = obj.tail_points.as_deref().unwrap_or(&[]);
    let Some(path) = build_unified_callout_path(rect, tails, shape) else {
        return;
    };

    let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    let fg = obj.foreground_color.unwrap_or(ColorRgba::rgb(244, 189, 33));
    let bg = obj.background_color.unwrap_or(fg);
    let stroke_w = obj.stroke_width.unwrap_or(2.0).max(0.0) as f32;
    let use_gradient = obj
        .extra
        .get("CalloutGradient")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let t = object_transform(obj, transform);

    if bg.a > 0 {
        draw_fill_shadow(pixmap, obj, &path, t);
        let mut fill = Paint::default();
        fill.anti_alias = true;
        if use_gradient {
            let light = ColorRgba::new(
                bg.r.saturating_add(35),
                bg.g.saturating_add(35),
                bg.b.saturating_add(35),
                bg.a,
            );
            if let Some(shader) = LinearGradient::new(
                Point::from_xy(rect.x(), rect.y()),
                Point::from_xy(rect.x(), rect.bottom()),
                vec![
                    GradientStop::new(0.0, to_skia_color(light, opacity)),
                    GradientStop::new(1.0, to_skia_color(bg, opacity)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                fill.shader = shader;
            } else {
                fill.set_color(to_skia_color(bg, opacity));
            }
        } else {
            fill.set_color(to_skia_color(bg, opacity));
        }
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

    // Internal callout text with padding
    let pad = obj
        .extra
        .get("ToolPadding")
        .and_then(|v| v.as_f64())
        .unwrap_or(6.0) as f32;
    let (text, text_opts) = extract_object_text(obj);
    if !text.is_empty() {
        render_text_box(
            pixmap,
            &text,
            rect.x() + pad,
            rect.y() + pad,
            (rect.width() - pad * 2.0).max(1.0),
            (rect.height() - pad * 2.0).max(1.0),
            &text_opts,
            t,
        );
    }
}

/// Renders a Rich Text object with optional background fill and crisp outer text outline (`TextOutlineWidth`).
pub fn render_text_object(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let Some(r) = object_bounds_rectf(obj) else { return };
    let t = object_transform(obj, transform);

    if let Some(bg) = obj.background_color {
        if bg.a > 0 {
            if let Some(rect) = Rect::from_xywh(r.x as f32, r.y as f32, (r.width as f32).max(1.0), (r.height as f32).max(1.0)) {
                let path = PathBuilder::from_rect(rect);
                let mut fill = Paint::default();
                fill.set_color(to_skia_color(bg, (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0)));
                pixmap.fill_path(&path, &fill, FillRule::Winding, t, None);
            }
        }
    }

    let (text, text_opts) = extract_object_text(obj);
    if !text.is_empty() {
        render_text_box(
            pixmap,
            &text,
            r.x as f32,
            r.y as f32,
            (r.width as f32).max(1.0),
            (r.height as f32).max(1.0),
            &text_opts,
            t,
        );
    }
}

/// Renders an SVG stamp from raw UTF-8 SVG bytes into a `Pixmap` of `(width, height)` via `resvg`.
pub fn render_svg_stamp_to_pixmap(svg_bytes: &[u8], width: u32, height: u32) -> Option<Pixmap> {
    if width == 0 || height == 0 {
        return None;
    }
    let opts = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(svg_bytes, &opts).ok()?;
    let size = tree.size();
    let sx = width as f32 / size.width().max(1.0);
    let sy = height as f32 / size.height().max(1.0);
    let mut pm = Pixmap::new(width, height)?;
    resvg::render(&tree, Transform::from_scale(sx, sy), &mut pm.as_mut());
    Some(pm)
}

/// Renders UTF-8 SVG bytes into straight (unpremultiplied) `RGBA8` pixels (`width * height * 4` bytes).
pub fn render_svg_to_rgba(svg_bytes: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let pm = render_svg_stamp_to_pixmap(svg_bytes, width, height)?;
    let src = pm.data();
    let mut out = vec![0u8; src.len()];
    for (s, d) in src.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let a = s[3];
        if a == 0 {
            d.copy_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            d.copy_from_slice(s);
        } else {
            let af = a as u32;
            d[0] = ((s[0] as u32 * 255 + af / 2) / af).min(255) as u8;
            d[1] = ((s[1] as u32 * 255 + af / 2) / af).min(255) as u8;
            d[2] = ((s[2] as u32 * 255 + af / 2) / af).min(255) as u8;
            d[3] = a;
        }
    }
    Some(out)
}

/// Draws an SVG stamp onto `pixmap` inside `obj`'s bounding rect.
pub fn draw_svg_stamp(
    pixmap: &mut Pixmap,
    obj: &CaptureObject,
    svg_bytes: &[u8],
    transform: Transform,
) {
    let Some(r) = object_bounds_rectf(obj) else { return };
    let w = (r.width.round() as u32).max(1);
    let h = (r.height.round() as u32).max(1);
    if let Some(stamp_pm) = render_svg_stamp_to_pixmap(svg_bytes, w, h) {
        let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
        let mut paint = PixmapPaint::default();
        paint.opacity = opacity;
        let t = Transform::from_translate(r.x as f32, r.y as f32)
            .post_concat(object_transform(obj, transform));
        pixmap.draw_pixmap(0, 0, stamp_pm.as_ref(), &paint, t, None);
    }
}
