use crate::error::{ModelError, Result};
use crate::geometry::{
    opt_point_str_serde, opt_vec_point_str_serde, ColorRgba, PointF,
};
use crate::tools::ToolMode;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Deserializes an optional boolean that may be encoded as JSON `true`/`false` OR integer `0`/`1` (macOS Snagit).
mod opt_bool_or_int {
    use super::*;

    pub fn serialize<S: Serializer>(
        v: &Option<bool>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match v {
            Some(b) => serializer.serialize_bool(*b),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<bool>, D::Error> {
        let opt = Option::<Value>::deserialize(deserializer)?;
        match opt {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(b)),
            Some(Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    Ok(Some(i != 0))
                } else if let Some(f) = n.as_f64() {
                    Ok(Some(f != 0.0))
                } else {
                    Err(serde::de::Error::custom("invalid number for bool"))
                }
            }
            Some(other) => Err(serde::de::Error::custom(format!(
                "expected bool or integer, got {other}"
            ))),
        }
    }
}
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// `index.json` inside a `.snagx` archive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SnagxIndex {
    #[serde(default)]
    pub pages: Vec<String>,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_version() -> String {
    "1.0".to_string()
}

impl Default for SnagxIndex {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            version: default_version(),
            extra: BTreeMap::new(),
        }
    }
}

/// `metadata.json` inside a `.snagx` archive.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SnagxMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_company: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_name: Option<String>,
    #[serde(rename = "WebURL", default, skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operating_system_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callout_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GradientBackground {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient_angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient_colors: Option<Vec<ColorRgba>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient_style: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SimplifySettings {
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_bool_or_int")]
    pub is_locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_bool_or_int")]
    pub show_original: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simplify_opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sui_detail: Option<f64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A single vector, text, callout, step, stamp, image, or drop-zone object on a Snagit page.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CaptureObject {
    #[serde(rename = "ObjectID", default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_mode: Option<ToolMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_point_str_serde"
    )]
    pub start_point: Option<PointF>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_point_str_serde"
    )]
    pub end_point: Option<PointF>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_vec_point_str_serde"
    )]
    pub points_array: Option<Vec<PointF>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_vec_point_str_serde"
    )]
    pub tail_points: Option<Vec<PointF>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_bool_or_int")]
    pub is_locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_bool_or_int")]
    pub is_flattened: Option<bool>,
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
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_bool_or_int")]
    pub drop_shadow_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_direction_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_direction_y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callout_shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(
        rename = "RTFEncodedText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub rtf_encoded_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_outline_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_outline_color: Option<ColorRgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_zone_group_id: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_point_str_serde"
    )]
    pub image_position: Option<PointF>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_point_str_serde"
    )]
    pub image_size: Option<PointF>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius_ratio: Option<f64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Template / Combine-Images section inside a `.snagx` or `.snagt` page.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Section {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_zone_group_id: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_point_str_serde"
    )]
    pub end_point: Option<PointF>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_bool_or_int")]
    pub is_locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_required_version: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_vec_point_str_serde"
    )]
    pub points_array: Option<Vec<PointF>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_graphics: Option<Vec<CaptureObject>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `{GUID}.json` page schema (also the exact schema of standalone `.snagt` template files).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SnagxPage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_canvas_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_canvas_height: Option<f64>,
    #[serde(rename = "DPI", default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_background_color: Option<ColorRgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_background_image: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_point_str_serde"
    )]
    pub capture_background_location: Option<PointF>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_point_str_serde"
    )]
    pub background_size: Option<PointF>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_color_space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient_background_color: Option<GradientBackground>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_tool_sequence_numeric: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_tool_sequence_uppercase_letter: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_tool_sequence_lowercase_letter: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simplify_settings: Option<SimplifySettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<Section>>,
    #[serde(default)]
    pub capture_objects: Vec<CaptureObject>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Alias for `.snagt` template files, which share the exact `SnagxPage` JSON schema.
pub type SnagTemplate = SnagxPage;

impl SnagxPage {
    pub fn from_template_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn to_template_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        fs::write(path, text)?;
        Ok(())
    }
}

/// A single page entry inside a `.snagx` archive, pairing `{GUID}.json` with its optional `{GUID}.png` and `{GUID}.backup.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct SnagxPageEntry {
    pub page_filename: String,
    pub page: SnagxPage,
    pub background_png: Option<Vec<u8>>,
    pub backup_json: Option<Vec<u8>>,
}

impl SnagxPageEntry {
    /// Returns the `{GUID}` stem without `.json`.
    pub fn stem(&self) -> &str {
        self.page_filename
            .strip_suffix(".json")
            .unwrap_or(&self.page_filename)
    }
}

/// Complete in-memory representation of a `.snagx` ZIP archive.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SnagxArchive {
    pub index: SnagxIndex,
    pub metadata: SnagxMetadata,
    pub thumbnail_png: Option<Vec<u8>>,
    pub pages: Vec<SnagxPageEntry>,
    /// Preserves any unrecognized entries inside the ZIP archive for lossless round-tripping.
    pub extra_entries: BTreeMap<String, Vec<u8>>,
}

impl SnagxArchive {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_reader(Cursor::new(bytes))
    }

    pub fn from_reader<R: Read + Seek>(reader: R) -> Result<Self> {
        let mut zip = ZipArchive::new(reader)?;
        let mut raw_entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();

        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            let name = file.name().to_string();
            let mut buf = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buf)?;
            raw_entries.insert(name, buf);
        }

        let index_bytes = raw_entries
            .remove("index.json")
            .ok_or_else(|| ModelError::MissingEntry("index.json".to_string()))?;
        let index: SnagxIndex = serde_json::from_slice(&index_bytes)?;

        let metadata = match raw_entries.remove("metadata.json") {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => SnagxMetadata::default(),
        };

        let thumbnail_png = raw_entries.remove("thumbnail.png");

        let mut pages = Vec::with_capacity(index.pages.len());
        for page_filename in &index.pages {
            let page_bytes = raw_entries
                .remove(page_filename)
                .ok_or_else(|| ModelError::MissingEntry(page_filename.clone()))?;
            let page: SnagxPage = serde_json::from_slice(&page_bytes)?;

            let stem = page_filename.strip_suffix(".json").unwrap_or(page_filename);
            let png_name = format!("{stem}.png");
            let backup_name = format!("{stem}.backup.json");

            let background_png = raw_entries.remove(&png_name);
            let backup_json = raw_entries.remove(&backup_name);

            pages.push(SnagxPageEntry {
                page_filename: page_filename.clone(),
                page,
                background_png,
                backup_json,
            });
        }

        Ok(Self {
            index,
            metadata,
            thumbnail_png,
            pages,
            extra_entries: raw_entries,
        })
    }

    /// Writes the archive using `CompressionMethod::Stored` (zero re-compression, matching Snagit 2022).
    pub fn write_to<W: Write + Seek>(&self, writer: W) -> Result<()> {
        let mut zip = ZipWriter::new(writer);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        // index.json
        zip.start_file("index.json", opts)?;
        let index_json = serde_json::to_vec_pretty(&self.index)?;
        zip.write_all(&index_json)?;

        // metadata.json
        zip.start_file("metadata.json", opts)?;
        let meta_json = serde_json::to_vec_pretty(&self.metadata)?;
        zip.write_all(&meta_json)?;

        // thumbnail.png
        if let Some(ref thumb) = self.thumbnail_png {
            zip.start_file("thumbnail.png", opts)?;
            zip.write_all(thumb)?;
        }

        // Pages
        for entry in &self.pages {
            zip.start_file(&entry.page_filename, opts)?;
            let page_json = serde_json::to_vec_pretty(&entry.page)?;
            zip.write_all(&page_json)?;

            let stem = entry.stem();
            if let Some(ref png) = entry.background_png {
                zip.start_file(format!("{stem}.png"), opts)?;
                zip.write_all(png)?;
            }
            if let Some(ref backup) = entry.backup_json {
                zip.start_file(format!("{stem}.backup.json"), opts)?;
                zip.write_all(backup)?;
            }
        }

        // Extra entries
        for (name, data) in &self.extra_entries {
            zip.start_file(name, opts)?;
            zip.write_all(data)?;
        }

        zip.finish()?;
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut cursor = Cursor::new(Vec::new());
        self.write_to(&mut cursor)?;
        Ok(cursor.into_inner())
    }

    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = self.to_bytes()?;
        fs::write(path, bytes)?;
        Ok(())
    }
}
