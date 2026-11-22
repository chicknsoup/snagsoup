use crate::error::Result;
use crate::geometry::ColorRgba;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// All 21 Snagit 2022 tools plus special canvas object modes (`DropZone`, `Image`, `Cursor`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolMode {
    Move,
    Arrow,
    Text,
    Callout,
    Shape,
    Stamp,
    Fill,
    Selection,
    Blur,
    Crop,
    CutOut,
    Pen,
    Line,
    Highlighter,
    Step,
    Simplify,
    Eraser,
    Magnify,
    SmartMove,
    MagicWand,
    Eyedropper,
    // Special object modes in .snagx / .snagt
    DropZone,
    Image,
    Cursor,
    /// Forward-compatible fallback for any unrecognized tool mode string.
    Unknown(String),
}

impl ToolMode {
    pub fn as_str(&self) -> &str {
        match self {
            ToolMode::Move => "Move",
            ToolMode::Arrow => "Arrow",
            ToolMode::Text => "Text",
            ToolMode::Callout => "Callout",
            ToolMode::Shape => "Shape",
            ToolMode::Stamp => "Stamp",
            ToolMode::Fill => "Fill",
            ToolMode::Selection => "Selection",
            ToolMode::Blur => "Blur",
            ToolMode::Crop => "Crop",
            ToolMode::CutOut => "CutOut",
            ToolMode::Pen => "Pen",
            ToolMode::Line => "Line",
            ToolMode::Highlighter => "Highlighter",
            ToolMode::Step => "Step",
            ToolMode::Simplify => "Simplify",
            ToolMode::Eraser => "Eraser",
            ToolMode::Magnify => "Magnify",
            ToolMode::SmartMove => "SmartMove",
            ToolMode::MagicWand => "MagicWand",
            ToolMode::Eyedropper => "Eyedropper",
            ToolMode::DropZone => "DropZone",
            ToolMode::Image => "Image",
            ToolMode::Cursor => "Cursor",
            ToolMode::Unknown(s) => s.as_str(),
        }
    }

    pub fn from_snagit_str(s: &str) -> Self {
        match s {
            "Move" => ToolMode::Move,
            "Arrow" => ToolMode::Arrow,
            "Text" => ToolMode::Text,
            "Callout" => ToolMode::Callout,
            "Shape" => ToolMode::Shape,
            "Stamp" => ToolMode::Stamp,
            "Fill" => ToolMode::Fill,
            "Selection" => ToolMode::Selection,
            "Blur" => ToolMode::Blur,
            "Crop" => ToolMode::Crop,
            "CutOut" => ToolMode::CutOut,
            "Pen" => ToolMode::Pen,
            "Line" => ToolMode::Line,
            "Highlighter" => ToolMode::Highlighter,
            "Step" => ToolMode::Step,
            "Simplify" => ToolMode::Simplify,
            "Eraser" => ToolMode::Eraser,
            "Magnify" => ToolMode::Magnify,
            "SmartMove" => ToolMode::SmartMove,
            "MagicWand" => ToolMode::MagicWand,
            "Eyedropper" => ToolMode::Eyedropper,
            "DropZone" => ToolMode::DropZone,
            "Image" => ToolMode::Image,
            "Cursor" => ToolMode::Cursor,
            other => ToolMode::Unknown(other.to_string()),
        }
    }
}

impl Default for ToolMode {
    fn default() -> Self {
        ToolMode::Move
    }
}

impl Serialize for ToolMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(ToolMode::from_snagit_str(&s))
    }
}

/// Complete tool style & property set matching `Favorites.json` and `.snagtheme` `QuickStyles`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ToolProperties {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_mode: Option<ToolMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_color: Option<ColorRgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<ColorRgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_color: Option<ColorRgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_shadow_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start_cap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end_cap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_start_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_end_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_color: Option<ColorRgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callout_shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_sequence_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_padding: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_max_padding: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_selection_bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_selection_italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_selection_underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_selection_color: Option<ColorRgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_outline_color: Option<ColorRgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_outline_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pen_shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_type: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_intensity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_horizontal_align: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_vertical_align: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_direction_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_direction_y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antialiasing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simplify_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simplify_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simplify_shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simplify_text_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simplify_line_spacing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callout_gradient: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_as_vector: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub magnify_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_out_style: Option<i32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Alias matching Snagit's QuickStyle concept.
pub type QuickStyle = ToolProperties;

/// `Favorites.json` root container.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FavoritesFile {
    #[serde(rename = "Quickstyles", alias = "QuickStyles", default)]
    pub quick_styles: Vec<QuickStyle>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl FavoritesFile {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        fs::write(path, text)?;
        Ok(())
    }
}

/// `.snagtheme` file schema.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SnagTheme {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: f64,
    #[serde(alias = "Quickstyles", default)]
    pub quick_styles: Vec<QuickStyle>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SnagTheme {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        fs::write(path, text)?;
        Ok(())
    }
}

/// `.snagpalette` file schema for Auto-Simplify color palettes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SnagPalette {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub palette_colors: Vec<ColorRgba>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SnagPalette {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        fs::write(path, text)?;
        Ok(())
    }
}
