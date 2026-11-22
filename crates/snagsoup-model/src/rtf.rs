use crate::error::Result;
use crate::geometry::ColorRgba;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RichTextStyle {
    pub font_family: Option<String>,
    pub font_size_pt: Option<f32>,
    pub color: Option<ColorRgba>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RichTextSpan {
    pub text: String,
    pub style: RichTextStyle,
}

/// Decodes a Snagit `RTFEncodedText` Base64 string into raw RTF text.
pub fn decode_rtf_base64(b64: &str) -> Result<String> {
    let bytes = BASE64.decode(b64.trim())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Encodes raw RTF text into a Snagit `RTFEncodedText` Base64 string.
pub fn encode_rtf_base64(rtf: &str) -> String {
    BASE64.encode(rtf.as_bytes())
}

/// Extracts plain text from an RTF string (skipping header tables and control words).
pub fn extract_plain_text(rtf: &str) -> String {
    let spans = parse_rtf_spans(rtf);
    let mut out = String::new();
    for s in spans {
        out.push_str(&s.text);
    }
    out
}

/// Lightweight pure-Rust RTF parser extracting text spans with Font, Size, Color, Bold, Italic, Underline.
pub fn parse_rtf_spans(rtf: &str) -> Vec<RichTextSpan> {
    let bytes = rtf.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    let mut font_table: Vec<String> = Vec::new();
    let mut color_table: Vec<Option<ColorRgba>> = Vec::new();

    #[derive(Clone, Default)]
    struct State {
        style: RichTextStyle,
        skip_group: bool,
        in_fonttbl: bool,
        in_colortbl: bool,
        uc_skip: usize,
    }

    let mut stack: Vec<State> = vec![State {
        uc_skip: 1,
        ..State::default()
    }];

    let mut spans: Vec<RichTextSpan> = Vec::new();
    let mut current_text = String::new();
    let mut current_style = RichTextStyle::default();

    // Helpers for parsing fonttbl / colortbl entries
    let mut cur_font_name = String::new();
    let mut cur_r: Option<u8> = None;
    let mut cur_g: Option<u8> = None;
    let mut cur_b: Option<u8> = None;

    let flush_span = |spans: &mut Vec<RichTextSpan>, text: &mut String, style: &RichTextStyle| {
        if !text.is_empty() {
            if let Some(last) = spans.last_mut() {
                if last.style == *style {
                    last.text.push_str(text);
                    text.clear();
                    return;
                }
            }
            spans.push(RichTextSpan {
                text: std::mem::take(text),
                style: style.clone(),
            });
        }
    };

    while i < len {
        let b = bytes[i];
        match b {
            b'{' => {
                let top = stack.last().cloned().unwrap_or_default();
                stack.push(top);
                i += 1;
            }
            b'}' => {
                if let Some(popped) = stack.pop() {
                    if popped.in_fonttbl && !cur_font_name.trim().is_empty() {
                        let cleaned = cur_font_name.trim().trim_end_matches(';').to_string();
                        if !cleaned.is_empty() {
                            font_table.push(cleaned);
                        }
                        cur_font_name.clear();
                    }
                }
                if let Some(top) = stack.last() {
                    if !top.skip_group && top.style != current_style {
                        flush_span(&mut spans, &mut current_text, &current_style);
                        current_style = top.style.clone();
                    }
                }
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= len {
                    break;
                }
                let next = bytes[i];
                if next == b'\\' || next == b'{' || next == b'}' {
                    if let Some(top) = stack.last() {
                        if !top.skip_group && !top.in_fonttbl && !top.in_colortbl {
                            if top.style != current_style {
                                flush_span(&mut spans, &mut current_text, &current_style);
                                current_style = top.style.clone();
                            }
                            current_text.push(next as char);
                        }
                    }
                    i += 1;
                } else if next == b'*' {
                    if let Some(top) = stack.last_mut() {
                        top.skip_group = true;
                    }
                    i += 1;
                } else if next == b'\'' {
                    // Hex escape \'hh
                    i += 1;
                    if i + 2 <= len {
                        if let Ok(hex_str) = std::str::from_utf8(&bytes[i..i + 2]) {
                            if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                                if let Some(top) = stack.last() {
                                    if !top.skip_group && !top.in_fonttbl && !top.in_colortbl {
                                        if top.style != current_style {
                                            flush_span(&mut spans, &mut current_text, &current_style);
                                            current_style = top.style.clone();
                                        }
                                        current_text.push(val as char);
                                    }
                                }
                            }
                        }
                        i += 2;
                    }
                } else if next == b'\n' || next == b'\r' {
                    if let Some(top) = stack.last() {
                        if !top.skip_group && !top.in_fonttbl && !top.in_colortbl {
                            current_text.push('\n');
                        }
                    }
                    i += 1;
                } else if next.is_ascii_alphabetic() {
                    let word_start = i;
                    while i < len && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = &rtf[word_start..i];

                    let mut neg = false;
                    if i < len && bytes[i] == b'-' {
                        neg = true;
                        i += 1;
                    }
                    let num_start = i;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let param: Option<i32> = if i > num_start {
                        rtf[num_start..i].parse::<i32>().ok().map(|v| if neg { -v } else { v })
                    } else {
                        None
                    };

                    // Optional single space delimiter after control word
                    if i < len && bytes[i] == b' ' {
                        i += 1;
                    }

                    let top = stack.last_mut().unwrap();
                    match word {
                        "fonttbl" => {
                            top.in_fonttbl = true;
                            top.skip_group = true;
                        }
                        "colortbl" => {
                            top.in_colortbl = true;
                            top.skip_group = true;
                        }
                        "stylesheet" | "info" | "pict" | "object" | "header" | "footer"
                        | "expandedcolortbl" | "listtable" | "listoverridetable" => {
                            top.skip_group = true;
                        }
                        "red" if top.in_colortbl => {
                            cur_r = param.map(|v| v.clamp(0, 255) as u8);
                        }
                        "green" if top.in_colortbl => {
                            cur_g = param.map(|v| v.clamp(0, 255) as u8);
                        }
                        "blue" if top.in_colortbl => {
                            cur_b = param.map(|v| v.clamp(0, 255) as u8);
                        }
                        "uc" => {
                            if let Some(n) = param {
                                top.uc_skip = n.max(0) as usize;
                            }
                        }
                        "u" if !top.skip_group => {
                            if let Some(code) = param {
                                let u16_val = if code < 0 {
                                    (code + 65536) as u32
                                } else {
                                    code as u32
                                };
                                if let Some(ch) = char::from_u32(u16_val) {
                                    if top.style != current_style {
                                        flush_span(&mut spans, &mut current_text, &current_style);
                                        current_style = top.style.clone();
                                    }
                                    current_text.push(ch);
                                }
                                // Skip fallback ANSI chars (uc_skip)
                                let mut skipped = 0;
                                while skipped < top.uc_skip && i < len {
                                    if bytes[i] == b'\\' && i + 1 < len && bytes[i + 1] == b'\'' {
                                        i = (i + 4).min(len);
                                    } else if bytes[i] == b'{' || bytes[i] == b'}' {
                                        break;
                                    } else {
                                        i += 1;
                                    }
                                    skipped += 1;
                                }
                            }
                        }
                        "par" | "line" if !top.skip_group => {
                            if top.style != current_style {
                                flush_span(&mut spans, &mut current_text, &current_style);
                                current_style = top.style.clone();
                            }
                            current_text.push('\n');
                        }
                        "tab" if !top.skip_group => {
                            if top.style != current_style {
                                flush_span(&mut spans, &mut current_text, &current_style);
                                current_style = top.style.clone();
                            }
                            current_text.push('\t');
                        }
                        "b" if !top.skip_group => {
                            top.style.bold = param != Some(0);
                        }
                        "i" if !top.skip_group => {
                            top.style.italic = param != Some(0);
                        }
                        "ul" if !top.skip_group => {
                            top.style.underline = param != Some(0);
                        }
                        "ulnone" if !top.skip_group => {
                            top.style.underline = false;
                        }
                        "fs" if !top.skip_group => {
                            if let Some(half_pts) = param {
                                top.style.font_size_pt = Some(half_pts as f32 / 2.0);
                            }
                        }
                        "f" if !top.skip_group => {
                            if let Some(idx) = param {
                                if let Some(name) = font_table.get(idx as usize) {
                                    top.style.font_family = Some(name.clone());
                                }
                            }
                        }
                        "cf" if !top.skip_group => {
                            if let Some(idx) = param {
                                if let Some(col) = color_table.get(idx as usize) {
                                    top.style.color = *col;
                                }
                            }
                        }
                        "plain" if !top.skip_group => {
                            top.style.bold = false;
                            top.style.italic = false;
                            top.style.underline = false;
                        }
                        _ => {}
                    }
                } else {
                    // Other control symbol like \~ or \-
                    i += 1;
                }
            }
            b'\r' | b'\n' => {
                i += 1;
            }
            _ => {
                let ch = rtf[i..].chars().next().unwrap();
                let ch_len = ch.len_utf8();
                if let Some(top) = stack.last() {
                    if top.in_fonttbl {
                        if ch == ';' {
                            let cleaned = cur_font_name.trim().to_string();
                            if !cleaned.is_empty() {
                                font_table.push(cleaned);
                            }
                            cur_font_name.clear();
                        } else {
                            cur_font_name.push(ch);
                        }
                    } else if top.in_colortbl {
                        if ch == ';' {
                            if let (Some(r), Some(g), Some(b)) = (cur_r, cur_g, cur_b) {
                                color_table.push(Some(ColorRgba::rgb(r, g, b)));
                            } else {
                                color_table.push(None);
                            }
                            cur_r = None;
                            cur_g = None;
                            cur_b = None;
                        }
                    } else if !top.skip_group {
                        if top.style != current_style {
                            flush_span(&mut spans, &mut current_text, &current_style);
                            current_style = top.style.clone();
                        }
                        current_text.push(ch);
                    }
                }
                i += ch_len;
            }
        }
    }

    flush_span(&mut spans, &mut current_text, &current_style);
    spans
}

/// Generates a compact, valid RTF 1.0 document from styled text spans.
pub fn generate_rtf(spans: &[RichTextSpan]) -> String {
    let mut fonts: Vec<String> = Vec::new();
    let mut colors: Vec<ColorRgba> = Vec::new();

    for span in spans {
        if let Some(ref f) = span.style.font_family {
            if !fonts.contains(f) {
                fonts.push(f.clone());
            }
        }
        if let Some(c) = span.style.color {
            if !colors.contains(&c) {
                colors.push(c);
            }
        }
    }
    if fonts.is_empty() {
        fonts.push("Arial".to_string());
    }

    let mut out = String::from("{\\rtf1\\ansi\\deff0");

    // Font table
    out.push_str("{\\fonttbl");
    for (idx, font) in fonts.iter().enumerate() {
        out.push_str(&format!("{{\\f{idx}\\fnil {font};}}"));
    }
    out.push('}');

    // Color table (index 0 is auto/empty)
    if !colors.is_empty() {
        out.push_str("{\\colortbl;");
        for c in &colors {
            out.push_str(&format!("\\red{}\\green{}\\blue{};", c.r, c.g, c.b));
        }
        out.push('}');
    }

    for span in spans {
        out.push('{');
        if let Some(ref f) = span.style.font_family {
            if let Some(idx) = fonts.iter().position(|x| x == f) {
                out.push_str(&format!("\\f{idx}"));
            }
        }
        if let Some(sz) = span.style.font_size_pt {
            let half_pts = (sz * 2.0).round() as i32;
            out.push_str(&format!("\\fs{half_pts}"));
        }
        if let Some(c) = span.style.color {
            if let Some(idx) = colors.iter().position(|x| x == &c) {
                out.push_str(&format!("\\cf{}", idx + 1));
            }
        }
        if span.style.bold {
            out.push_str("\\b");
        }
        if span.style.italic {
            out.push_str("\\i");
        }
        if span.style.underline {
            out.push_str("\\ul");
        }
        out.push(' ');

        for ch in span.text.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '{' => out.push_str("\\{"),
                '}' => out.push_str("\\}"),
                '\n' => out.push_str("\\par "),
                '\t' => out.push_str("\\tab "),
                c if c.is_ascii() => out.push(c),
                c => {
                    let code = c as u32;
                    if code <= 0xFFFF {
                        let signed = code as i16;
                        out.push_str(&format!("\\u{signed}?"));
                    } else {
                        out.push('?');
                    }
                }
            }
        }
        out.push('}');
    }

    out.push('}');
    out
}
