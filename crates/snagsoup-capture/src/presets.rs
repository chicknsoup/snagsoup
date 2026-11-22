//! Capture Preset store (JSON persistence) and Snagit 2022 `Presets4.xml` importer
//! powered by pure-Rust `quick-xml` (Zero-CC).

use crate::CaptureError;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use snagsoup_model::{CaptureKind, CapturePreset, SelectionMode, TimedCaptureConfig};
use std::fs;
use std::path::Path;

/// Persistent collection of user capture presets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetStore {
    pub presets: Vec<CapturePreset>,
}

impl PresetStore {
    /// Built-in default presets matching Snagit's starter presets.
    pub fn with_defaults() -> Self {
        let mut image_region = CapturePreset::default();
        image_region.id = "preset-image-region".into();
        image_region.name = "Image Region".into();
        image_region.kind = CaptureKind::Image;
        image_region.selection_mode = SelectionMode::Region;
        image_region.hotkey = None;

        let mut fullscreen = CapturePreset::default();
        fullscreen.id = "preset-fullscreen".into();
        fullscreen.name = "Full Screen".into();
        fullscreen.kind = CaptureKind::Image;
        fullscreen.selection_mode = SelectionMode::FullScreen;
        fullscreen.hotkey = None;

        let mut panoramic = CapturePreset::default();
        panoramic.id = "preset-panoramic".into();
        panoramic.name = "Panoramic Scrolling".into();
        panoramic.kind = CaptureKind::Image;
        panoramic.selection_mode = SelectionMode::Panoramic;
        panoramic.hotkey = None;

        Self {
            presets: vec![CapturePreset::default(), image_region, fullscreen, panoramic],
        }
    }

    pub fn from_json_str(json: &str) -> Result<Self, CaptureError> {
        serde_json::from_str(json).map_err(|e| CaptureError::PresetParse(e.to_string()))
    }

    pub fn to_json_str(&self) -> Result<String, CaptureError> {
        serde_json::to_string_pretty(self).map_err(|e| CaptureError::PresetParse(e.to_string()))
    }

    pub fn load_file(path: &Path) -> Result<Self, CaptureError> {
        let text = fs::read_to_string(path).map_err(|e| CaptureError::Io(e.to_string()))?;
        Self::from_json_str(&text)
    }

    pub fn save_file(&self, path: &Path) -> Result<(), CaptureError> {
        let text = self.to_json_str()?;
        fs::write(path, text).map_err(|e| CaptureError::Io(e.to_string()))
    }

    pub fn add_or_update(&mut self, preset: CapturePreset) {
        if let Some(existing) = self.presets.iter_mut().find(|p| p.id == preset.id) {
            *existing = preset;
        } else {
            self.presets.push(preset);
        }
    }

    pub fn remove_by_id(&mut self, id: &str) -> bool {
        let before = self.presets.len();
        self.presets.retain(|p| p.id != id);
        self.presets.len() != before
    }

    pub fn find_by_id(&self, id: &str) -> Option<&CapturePreset> {
        self.presets.iter().find(|p| p.id == id)
    }
}

impl Default for PresetStore {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Import capture presets from TechSmith Snagit 2022's `Presets4.xml` format.
pub fn import_snagit_presets4_xml(xml_str: &str) -> Result<Vec<CapturePreset>, CaptureError> {
    let mut reader = Reader::from_str(xml_str);
    reader.config_mut().trim_text(true);

    let mut presets = Vec::new();
    let mut current: Option<CapturePreset> = None;
    let mut current_tag = String::new();
    let mut index = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag.eq_ignore_ascii_case("Preset") {
                    index += 1;
                    let mut p = CapturePreset::default();
                    p.id = format!("snagit-import-{index}");
                    p.hotkey = None;
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref());
                        let val = String::from_utf8_lossy(&attr.value);
                        if key.eq_ignore_ascii_case("id") || key.eq_ignore_ascii_case("guid") {
                            p.id = val.to_string();
                        } else if key.eq_ignore_ascii_case("name") {
                            p.name = val.to_string();
                        }
                    }
                    current = Some(p);
                }
                current_tag = tag;
            }
            Ok(Event::Text(t)) => {
                if let Some(ref mut p) = current {
                    let val = t
                        .unescape()
                        .map_err(|e| CaptureError::PresetParse(e.to_string()))?
                        .trim()
                        .to_string();
                    if val.is_empty() {
                        continue;
                    }
                    match current_tag.to_ascii_lowercase().as_str() {
                        "name" | "presetname" => p.name = val,
                        "hotkey" => p.hotkey = Some(val),
                        "capturetype" | "mode" => {
                            p.kind = match val.to_ascii_lowercase().as_str() {
                                "image" => CaptureKind::Image,
                                "video" => CaptureKind::Video,
                                _ => CaptureKind::AllInOne,
                            };
                        }
                        "selectiontype" | "input" | "selectionmode" => {
                            p.selection_mode = parse_selection_mode(&val);
                        }
                        "capturecursor" | "includecursor" => {
                            p.capture_cursor = parse_xml_bool(&val);
                        }
                        "copytoclipboard" => {
                            p.copy_to_clipboard = parse_xml_bool(&val);
                        }
                        "previewineditor" => {
                            p.preview_in_editor = parse_xml_bool(&val);
                        }
                        "timedelay" | "delayseconds" => {
                            if let Ok(sec) = val.parse::<u32>() {
                                p.timed_capture = TimedCaptureConfig {
                                    enabled: sec > 0,
                                    delay_seconds: sec,
                                    show_countdown: sec > 0,
                                    repeat_interval_seconds: None,
                                };
                            }
                        }
                        "output" | "sharedestination" => {
                            p.share_destination = Some(val);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                let tag = String::from_utf8_lossy(name.as_ref());
                if tag.eq_ignore_ascii_case("Preset") {
                    if let Some(p) = current.take() {
                        presets.push(p);
                    }
                }
                current_tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(CaptureError::PresetParse(format!(
                    "XML error at position {}: {e}",
                    reader.error_position()
                )));
            }
            _ => {}
        }
    }

    Ok(presets)
}

fn parse_xml_bool(val: &str) -> bool {
    matches!(
        val.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

fn parse_selection_mode(val: &str) -> SelectionMode {
    match val.trim().to_ascii_lowercase().as_str() {
        "region" => SelectionMode::Region,
        "window" => SelectionMode::Window,
        "fullscreen" | "full screen" | "alldisplays" => SelectionMode::FullScreen,
        "scrolling" | "scrollingwindow" => SelectionMode::ScrollingWindow,
        "panoramic" => SelectionMode::Panoramic,
        "fixedregion" | "fixed" => SelectionMode::FixedRegion,
        "freehand" => SelectionMode::Freehand,
        _ => SelectionMode::AllInOne,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_store_json_roundtrip() {
        let store = PresetStore::with_defaults();
        let json = store.to_json_str().expect("serialize");
        let parsed = PresetStore::from_json_str(&json).expect("deserialize");
        assert_eq!(parsed, store);
    }

    #[test]
    fn import_snagit_presets4_xml_parses_multiple_presets() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
        <Presets version="4">
            <Preset id="p-1">
                <Name>Delayed Menu Capture</Name>
                <CaptureType>Image</CaptureType>
                <SelectionType>Region</SelectionType>
                <Hotkey>Ctrl+Shift+M</Hotkey>
                <CaptureCursor>true</CaptureCursor>
                <CopyToClipboard>1</CopyToClipboard>
                <PreviewInEditor>true</PreviewInEditor>
                <TimeDelay>5</TimeDelay>
                <Output>Clipboard</Output>
            </Preset>
            <Preset id="p-2">
                <Name>Full Screen Quick</Name>
                <CaptureType>Image</CaptureType>
                <SelectionType>FullScreen</SelectionType>
                <CaptureCursor>false</CaptureCursor>
                <PreviewInEditor>0</PreviewInEditor>
            </Preset>
        </Presets>"#;

        let imported = import_snagit_presets4_xml(xml).expect("parse Presets4.xml");
        assert_eq!(imported.len(), 2);

        assert_eq!(imported[0].id, "p-1");
        assert_eq!(imported[0].name, "Delayed Menu Capture");
        assert_eq!(imported[0].kind, CaptureKind::Image);
        assert_eq!(imported[0].selection_mode, SelectionMode::Region);
        assert_eq!(imported[0].hotkey.as_deref(), Some("Ctrl+Shift+M"));
        assert!(imported[0].capture_cursor);
        assert!(imported[0].copy_to_clipboard);
        assert!(imported[0].timed_capture.enabled);
        assert_eq!(imported[0].timed_capture.delay_seconds, 5);
        assert_eq!(imported[0].share_destination.as_deref(), Some("Clipboard"));

        assert_eq!(imported[1].name, "Full Screen Quick");
        assert_eq!(imported[1].selection_mode, SelectionMode::FullScreen);
        assert!(!imported[1].preview_in_editor);
    }
}
