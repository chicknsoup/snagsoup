//! SnagSoup Data Model: geometry, colors, 21 tools, capture/export configs,
//! lightweight RTF codec, and 100%-compatible `.snagx`/`.snagt`/`.snagtheme`/`.snagpalette` I/O.

pub mod config;
pub mod error;
pub mod geometry;
pub mod rtf;
pub mod snagx;
pub mod tools;

pub use config::*;
pub use error::{ModelError, Result};
pub use geometry::{ColorRgba, PointF, RectF, SizeU};
pub use rtf::{
    decode_rtf_base64, encode_rtf_base64, extract_plain_text, generate_rtf, parse_rtf_spans,
    RichTextSpan, RichTextStyle,
};
pub use snagx::{
    CaptureObject, GradientBackground, Section, SimplifySettings, SnagTemplate, SnagxArchive,
    SnagxIndex, SnagxMetadata, SnagxPage, SnagxPageEntry,
};
pub use tools::{FavoritesFile, QuickStyle, SnagPalette, SnagTheme, ToolMode, ToolProperties};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn color_rgba_parse_and_format() {
        let c = ColorRgba::from_hex("#FFFC4242").unwrap();
        assert_eq!(c, ColorRgba::new(0xFC, 0x42, 0x42, 0xFF));
        assert_eq!(c.to_argb_hex(), "#FFFC4242");

        let c_rgb = ColorRgba::from_hex("#123456").unwrap();
        assert_eq!(c_rgb, ColorRgba::new(0x12, 0x34, 0x56, 0xFF));
        assert_eq!(c_rgb.to_argb_hex(), "#FF123456");

        assert!(ColorRgba::from_hex("#123").is_err());
    }

    #[test]
    fn point_snagit_string_roundtrip() {
        let p = PointF::from_snagit_str("346.854889,310.654449").unwrap();
        assert!((p.x - 346.854889).abs() < 1e-6);
        assert!((p.y - 310.654449).abs() < 1e-6);
        assert_eq!(p.to_snagit_string(), "346.854889,310.654449");
    }

    #[test]
    fn rtf_generate_and_parse_roundtrip() {
        let spans = vec![
            RichTextSpan {
                text: "Hello ".to_string(),
                style: RichTextStyle {
                    font_family: Some("Trebuchet MS".to_string()),
                    font_size_pt: Some(24.0),
                    color: Some(ColorRgba::rgb(255, 0, 0)),
                    bold: true,
                    italic: false,
                    underline: false,
                },
            },
            RichTextSpan {
                text: "SnagSoup!\nLine 2".to_string(),
                style: RichTextStyle {
                    font_family: Some("Trebuchet MS".to_string()),
                    font_size_pt: Some(18.0),
                    color: Some(ColorRgba::rgb(0, 128, 255)),
                    bold: false,
                    italic: true,
                    underline: true,
                },
            },
        ];

        let rtf = generate_rtf(&spans);
        let b64 = encode_rtf_base64(&rtf);
        let decoded = decode_rtf_base64(&b64).unwrap();
        assert_eq!(extract_plain_text(&decoded), "Hello SnagSoup!\nLine 2");

        let parsed = parse_rtf_spans(&decoded);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].style.bold);
        assert_eq!(parsed[0].style.font_size_pt, Some(24.0));
        assert_eq!(parsed[0].style.color, Some(ColorRgba::rgb(255, 0, 0)));
        assert!(parsed[1].style.italic);
        assert!(parsed[1].style.underline);
    }

    #[test]
    fn snagx_in_memory_roundtrip() {
        let page = SnagxPage {
            capture_canvas_width: Some(1920.0),
            capture_canvas_height: Some(1080.0),
            dpi: Some(96.0),
            capture_background_color: Some(ColorRgba::WHITE),
            capture_objects: vec![CaptureObject {
                object_id: Some("obj-1".to_string()),
                tool_mode: Some(ToolMode::Arrow),
                start_point: Some(PointF::new(10.0, 20.0)),
                end_point: Some(PointF::new(300.5, 400.25)),
                foreground_color: Some(ColorRgba::from_hex("#FFFC4242").unwrap()),
                stroke_width: Some(6.0),
                ..CaptureObject::default()
            }],
            ..SnagxPage::default()
        };

        let archive = SnagxArchive {
            index: SnagxIndex {
                pages: vec!["{TEST-GUID}.json".to_string()],
                version: "1.0".to_string(),
                extra: Default::default(),
            },
            metadata: SnagxMetadata {
                app_name: Some("SnagSoup".to_string()),
                tags: Some(vec!["rust".to_string(), "test".to_string()]),
                ..SnagxMetadata::default()
            },
            thumbnail_png: Some(vec![137, 80, 78, 71]),
            pages: vec![SnagxPageEntry {
                page_filename: "{TEST-GUID}.json".to_string(),
                page,
                background_png: Some(vec![137, 80, 78, 71, 13, 10, 26, 10]),
                backup_json: None,
            }],
            extra_entries: Default::default(),
        };

        let bytes = archive.to_bytes().unwrap();
        let reloaded = SnagxArchive::from_bytes(&bytes).unwrap();
        assert_eq!(reloaded, archive);
    }

    #[test]
    fn golden_parse_snagit_favorites_json() {
        let path = Path::new(r"C:\Program Files\TechSmith\Snagit 2022\Favorites.json");
        if !path.exists() {
            return;
        }
        let fav = FavoritesFile::from_file(path).unwrap();
        assert!(!fav.quick_styles.is_empty());
        assert_eq!(fav.quick_styles[0].tool_mode, Some(ToolMode::Arrow));
    }

    #[test]
    fn golden_parse_snagit_urban_theme() {
        let path = Path::new(r"C:\Program Files\TechSmith\Snagit 2022\Themes\Urban.snagtheme");
        if !path.exists() {
            return;
        }
        let theme = SnagTheme::from_file(path).unwrap();
        assert!(!theme.name.is_empty());
        assert!(!theme.quick_styles.is_empty());
    }

    #[test]
    fn golden_parse_snagit_dark_grayscale_palette() {
        let path = Path::new(
            r"C:\Program Files\TechSmith\Snagit 2022\Palettes\Dark Grayscale.snagpalette",
        );
        if !path.exists() {
            return;
        }
        let palette = SnagPalette::from_file(path).unwrap();
        assert_eq!(palette.name, "Dark Grayscale");
        assert_eq!(palette.palette_colors.len(), 4);
    }

    #[test]
    fn golden_roundtrip_editor_overview_snagx() {
        let path = Path::new(r"C:\Program Files\TechSmith\Snagit 2022\en-US\EditorOverview.snagx");
        if !path.exists() {
            return;
        }
        let archive = SnagxArchive::from_file(path).unwrap();
        assert_eq!(archive.pages.len(), 1);
        assert!(archive.thumbnail_png.is_some());
        assert!(archive.pages[0].background_png.is_some());

        let bytes = archive.to_bytes().unwrap();
        let reloaded = SnagxArchive::from_bytes(&bytes).unwrap();
        assert_eq!(
            reloaded.pages[0].page.capture_objects.len(),
            archive.pages[0].page.capture_objects.len()
        );
        assert_eq!(reloaded.metadata, archive.metadata);
    }

    #[test]
    fn golden_roundtrip_template_onboarding_snagx() {
        let path =
            Path::new(r"C:\Program Files\TechSmith\Snagit 2022\en-US\TemplateOnboarding.snagx");
        if !path.exists() {
            return;
        }
        let archive = SnagxArchive::from_file(path).unwrap();
        assert_eq!(archive.pages.len(), 1);
        assert!(archive.thumbnail_png.is_some());

        // Verify RTF extraction works on real Snagit RTFEncodedText objects if present
        let mut rtf_count = 0;
        for obj in &archive.pages[0].page.capture_objects {
            if let Some(ref b64) = obj.rtf_encoded_text {
                let rtf = decode_rtf_base64(b64).unwrap();
                let _plain = extract_plain_text(&rtf);
                rtf_count += 1;
            }
        }
        if let Some(ref sections) = archive.pages[0].page.sections {
            for sec in sections {
                if let Some(ref graphics) = sec.section_graphics {
                    for obj in graphics {
                        if let Some(ref b64) = obj.rtf_encoded_text {
                            let rtf = decode_rtf_base64(b64).unwrap();
                            let _plain = extract_plain_text(&rtf);
                            rtf_count += 1;
                        }
                    }
                }
            }
        }
        assert!(rtf_count > 0, "Expected RTFEncodedText in TemplateOnboarding.snagx");

        let bytes = archive.to_bytes().unwrap();
        let reloaded = SnagxArchive::from_bytes(&bytes).unwrap();
        assert_eq!(
            reloaded.pages[0].page.capture_objects.len(),
            archive.pages[0].page.capture_objects.len()
        );
    }

    #[test]
    fn golden_parse_meme_1_snagt() {
        let path = Path::new(r"C:\ProgramData\TechSmith\Snagit 22\Templates\Meme_1.snagt");
        if !path.exists() {
            return;
        }
        let tmpl = SnagTemplate::from_template_file(path).unwrap();
        assert_eq!(tmpl.capture_canvas_width, Some(500.0));
        assert_eq!(tmpl.capture_canvas_height, Some(500.0));
        assert!(tmpl.sections.as_ref().map_or(false, |s| !s.is_empty()));
    }
}
