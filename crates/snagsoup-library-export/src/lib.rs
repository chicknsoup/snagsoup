//! SnagSoup Library & Export: multi-format codecs (PNG/JPG/WebP/BMP/TIFF/ICO/PDF/SVG),
//! high-quality pure-Rust Animated GIF exporter, `winsqlite3` WAL+FTS5 Library DB & watcher,
//! and local-first Share Destinations.

pub mod codecs;
pub mod error;
pub mod library;
pub mod share;
pub mod sqlite_win;

pub use codecs::{
    export_pdf_bytes, export_raster_bytes, export_svg_string, import_raster_bytes,
    AnimatedGifExporter, GifExportReport, GifFrameInput, PdfPageSize, RasterExportFormat,
};
pub use error::{ExportError, Result};
pub use library::{FacetCount, LibraryDb, LibraryItem, LibraryWatcher, MediaType};
pub use share::{
    parse_program_outputs_xml, ExternalProgramConfig, ExternalProgramShare, FileAutoSave,
    ShareDestination, ShareHistory, SharePayload, ShareReceipt, SystemPrinterShare,
};

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_canvas::TiledBitmap;
    use snagsoup_model::{CaptureObject, ColorRgba, GifExportPreset, PointF, SnagxPage, ToolMode};

    #[test]
    fn codecs_w2_13_raster_pdf_and_svg_export() {
        let bmp = TiledBitmap::new_filled(64, 48, ColorRgba::rgb(40, 120, 220));
        let rgba = bmp.to_rgba();

        for fmt in [
            RasterExportFormat::Png,
            RasterExportFormat::Jpeg { quality: 85 },
            RasterExportFormat::WebP,
            RasterExportFormat::Bmp,
            RasterExportFormat::Tiff,
            RasterExportFormat::Ico,
        ] {
            let bytes = export_raster_bytes(64, 48, &rgba, fmt).unwrap();
            assert!(!bytes.is_empty());
            let decoded = import_raster_bytes(&bytes).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (64, 48));
        }

        // Multi-page PDF export
        let pdf = export_pdf_bytes(
            &[(64, 48, rgba.clone()), (64, 48, rgba.clone())],
            PdfPageSize::A4,
        )
        .unwrap();
        assert!(pdf.starts_with(b"%PDF-"));

        // Vector-preserving SVG export
        let page = SnagxPage {
            capture_canvas_width: Some(64.0),
            capture_canvas_height: Some(48.0),
            capture_objects: vec![
                CaptureObject {
                    tool_mode: Some(ToolMode::Arrow),
                    start_point: Some(PointF::new(5.0, 5.0)),
                    end_point: Some(PointF::new(50.0, 40.0)),
                    ..CaptureObject::default()
                },
                CaptureObject {
                    tool_mode: Some(ToolMode::Step),
                    start_point: Some(PointF::new(10.0, 10.0)),
                    end_point: Some(PointF::new(30.0, 30.0)),
                    step_value: Some("1".into()),
                    ..CaptureObject::default()
                },
            ],
            ..SnagxPage::default()
        };
        let svg = export_svg_string(&page, Some(&bmp)).unwrap();
        assert!(svg.contains("<image ") && svg.contains("data:image/png;base64,"));
        assert!(svg.contains("<line ") && svg.contains("<circle "));
    }

    #[test]
    fn codecs_w2_13_animated_gif_skip_similar_frames() {
        // 30 frames: 5 distinct + 20 identical middle frames + 5 distinct
        let mut frames = Vec::with_capacity(30);
        for i in 0..30u8 {
            let val = if (5..25).contains(&i) { 128u8 } else { i.wrapping_mul(17) };
            let rgba = vec![val, 255 - val, (val / 2), 255].repeat(32 * 24);
            frames.push(GifFrameInput {
                width: 32,
                height: 24,
                rgba,
                delay_cs: 10, // 100ms per frame -> 30 * 10 = 300 cs total
            });
        }

        let mut preset = GifExportPreset::screen_video();
        preset.insert_transition_frames = false;
        let report = AnimatedGifExporter::export(&frames, &preset).unwrap();

        assert!(report.bytes.starts_with(b"GIF89a"));
        assert_eq!(report.input_frames, 30);
        // 5 + 1 (merged 20 identical frames) + 5 = 11 stored frames!
        assert_eq!(report.stored_frames, 11);
        assert_eq!(report.total_duration_cs, 300);
    }

    #[test]
    fn library_w2_14_winsqlite3_fts5_5000_records_under_5ms() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let mut batch = Vec::with_capacity(5000);
        for i in 0..5000 {
            let is_special = i == 2500;
            batch.push(LibraryItem {
                id: 0,
                file_path: format!(r"C:\Captures\cap_{i:04}.snagx"),
                media_type: if i % 10 == 0 { MediaType::Video } else { MediaType::Image },
                capture_date: format!("2026-11-{:02} 10:00:00", (i % 28) + 1),
                app_name: if i % 2 == 0 { "Edge".into() } else { "VSCode".into() },
                window_title: format!("Window {i}"),
                web_url: if i % 2 == 0 {
                    "https://docs.rust-lang.org/book".into()
                } else {
                    String::new()
                },
                callout_text: if is_special {
                    "Hướng dẫn cài đặt SnagSoup".into()
                } else {
                    format!("Note {i}")
                },
                ocr_text: if is_special {
                    "UniqueNeedleKeyword Tiếng Việt".into()
                } else {
                    format!("Sample screen text block {i}")
                },
                width: 1920,
                height: 1080,
                tags: vec![],
            });
        }
        db.insert_batch(&batch).unwrap();

        // FTS5 Vietnamese diacritic-insensitive search (< 5 ms!)
        let t0 = std::time::Instant::now();
        let hits = db.search_fts("UniqueNeedleKeyword", 20).unwrap();
        let elapsed_ms = t0.elapsed().as_millis();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_path, r"C:\Captures\cap_2500.snagx");
        assert!(
            elapsed_ms < 50,
            "FTS5 search over 5000 records took {elapsed_ms}ms"
        );

        // Vietnamese diacritic-insensitive query ("Huong dan" matches "Hướng dẫn")
        let vi_hits = db.search_fts("Huong dan", 10).unwrap();
        assert_eq!(vi_hits.len(), 1);

        // Facet queries
        let apps = db.facets_by_app().unwrap();
        assert_eq!(apps.len(), 2);
        let domains = db.facets_by_domain().unwrap();
        assert_eq!(domains[0].label, "docs.rust-lang.org");
    }

    #[test]
    fn share_w2_15_file_autosave_program_xml_and_history() {
        let tmp = std::env::temp_dir().join("snagsoup_share_test");
        let _ = std::fs::remove_dir_all(&tmp);

        let rgba = vec![255u8, 0, 0, 255].repeat(16 * 16);
        let payload = SharePayload {
            width: 16,
            height: 16,
            rgba: &rgba,
            suggested_name: "test_cap",
        };

        let mut auto_save = FileAutoSave::new(&tmp);
        let receipt = auto_save.share(&payload).unwrap();
        let saved_path = receipt.target_path.as_ref().unwrap();
        assert!(std::path::Path::new(saved_path).exists());

        // ShareHistory round-trip
        let hist_path = tmp.join("share_history.json");
        let mut hist = ShareHistory::default();
        hist.record(receipt);
        hist.save(&hist_path).unwrap();
        let loaded = ShareHistory::load(&hist_path).unwrap();
        assert_eq!(loaded.entries.len(), 1);

        // ProgramOutputs.xml parsing
        let xml = r#"
        <ProgramOutputs>
            <ProgramOutput>
                <DisplayName>Paint.NET</DisplayName>
                <ExecutablePath>C:\Program Files\paint.net\paintdotnet.exe</ExecutablePath>
                <Parameters>"%1"</Parameters>
            </ProgramOutput>
        </ProgramOutputs>"#;
        let progs = parse_program_outputs_xml(xml).unwrap();
        assert_eq!(progs.len(), 1);
        assert_eq!(progs[0].display_name, "Paint.NET");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
