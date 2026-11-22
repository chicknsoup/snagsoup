use crate::error::{CvOcrError, Result};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use snagsoup_model::{
    decode_rtf_base64, encode_rtf_base64, extract_plain_text, generate_rtf, parse_rtf_spans,
    CaptureObject, RichTextSpan, RichTextStyle, SnagxArchive, ToolMode,
};
use std::collections::HashMap;
use std::io::Cursor;

#[derive(Debug, Clone, PartialEq)]
pub enum TranslationIssue {
    TextOverflow {
        object_id: String,
        translated_text: String,
        estimated_height_px: f64,
        box_height_px: f64,
    },
    MissingTarget {
        object_id: String,
    },
}

/// Exports all `Text` and `Callout` strings in a `.snagx` archive into XLIFF 1.2 XML.
pub fn export_snagx_to_xliff(
    archive: &SnagxArchive,
    source_lang: &str,
    target_lang: &str,
) -> Result<String> {
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);

    let mut xliff = BytesStart::new("xliff");
    xliff.push_attribute(("version", "1.2"));
    xliff.push_attribute(("xmlns", "urn:oasis:names:tc:xliff:document:1.2"));
    writer
        .write_event(Event::Start(xliff))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;

    let mut file_tag = BytesStart::new("file");
    file_tag.push_attribute(("original", "snagx"));
    file_tag.push_attribute(("source-language", source_lang));
    file_tag.push_attribute(("target-language", target_lang));
    file_tag.push_attribute(("datatype", "plaintext"));
    writer
        .write_event(Event::Start(file_tag))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;

    writer
        .write_event(Event::Start(BytesStart::new("body")))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;

    let mut counter = 0usize;
    for page_entry in &archive.pages {
        for obj in &page_entry.page.capture_objects {
            if let Some((id, text)) = extract_translatable_object(obj, &mut counter) {
                write_trans_unit(&mut writer, &id, &text)?;
            }
        }
        if let Some(ref sections) = page_entry.page.sections {
            for sec in sections {
                if let Some(ref graphics) = sec.section_graphics {
                    for obj in graphics {
                        if let Some((id, text)) = extract_translatable_object(obj, &mut counter) {
                            write_trans_unit(&mut writer, &id, &text)?;
                        }
                    }
                }
            }
        }
    }

    writer
        .write_event(Event::End(BytesEnd::new("body")))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;
    writer
        .write_event(Event::End(BytesEnd::new("file")))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;
    writer
        .write_event(Event::End(BytesEnd::new("xliff")))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;

    let bytes = writer.into_inner().into_inner();
    String::from_utf8(bytes).map_err(|e| CvOcrError::InvalidInput(e.to_string()))
}

fn write_trans_unit(writer: &mut Writer<Cursor<Vec<u8>>>, id: &str, text: &str) -> Result<()> {
    let mut tu = BytesStart::new("trans-unit");
    tu.push_attribute(("id", id));
    writer
        .write_event(Event::Start(tu))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;

    writer
        .write_event(Event::Start(BytesStart::new("source")))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;
    writer
        .write_event(Event::Text(BytesText::new(text)))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;
    writer
        .write_event(Event::End(BytesEnd::new("source")))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;

    let mut target = BytesStart::new("target");
    target.push_attribute(("state", "needs-translation"));
    writer
        .write_event(Event::Empty(target))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;

    writer
        .write_event(Event::End(BytesEnd::new("trans-unit")))
        .map_err(|e| CvOcrError::InvalidInput(e.to_string()))?;
    Ok(())
}

fn extract_translatable_object(obj: &CaptureObject, counter: &mut usize) -> Option<(String, String)> {
    if !matches!(obj.tool_mode, Some(ToolMode::Text) | Some(ToolMode::Callout)) {
        return None;
    }
    let id = obj.object_id.clone().unwrap_or_else(|| {
        *counter += 1;
        format!("obj-{counter}")
    });
    if let Some(ref b64) = obj.rtf_encoded_text {
        if let Ok(rtf) = decode_rtf_base64(b64) {
            let plain = extract_plain_text(&rtf);
            if !plain.trim().is_empty() {
                return Some((id, plain));
            }
        }
    }
    if let Some(ref ph) = obj.placeholder_text {
        if !ph.trim().is_empty() {
            return Some((id, ph.clone()));
        }
    }
    None
}

/// Imports a translated XLIFF 1.2 XML string into a cloned `.snagx` archive, updating `RTFEncodedText`
/// and returning any `TranslationIssue::TextOverflow` warnings when translated text exceeds box height.
pub fn import_xliff_to_snagx(
    archive: &SnagxArchive,
    xliff_xml: &str,
) -> Result<(SnagxArchive, Vec<TranslationIssue>)> {
    let translations = parse_xliff_targets(xliff_xml)?;
    let mut out = archive.clone();
    let mut issues = Vec::new();
    let mut counter = 0usize;

    for page_entry in &mut out.pages {
        for obj in &mut page_entry.page.capture_objects {
            apply_translation_to_object(obj, &translations, &mut counter, &mut issues);
        }
        if let Some(ref mut sections) = page_entry.page.sections {
            for sec in sections {
                if let Some(ref mut graphics) = sec.section_graphics {
                    for obj in graphics {
                        apply_translation_to_object(obj, &translations, &mut counter, &mut issues);
                    }
                }
            }
        }
    }

    Ok((out, issues))
}

fn parse_xliff_targets(xml: &str) -> Result<HashMap<String, String>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut map = HashMap::new();

    let mut cur_id: Option<String> = None;
    let mut in_target = false;
    let mut cur_target = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"trans-unit" => {
                    cur_id = None;
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"id" {
                            cur_id = attr.unescape_value().ok().map(|s| s.into_owned());
                        }
                    }
                }
                b"target" => {
                    in_target = true;
                    cur_target.clear();
                }
                _ => {}
            },
            Ok(Event::Text(ref e)) if in_target => {
                if let Ok(t) = e.unescape() {
                    cur_target.push_str(&t);
                }
            }
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"target" => {
                    in_target = false;
                    if let Some(ref id) = cur_id {
                        if !cur_target.trim().is_empty() {
                            map.insert(id.clone(), cur_target.clone());
                        }
                    }
                }
                b"trans-unit" => {
                    cur_id = None;
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(CvOcrError::InvalidInput(format!("XLIFF parse error: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(map)
}

fn apply_translation_to_object(
    obj: &mut CaptureObject,
    translations: &HashMap<String, String>,
    counter: &mut usize,
    issues: &mut Vec<TranslationIssue>,
) {
    if !matches!(obj.tool_mode, Some(ToolMode::Text) | Some(ToolMode::Callout)) {
        return;
    }
    let id = obj.object_id.clone().unwrap_or_else(|| {
        *counter += 1;
        format!("obj-{counter}")
    });

    let Some(translated) = translations.get(&id) else {
        issues.push(TranslationIssue::MissingTarget { object_id: id });
        return;
    };

    // Preserve first span style from existing RTF if available
    let mut style = RichTextStyle {
        font_family: obj.font_family.clone(),
        font_size_pt: obj.font_size.map(|s| s as f32),
        color: obj.foreground_color,
        ..RichTextStyle::default()
    };
    if let Some(ref b64) = obj.rtf_encoded_text {
        if let Ok(rtf) = decode_rtf_base64(b64) {
            if let Some(first) = parse_rtf_spans(&rtf).first() {
                style = first.style.clone();
            }
        }
    }

    let new_rtf = generate_rtf(&[RichTextSpan {
        text: translated.clone(),
        style: style.clone(),
    }]);
    obj.rtf_encoded_text = Some(encode_rtf_base64(&new_rtf));
    obj.placeholder_text = Some(translated.clone());

    // Overflow heuristic: estimate wrapped line count against bounding box height
    if let (Some(p1), Some(p2)) = (obj.start_point, obj.end_point) {
        let box_w = (p2.x - p1.x).abs().max(20.0);
        let box_h = (p2.y - p1.y).abs().max(10.0);
        let font_sz = style
            .font_size_pt
            .map(|f| f as f64)
            .or(obj.font_size)
            .unwrap_or(16.0);
        let avg_char_w = font_sz * 0.58;
        let chars_per_line = (box_w / avg_char_w).floor().max(1.0);
        let mut total_lines = 0.0;
        for para in translated.split('\n') {
            let len = para.chars().count().max(1) as f64;
            total_lines += (len / chars_per_line).ceil();
        }
        let est_h = total_lines * font_sz * 1.28;
        if est_h > box_h * 1.05 {
            issues.push(TranslationIssue::TextOverflow {
                object_id: id,
                translated_text: translated.clone(),
                estimated_height_px: est_h,
                box_height_px: box_h,
            });
        }
    }
}
