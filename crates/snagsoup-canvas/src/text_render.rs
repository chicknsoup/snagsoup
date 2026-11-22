use fontdue::{Font, FontSettings};
use snagsoup_model::ColorRgba;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tiny_skia::{Pixmap, Transform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizontalAlign {
    Left,
    Center,
    Right,
}

impl HorizontalAlign {
    pub fn from_snagit_str(s: Option<&str>) -> Self {
        match s.unwrap_or("Center") {
            "Left" | "Near" => Self::Left,
            "Right" | "Far" => Self::Right,
            _ => Self::Center,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Top,
    Center,
    Bottom,
}

impl VerticalAlign {
    pub fn from_snagit_str(s: Option<&str>) -> Self {
        match s.unwrap_or("Center") {
            "Top" | "Near" => Self::Top,
            "Bottom" | "Far" => Self::Bottom,
            _ => Self::Center,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextRenderOptions {
    pub font_family: String,
    pub font_size: f32,
    pub color: ColorRgba,
    pub outline_color: ColorRgba,
    pub outline_width: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub h_align: HorizontalAlign,
    pub v_align: VerticalAlign,
    pub opacity: f32,
}

impl Default for TextRenderOptions {
    fn default() -> Self {
        Self {
            font_family: "Arial".to_string(),
            font_size: 18.0,
            color: ColorRgba::BLACK,
            outline_color: ColorRgba::WHITE,
            outline_width: 0.0,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            h_align: HorizontalAlign::Center,
            v_align: VerticalAlign::Center,
            opacity: 1.0,
        }
    }
}

static FONT_CACHE: OnceLock<Mutex<HashMap<String, Option<Font>>>> = OnceLock::new();

fn resolve_windows_font_path(family: &str, bold: bool, italic: bool) -> Vec<&'static str> {
    let lower = family.to_ascii_lowercase();
    let mut candidates = Vec::new();
    if lower.contains("segoe") {
        match (bold, italic) {
            (true, true) => candidates.push(r"C:\Windows\Fonts\segoeuiz.ttf"),
            (true, false) => candidates.push(r"C:\Windows\Fonts\segoeuib.ttf"),
            (false, true) => candidates.push(r"C:\Windows\Fonts\segoeuii.ttf"),
            (false, false) => candidates.push(r"C:\Windows\Fonts\segoeui.ttf"),
        }
    } else if lower.contains("trebuchet") {
        match (bold, italic) {
            (true, true) => candidates.push(r"C:\Windows\Fonts\trebucbi.ttf"),
            (true, false) => candidates.push(r"C:\Windows\Fonts\trebucbd.ttf"),
            (false, true) => candidates.push(r"C:\Windows\Fonts\trebucit.ttf"),
            (false, false) => candidates.push(r"C:\Windows\Fonts\trebuc.ttf"),
        }
    } else if lower.contains("impact") {
        candidates.push(r"C:\Windows\Fonts\impact.ttf");
    } else if lower.contains("consolas") {
        candidates.push(r"C:\Windows\Fonts\consola.ttf");
    } else if lower.contains("calibri") {
        candidates.push(if bold {
            r"C:\Windows\Fonts\calibrib.ttf"
        } else {
            r"C:\Windows\Fonts\calibri.ttf"
        });
    } else if lower.contains("times") {
        candidates.push(r"C:\Windows\Fonts\times.ttf");
    }

    // Standard Arial / Segoe UI fallbacks always present on Windows
    match (bold, italic) {
        (true, true) => candidates.push(r"C:\Windows\Fonts\arialbi.ttf"),
        (true, false) => candidates.push(r"C:\Windows\Fonts\arialbd.ttf"),
        (false, true) => candidates.push(r"C:\Windows\Fonts\ariali.ttf"),
        (false, false) => candidates.push(r"C:\Windows\Fonts\arial.ttf"),
    }
    candidates.push(r"C:\Windows\Fonts\segoeui.ttf");
    candidates.push(r"C:\Windows\Fonts\arial.ttf");
    candidates
}

fn load_font(family: &str, bold: bool, italic: bool) -> Option<Font> {
    let key = format!("{}|{}|{}", family.to_ascii_lowercase(), bold, italic);
    let cache = FONT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap();
    if let Some(entry) = guard.get(&key) {
        return entry.clone();
    }

    let mut loaded = None;
    for path_str in resolve_windows_font_path(family, bold, italic) {
        let path = Path::new(path_str);
        if path.exists() {
            if let Ok(bytes) = fs::read(path) {
                if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                    loaded = Some(font);
                    break;
                }
            }
        }
    }
    guard.insert(key, loaded.clone());
    loaded
}

/// Renders multi-line wrapped text inside `(x, y, width, height)` in canvas space
/// with optional crisp outer text stroke (`outline_width` > 0), underline, and strikethrough.
pub fn render_text_box(
    pixmap: &mut Pixmap,
    text: &str,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    opts: &TextRenderOptions,
    transform: Transform,
) {
    if text.is_empty() || width < 1.0 || height < 1.0 || opts.opacity <= 0.0 {
        return;
    }
    let Some(font) = load_font(&opts.font_family, opts.bold, opts.italic) else {
        return;
    };

    // Scale font size by transform scale
    let scale = transform.sx.abs().max(0.1);
    let px_size = (opts.font_size * scale).clamp(4.0, 512.0);
    let box_w = (width * scale).max(1.0);
    let box_h = (height * scale).max(1.0);
    let origin_x = x * transform.sx + transform.tx;
    let origin_y = y * transform.sy + transform.ty;

    let line_metrics = font.horizontal_line_metrics(px_size);
    let ascent = line_metrics.map(|m| m.ascent).unwrap_or(px_size * 0.8);
    let line_height = line_metrics
        .map(|m| m.new_line_size)
        .unwrap_or(px_size * 1.2)
        .max(px_size);

    // Word-wrap lines
    let wrapped = wrap_lines(&font, text, px_size, box_w);
    if wrapped.is_empty() {
        return;
    }

    let total_text_h = wrapped.len() as f32 * line_height;
    let start_y = match opts.v_align {
        VerticalAlign::Top => origin_y,
        VerticalAlign::Center => origin_y + (box_h - total_text_h) * 0.5,
        VerticalAlign::Bottom => origin_y + (box_h - total_text_h),
    };

    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;

    // Collect rasterized glyph mask in local bounding region if outline is requested
    let outline_r = (opts.outline_width * scale).round().max(0.0) as i32;

    struct PlacedGlyph {
        gx: i32,
        gy: i32,
        w: usize,
        h: usize,
        bitmap: Vec<u8>,
    }

    let mut placed: Vec<PlacedGlyph> = Vec::new();
    let mut line_spans: Vec<(i32, i32, i32)> = Vec::new(); // (x0, x1, baseline_y)

    for (line_idx, line) in wrapped.iter().enumerate() {
        let line_w = measure_line_width(&font, line, px_size);
        let line_x = match opts.h_align {
            HorizontalAlign::Left => origin_x,
            HorizontalAlign::Center => origin_x + (box_w - line_w) * 0.5,
            HorizontalAlign::Right => origin_x + (box_w - line_w),
        };
        let baseline_y = start_y + (line_idx as f32) * line_height + ascent;
        line_spans.push((
            line_x.round() as i32,
            (line_x + line_w).round() as i32,
            baseline_y.round() as i32,
        ));

        let mut cursor_x = line_x;
        for ch in line.chars() {
            let (metrics, bitmap) = font.rasterize(ch, px_size);
            if metrics.width > 0 && metrics.height > 0 {
                let gx = (cursor_x + metrics.xmin as f32).round() as i32;
                let gy = (baseline_y - metrics.height as f32 - metrics.ymin as f32).round() as i32;
                placed.push(PlacedGlyph {
                    gx,
                    gy,
                    w: metrics.width,
                    h: metrics.height,
                    bitmap,
                });
            }
            cursor_x += metrics.advance_width;
        }
    }

    // 1. Render crisp outer text outline via circular dilation if outline_r > 0
    if outline_r > 0 && opts.outline_color.a > 0 {
        let r2 = outline_r * outline_r;
        let mut offsets = Vec::new();
        for dy in -outline_r..=outline_r {
            for dx in -outline_r..=outline_r {
                if dx * dx + dy * dy <= r2 {
                    offsets.push((dx, dy));
                }
            }
        }
        let data = pixmap.data_mut();
        for g in &placed {
            for row in 0..g.h {
                for col in 0..g.w {
                    let cov = g.bitmap[row * g.w + col];
                    if cov < 16 {
                        continue;
                    }
                    let px_x = g.gx + col as i32;
                    let px_y = g.gy + row as i32;
                    for &(dx, dy) in &offsets {
                        blend_pixel_premul(
                            data,
                            pw,
                            ph,
                            px_x + dx,
                            px_y + dy,
                            opts.outline_color,
                            cov,
                            opts.opacity,
                        );
                    }
                }
            }
        }
    }

    // 2. Render main text fill glyphs
    {
        let data = pixmap.data_mut();
        for g in &placed {
            for row in 0..g.h {
                for col in 0..g.w {
                    let cov = g.bitmap[row * g.w + col];
                    if cov == 0 {
                        continue;
                    }
                    blend_pixel_premul(
                        data,
                        pw,
                        ph,
                        g.gx + col as i32,
                        g.gy + row as i32,
                        opts.color,
                        cov,
                        opts.opacity,
                    );
                }
            }
        }

        // 3. Underline / Strikethrough
        let thickness = (px_size * 0.07).round().max(1.0) as i32;
        for &(x0, x1, base_y) in &line_spans {
            if opts.underline {
                let uy = base_y + (px_size * 0.12).round().max(1.0) as i32;
                for dy in 0..thickness {
                    for px_x in x0..x1 {
                        blend_pixel_premul(data, pw, ph, px_x, uy + dy, opts.color, 255, opts.opacity);
                    }
                }
            }
            if opts.strikethrough {
                let sy = base_y - (ascent * 0.35).round() as i32;
                for dy in 0..thickness {
                    for px_x in x0..x1 {
                        blend_pixel_premul(data, pw, ph, px_x, sy + dy, opts.color, 255, opts.opacity);
                    }
                }
            }
        }
    }
}

fn measure_line_width(font: &Font, line: &str, px_size: f32) -> f32 {
    line.chars()
        .map(|ch| font.metrics(ch, px_size).advance_width)
        .sum()
}

fn wrap_lines(font: &Font, text: &str, px_size: f32, max_width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for raw_line in text.split('\n') {
        let trimmed = raw_line.trim_end_matches('\r');
        if trimmed.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in trimmed.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if !current.is_empty() && measure_line_width(font, &candidate, px_size) > max_width {
                out.push(current);
                current = word.to_string();
            } else {
                current = candidate;
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn blend_pixel_premul(
    data: &mut [u8],
    pw: i32,
    ph: i32,
    x: i32,
    y: i32,
    color: ColorRgba,
    coverage: u8,
    opacity: f32,
) {
    if x < 0 || y < 0 || x >= pw || y >= ph {
        return;
    }
    let sa = ((color.a as f32 / 255.0) * (coverage as f32 / 255.0) * opacity).clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let idx = ((y as usize) * (pw as usize) + (x as usize)) * 4;
    let inv_sa = 1.0 - sa;
    let sr = (color.r as f32) * sa;
    let sg = (color.g as f32) * sa;
    let sb = (color.b as f32) * sa;

    data[idx] = (sr + data[idx] as f32 * inv_sa).round().clamp(0.0, 255.0) as u8;
    data[idx + 1] = (sg + data[idx + 1] as f32 * inv_sa).round().clamp(0.0, 255.0) as u8;
    data[idx + 2] = (sb + data[idx + 2] as f32 * inv_sa).round().clamp(0.0, 255.0) as u8;
    data[idx + 3] = ((sa * 255.0) + data[idx + 3] as f32 * inv_sa)
        .round()
        .clamp(0.0, 255.0) as u8;
}
