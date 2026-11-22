use crate::error::{CanvasError, Result};
use crate::raster_tools::{render_blur_or_pixelate, render_magnify_object};
use crate::tile_bitmap::{encode_rgba_to_png, PixelRect, TiledBitmap};
use crate::vector_g1::{render_arrow, render_shape, render_stamp, render_step, render_stroke_tool};
use crate::vector_g2::{render_callout, render_text_object};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use snagsoup_model::{CaptureObject, ColorRgba, PointF, RectF, SnagxPage, ToolMode};
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap,
    PixmapPaint, Point, Rect, SpreadMode, Transform,
};

/// Composites a `SnagxPage` + background `TiledBitmap` into flat RGBA8 or zoomed viewports.
pub struct SceneCompositor<'a> {
    pub page: &'a SnagxPage,
    pub bitmap: Option<&'a TiledBitmap>,
}

impl<'a> SceneCompositor<'a> {
    pub fn new(page: &'a SnagxPage, bitmap: Option<&'a TiledBitmap>) -> Self {
        Self { page, bitmap }
    }

    /// Resolves the logical canvas dimensions `(width, height)` in pixels.
    pub fn canvas_size(&self) -> (u32, u32) {
        let page_w = self.page.capture_canvas_width.unwrap_or(0.0).round().max(0.0) as u32;
        let page_h = self.page.capture_canvas_height.unwrap_or(0.0).round().max(0.0) as u32;
        let bmp_w = self.bitmap.map_or(0, |b| b.width());
        let bmp_h = self.bitmap.map_or(0, |b| b.height());
        let w = if page_w > 0 { page_w } else { bmp_w };
        let h = if page_h > 0 { page_h } else { bmp_h };
        (w, h)
    }

    /// Flattens the entire scene to a straight-alpha RGBA8 buffer `(width, height, rgba)`.
    pub fn flatten_to_rgba(&self) -> Result<(u32, u32, Vec<u8>)> {
        let (w, h) = self.canvas_size();
        if w == 0 || h == 0 {
            return Err(CanvasError::InvalidDimensions { width: w, height: h });
        }
        let mut pixmap = Pixmap::new(w, h)
            .ok_or(CanvasError::InvalidDimensions { width: w, height: h })?;
        self.render_into_pixmap(&mut pixmap, Transform::identity());
        Ok((w, h, pixmap_to_straight_rgba(&pixmap)))
    }

    /// Flattens the scene and applies a chain of global `EffectConfig`s in order.
    pub fn flatten_with_effects(
        &self,
        effects: &[snagsoup_model::EffectConfig],
    ) -> Result<(u32, u32, Vec<u8>)> {
        let (w, h, rgba) = self.flatten_to_rgba()?;
        if effects.is_empty() {
            return Ok((w, h, rgba));
        }
        Ok(crate::effects_geom::apply_effects(w, h, rgba, effects))
    }

    /// Renders only `viewport_rect` (in canvas coordinates) scaled by `zoom` into an RGBA8 buffer.
    pub fn render_viewport(&self, viewport_rect: RectF, zoom: f32) -> Result<(u32, u32, Vec<u8>)> {
        let zoom = zoom.max(0.01);
        let out_w = (viewport_rect.width as f32 * zoom).ceil().max(1.0) as u32;
        let out_h = (viewport_rect.height as f32 * zoom).ceil().max(1.0) as u32;
        let mut pixmap = Pixmap::new(out_w, out_h).ok_or(CanvasError::InvalidDimensions {
            width: out_w,
            height: out_h,
        })?;
        let transform = Transform::from_translate(
            -(viewport_rect.x as f32),
            -(viewport_rect.y as f32),
        )
        .post_scale(zoom, zoom);
        self.render_into_pixmap(&mut pixmap, transform);
        Ok((out_w, out_h, pixmap_to_straight_rgba(&pixmap)))
    }

    /// Generates an aspect-preserving RGBA8 thumbnail fitting within `(max_w, max_h)`.
    pub fn generate_thumbnail_rgba(&self, max_w: u32, max_h: u32) -> Result<(u32, u32, Vec<u8>)> {
        if max_w == 0 || max_h == 0 {
            return Err(CanvasError::InvalidDimensions {
                width: max_w,
                height: max_h,
            });
        }
        let (src_w, src_h, src_rgba) = self.flatten_to_rgba()?;
        let scale = (max_w as f32 / src_w as f32)
            .min(max_h as f32 / src_h as f32)
            .min(1.0);
        let dst_w = ((src_w as f32 * scale).round() as u32).max(1);
        let dst_h = ((src_h as f32 * scale).round() as u32).max(1);
        let resized = downscale_rgba_bilinear(&src_rgba, src_w, src_h, dst_w, dst_h);
        Ok((dst_w, dst_h, resized))
    }

    /// Generates a PNG-encoded thumbnail fitting within `(max_w, max_h)`.
    pub fn generate_thumbnail_png(&self, max_w: u32, max_h: u32) -> Result<Vec<u8>> {
        let (w, h, rgba) = self.generate_thumbnail_rgba(max_w, max_h)?;
        encode_rgba_to_png(w, h, &rgba)
    }

    fn render_into_pixmap(&self, pixmap: &mut Pixmap, transform: Transform) {
        let (cw, ch) = self.canvas_size();
        // 1. Solid or Gradient Canvas Background
        self.render_background(pixmap, cw as f32, ch as f32, transform);

        // 2. Background TiledBitmap Layer (scale-aware: 1:1 bit-exact at 100%, Fant area-average < 100%, Bicubic 100%..400%, Nearest >= 400%)
        if let Some(bmp) = self.bitmap {
            if bmp.width() > 0 && bmp.height() > 0 {
                let loc = self
                    .page
                    .capture_background_location
                    .unwrap_or(PointF::new(0.0, 0.0));
                let t = Transform::from_translate(loc.x as f32, loc.y as f32).post_concat(transform);
                draw_tiled_bitmap_crisp(pixmap, bmp, t);
            }
        }

        // 3. Collect Section Graphics + CaptureObjects sorted by ObjectPriority (stable)
        let mut objects: Vec<(usize, &CaptureObject)> = Vec::new();
        if let Some(ref sections) = self.page.sections {
            for sec in sections {
                if let Some(ref graphics) = sec.section_graphics {
                    for obj in graphics {
                        objects.push((objects.len(), obj));
                    }
                }
            }
        }
        for obj in &self.page.capture_objects {
            objects.push((objects.len(), obj));
        }
        objects.sort_by_key(|(idx, obj)| (obj.object_priority.unwrap_or(0), *idx));

        for (_, obj) in objects {
            render_object_preview(pixmap, obj, transform);
        }
    }

    fn render_background(&self, pixmap: &mut Pixmap, cw: f32, ch: f32, transform: Transform) {
        let Some(rect) = Rect::from_xywh(0.0, 0.0, cw, ch) else {
            return;
        };

        if let Some(ref grad) = self.page.gradient_background_color {
            if let Some(ref colors) = grad.gradient_colors {
                if colors.len() >= 2 {
                    let angle_rad = (grad.gradient_angle.unwrap_or(0.0) as f32).to_radians();
                    let dx = angle_rad.cos();
                    let dy = angle_rad.sin();
                    let cx = cw * 0.5;
                    let cy = ch * 0.5;
                    let half = (cw.hypot(ch)) * 0.5;
                    let start = Point::from_xy(cx - dx * half, cy - dy * half);
                    let end = Point::from_xy(cx + dx * half, cy + dy * half);
                    let stops: Vec<GradientStop> = colors
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let pos = i as f32 / (colors.len() - 1) as f32;
                            GradientStop::new(pos, to_skia_color(*c, 1.0))
                        })
                        .collect();
                    if let Some(shader) =
                        LinearGradient::new(start, end, stops, SpreadMode::Pad, Transform::identity())
                    {
                        let mut paint = Paint::default();
                        paint.shader = shader;
                        if let Some(path) = PathBuilder::from_rect(rect).into() {
                            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
                        }
                        return;
                    }
                }
            }
        }

        if let Some(bg) = self.page.capture_background_color {
            if bg.a > 0 {
                let mut paint = Paint::default();
                paint.set_color(to_skia_color(bg, 1.0));
                let path = PathBuilder::from_rect(rect);
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
        }
    }
}

fn to_skia_color(c: ColorRgba, opacity_mul: f32) -> Color {
    let a = ((c.a as f32 / 255.0) * opacity_mul).clamp(0.0, 1.0);
    Color::from_rgba(
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        a,
    )
    .unwrap_or(Color::TRANSPARENT)
}

fn object_bounds(obj: &CaptureObject) -> Option<Rect> {
    if let (Some(p1), Some(p2)) = (obj.start_point, obj.end_point) {
        let r = RectF::from_points(p1, p2);
        return Rect::from_xywh(
            r.x as f32,
            r.y as f32,
            (r.width as f32).max(1.0),
            (r.height as f32).max(1.0),
        );
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
            return Rect::from_xywh(
                min_x as f32,
                min_y as f32,
                ((max_x - min_x) as f32).max(1.0),
                ((max_y - min_y) as f32).max(1.0),
            );
        }
    }
    None
}

fn render_object_preview(pixmap: &mut Pixmap, obj: &CaptureObject, transform: Transform) {
    let opacity = (obj.opacity.unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    let mode = obj.tool_mode.clone().unwrap_or(ToolMode::Shape);

    // Embedded Base64 PNG image (Image / Cursor / DropZone with filled image)
    if let Some(ref img_b64) = obj.image {
        if !img_b64.is_empty() {
            if let Ok(raw) = BASE64.decode(img_b64.trim()) {
                if let Ok(bmp) = TiledBitmap::from_png_bytes(&raw) {
                    if let (Some(img_pm), Some(bounds)) =
                        (tiled_bitmap_to_pixmap(&bmp), object_bounds(obj))
                    {
                        let sx = bounds.width() / (bmp.width().max(1) as f32);
                        let sy = bounds.height() / (bmp.height().max(1) as f32);
                        let t = Transform::from_scale(sx, sy)
                            .post_translate(bounds.x(), bounds.y())
                            .post_concat(transform);
                        let eff_sx = (sx * transform.sx).abs();
                        let mut paint = PixmapPaint::default();
                        paint.opacity = opacity;
                        paint.quality = if (eff_sx - 1.0).abs() < 1e-3 || eff_sx >= 4.0 {
                            FilterQuality::Nearest
                        } else {
                            FilterQuality::Bicubic
                        };
                        pixmap.draw_pixmap(0, 0, img_pm.as_ref(), &paint, t, None);
                        return;
                    }
                }
            }
        }
    }

    match mode {
        ToolMode::Arrow => render_arrow(pixmap, obj, transform),
        ToolMode::Line | ToolMode::Pen | ToolMode::Highlighter => {
            render_stroke_tool(pixmap, obj, &mode, transform)
        }
        ToolMode::Step => render_step(pixmap, obj, transform),
        ToolMode::Stamp => render_stamp(pixmap, obj, transform),
        ToolMode::Callout => render_callout(pixmap, obj, transform),
        ToolMode::Text => render_text_object(pixmap, obj, transform),
        ToolMode::Blur => render_blur_or_pixelate(pixmap, obj, transform),
        ToolMode::Magnify => render_magnify_object(pixmap, obj, transform),
        _ => render_shape(pixmap, obj, transform),
    }
}

/// Scale-aware high-fidelity background `TiledBitmap` renderer:
/// - `scale == 1.0` (`100%`): exact integer 1:1 copy of only the visible `PixelRect` (zero blur)
/// - `scale < 1.0` (`< 100%`): subpixel Fant box/area-average integration (preserves crisp 1px text stems)
/// - `1.0 < scale < 4.0`: smooth `FilterQuality::Bicubic` interpolation
/// - `scale >= 4.0`: crisp `FilterQuality::Nearest` pixel-grid inspection
fn draw_tiled_bitmap_crisp(pixmap: &mut Pixmap, bmp: &TiledBitmap, t: Transform) {
    let dst_w = pixmap.width();
    let dst_h = pixmap.height();
    let src_w = bmp.width();
    let src_h = bmp.height();
    if dst_w == 0 || dst_h == 0 || src_w == 0 || src_h == 0 || t.sx <= 0.0 || t.sy <= 0.0 {
        return;
    }

    // Compute visible source region in `bmp` coordinates so we never copy off-screen tiles on tall captures
    let inv_x0 = ((0.0 - t.tx) / t.sx).floor() - 2.0;
    let inv_y0 = ((0.0 - t.ty) / t.sy).floor() - 2.0;
    let inv_x1 = (((dst_w as f32) - t.tx) / t.sx).ceil() + 2.0;
    let inv_y1 = (((dst_h as f32) - t.ty) / t.sy).ceil() + 2.0;

    let sx0 = inv_x0.clamp(0.0, src_w as f32) as u32;
    let sy0 = inv_y0.clamp(0.0, src_h as f32) as u32;
    let sx1 = inv_x1.clamp(0.0, src_w as f32) as u32;
    let sy1 = inv_y1.clamp(0.0, src_h as f32) as u32;
    if sx1 <= sx0 || sy1 <= sy0 {
        return;
    }
    let sub_w = sx1 - sx0;
    let sub_h = sy1 - sy0;
    let sub_rgba = bmp.read_region(PixelRect::new(sx0, sy0, sub_w, sub_h));
    let sub_tx = t.tx + (sx0 as f32) * t.sx;
    let sub_ty = t.ty + (sy0 as f32) * t.sy;

    // 1. Exact 1:1 native pixel copy (`100%` zoom) -> zero resampling blur!
    if (t.sx - 1.0).abs() < 1e-3 && (t.sy - 1.0).abs() < 1e-3 && t.kx == 0.0 && t.ky == 0.0 {
        let ox = sub_tx.round() as i32;
        let oy = sub_ty.round() as i32;
        let dst = pixmap.data_mut();
        for ly in 0..sub_h as i32 {
            let dy = oy + ly;
            if dy < 0 || dy >= dst_h as i32 {
                continue;
            }
            for lx in 0..sub_w as i32 {
                let dx = ox + lx;
                if dx < 0 || dx >= dst_w as i32 {
                    continue;
                }
                let si = ((ly as usize) * (sub_w as usize) + (lx as usize)) * 4;
                let di = ((dy as usize) * (dst_w as usize) + (dx as usize)) * 4;
                blend_straight_over_premul(&sub_rgba[si..si + 4], &mut dst[di..di + 4]);
            }
        }
        return;
    }

    // 2. Downscaling (`< 100%` zoom): exact Fant subpixel box/area-averaging so 1px UI fonts stay sharp!
    if t.sx < 1.0 && t.sy < 1.0 && t.kx == 0.0 && t.ky == 0.0 {
        let dst = pixmap.data_mut();
        let dy_min = sub_ty.floor().max(0.0) as u32;
        let dy_max = (sub_ty + (sub_h as f32) * t.sy).ceil().clamp(0.0, dst_h as f32) as u32;
        let dx_min = sub_tx.floor().max(0.0) as u32;
        let dx_max = (sub_tx + (sub_w as f32) * t.sx).ceil().clamp(0.0, dst_w as f32) as u32;

        for dy in dy_min..dy_max {
            let fy0 = ((dy as f32 - sub_ty) / t.sy).clamp(0.0, sub_h as f32);
            let fy1 = (((dy + 1) as f32 - sub_ty) / t.sy).clamp(0.0, sub_h as f32);
            if fy1 <= fy0 {
                continue;
            }
            let iy0 = fy0.floor() as u32;
            let iy1 = (fy1.ceil() as u32).min(sub_h);

            for dx in dx_min..dx_max {
                let fx0 = ((dx as f32 - sub_tx) / t.sx).clamp(0.0, sub_w as f32);
                let fx1 = (((dx + 1) as f32 - sub_tx) / t.sx).clamp(0.0, sub_w as f32);
                if fx1 <= fx0 {
                    continue;
                }
                let ix0 = fx0.floor() as u32;
                let ix1 = (fx1.ceil() as u32).min(sub_w);

                let mut acc_r = 0.0f32;
                let mut acc_g = 0.0f32;
                let mut acc_b = 0.0f32;
                let mut acc_a = 0.0f32;
                let mut acc_w = 0.0f32;

                for iy in iy0..iy1 {
                    let wy = (fy1.min((iy + 1) as f32) - fy0.max(iy as f32)).max(0.0);
                    let row_off = (iy as usize) * (sub_w as usize);
                    for ix in ix0..ix1 {
                        let wx = (fx1.min((ix + 1) as f32) - fx0.max(ix as f32)).max(0.0);
                        let w = wx * wy;
                        let si = (row_off + (ix as usize)) * 4;
                        let a = sub_rgba[si + 3] as f32 / 255.0;
                        acc_r += (sub_rgba[si] as f32) * a * w;
                        acc_g += (sub_rgba[si + 1] as f32) * a * w;
                        acc_b += (sub_rgba[si + 2] as f32) * a * w;
                        acc_a += (sub_rgba[si + 3] as f32) * w;
                        acc_w += w;
                    }
                }

                if acc_w > 0.0 {
                    let inv = 1.0 / acc_w;
                    let pm = [
                        (acc_r * inv).round().clamp(0.0, 255.0) as u8,
                        (acc_g * inv).round().clamp(0.0, 255.0) as u8,
                        (acc_b * inv).round().clamp(0.0, 255.0) as u8,
                        (acc_a * inv).round().clamp(0.0, 255.0) as u8,
                    ];
                    let di = ((dy as usize) * (dst_w as usize) + (dx as usize)) * 4;
                    blend_premul_over_premul(&pm, &mut dst[di..di + 4]);
                }
            }
        }
        return;
    }

    // 3. Upscaling (`100%..400%` -> Bicubic; `>= 400%` -> Nearest pixel grid)
    if let Some(mut sub_pm) = Pixmap::new(sub_w, sub_h) {
        for (s, d) in sub_rgba
            .chunks_exact(4)
            .zip(sub_pm.data_mut().chunks_exact_mut(4))
        {
            let a = s[3] as u16;
            if a == 255 {
                d.copy_from_slice(s);
            } else if a > 0 {
                d[0] = ((s[0] as u16 * a + 127) / 255) as u8;
                d[1] = ((s[1] as u16 * a + 127) / 255) as u8;
                d[2] = ((s[2] as u16 * a + 127) / 255) as u8;
                d[3] = s[3];
            }
        }
        let mut paint = PixmapPaint::default();
        paint.quality = if t.sx >= 4.0 && t.sy >= 4.0 {
            FilterQuality::Nearest
        } else {
            FilterQuality::Bicubic
        };
        let sub_t = Transform::from_row(t.sx, t.ky, t.kx, t.sy, sub_tx, sub_ty);
        pixmap.draw_pixmap(0, 0, sub_pm.as_ref(), &paint, sub_t, None);
    }
}

#[inline]
fn blend_straight_over_premul(src: &[u8], dst: &mut [u8]) {
    let a = src[3] as u16;
    if a == 255 {
        dst.copy_from_slice(src);
    } else if a > 0 {
        let inv_a = 255 - a;
        dst[0] = ((src[0] as u16 * a + dst[0] as u16 * inv_a + 127) / 255) as u8;
        dst[1] = ((src[1] as u16 * a + dst[1] as u16 * inv_a + 127) / 255) as u8;
        dst[2] = ((src[2] as u16 * a + dst[2] as u16 * inv_a + 127) / 255) as u8;
        dst[3] = (a + (dst[3] as u16 * inv_a + 127) / 255).min(255) as u8;
    }
}

#[inline]
fn blend_premul_over_premul(pm: &[u8; 4], dst: &mut [u8]) {
    let a = pm[3] as u16;
    if a == 255 {
        dst.copy_from_slice(pm);
    } else if a > 0 {
        let inv_a = 255 - a;
        dst[0] = (pm[0] as u16 + (dst[0] as u16 * inv_a + 127) / 255).min(255) as u8;
        dst[1] = (pm[1] as u16 + (dst[1] as u16 * inv_a + 127) / 255).min(255) as u8;
        dst[2] = (pm[2] as u16 + (dst[2] as u16 * inv_a + 127) / 255).min(255) as u8;
        dst[3] = (a + (dst[3] as u16 * inv_a + 127) / 255).min(255) as u8;
    }
}

/// Converts a `TiledBitmap` (straight RGBA8) to a `tiny-skia::Pixmap` (premultiplied RGBA8).
fn tiled_bitmap_to_pixmap(bmp: &TiledBitmap) -> Option<Pixmap> {
    let mut pm = Pixmap::new(bmp.width(), bmp.height())?;
    let src = bmp.to_rgba();
    let dst = pm.data_mut();
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let a = s[3] as u16;
        if a == 255 {
            d.copy_from_slice(s);
        } else if a == 0 {
            d.copy_from_slice(&[0, 0, 0, 0]);
        } else {
            d[0] = ((s[0] as u16 * a + 127) / 255) as u8;
            d[1] = ((s[1] as u16 * a + 127) / 255) as u8;
            d[2] = ((s[2] as u16 * a + 127) / 255) as u8;
            d[3] = s[3];
        }
    }
    Some(pm)
}

/// Converts a `tiny-skia::Pixmap` (premultiplied RGBA8) back to straight RGBA8.
fn pixmap_to_straight_rgba(pm: &Pixmap) -> Vec<u8> {
    let src = pm.data();
    let mut out = vec![0u8; src.len()];
    for (s, d) in src.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let a = s[3];
        if a == 255 || a == 0 {
            d.copy_from_slice(s);
        } else {
            let af = a as f32 / 255.0;
            d[0] = ((s[0] as f32 / af).round().clamp(0.0, 255.0)) as u8;
            d[1] = ((s[1] as f32 / af).round().clamp(0.0, 255.0)) as u8;
            d[2] = ((s[2] as f32 / af).round().clamp(0.0, 255.0)) as u8;
            d[3] = a;
        }
    }
    out
}

/// Fast area/bilinear downscaler for RGBA8 thumbnails.
pub fn downscale_rgba_bilinear(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<u8> {
    if src_w == dst_w && src_h == dst_h {
        return src.to_vec();
    }
    let mut out = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;

    for dy in 0..dst_h {
        let sy = ((dy as f32 + 0.5) * y_ratio - 0.5).clamp(0.0, (src_h - 1) as f32);
        let y0 = sy.floor() as u32;
        let y1 = (y0 + 1).min(src_h - 1);
        let wy = sy - y0 as f32;

        for dx in 0..dst_w {
            let sx = ((dx as f32 + 0.5) * x_ratio - 0.5).clamp(0.0, (src_w - 1) as f32);
            let x0 = sx.floor() as u32;
            let x1 = (x0 + 1).min(src_w - 1);
            let wx = sx - x0 as f32;

            let i00 = ((y0 * src_w + x0) as usize) * 4;
            let i10 = ((y0 * src_w + x1) as usize) * 4;
            let i01 = ((y1 * src_w + x0) as usize) * 4;
            let i11 = ((y1 * src_w + x1) as usize) * 4;
            let dst_i = ((dy * dst_w + dx) as usize) * 4;

            for c in 0..4 {
                let top = src[i00 + c] as f32 * (1.0 - wx) + src[i10 + c] as f32 * wx;
                let bot = src[i01 + c] as f32 * (1.0 - wx) + src[i11 + c] as f32 * wx;
                out[dst_i + c] = (top * (1.0 - wy) + bot * wy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}
