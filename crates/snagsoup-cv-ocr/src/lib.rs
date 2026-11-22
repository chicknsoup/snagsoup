//! SnagSoup CV & OCR: Windows.Media.Ocr (Grab Text & Edit Text),
//! Magic Wand (Contiguous & Global + Marching Squares), Auto-Simplify SUI,
//! Two-Tier Inpainting (UI Fast + Boundary Diffusion), and Smart Move.

pub mod edit_text;
pub mod error;
pub mod inpaint;
pub mod magic_wand;
pub mod neural_ocr;
pub mod ocr;
pub mod simplify_sui;
pub mod smart_move;
pub mod xliff;

pub use edit_text::edit_text_in_session;
pub use error::{CvOcrError, Result};
pub use inpaint::{inpaint_rect, inpaint_region, InpaintTierUsed};
pub use magic_wand::{
    extract_boundary_polylines, magic_wand_select, refine_mask, MagicWandMode, SelectionMask,
};
pub use neural_ocr::{is_neural_ocr_available, locate_neural_ocr_models};
pub use ocr::{
    available_ocr_languages, format_grab_text, grab_text, recognize_bitmap, recognize_rgba,
    recognize_rgba_with_progress, OcrDocument, OcrLine, OcrWord,
};
pub use simplify_sui::auto_simplify;
pub use smart_move::{detect_smart_objects, lift_smart_object, SmartElement};
pub use xliff::{export_snagx_to_xliff, import_xliff_to_snagx, TranslationIssue};

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_canvas::{
        render_text_box, DocumentSession, HorizontalAlign, PixelRect, TextRenderOptions,
        TiledBitmap, VerticalAlign,
    };
    use snagsoup_model::{ColorRgba, SnagPalette, SnagxArchive, SnagxPage, ToolMode};
    use std::path::Path;
    use tiny_skia::{Pixmap, Transform};

    #[test]
    fn ocr_grab_text_and_edit_text_on_rendered_image() {
        let langs = available_ocr_languages();
        if langs.is_empty() {
            // Gracefully skip if running on a headless Windows SKU without OCR packs
            return;
        }

        // Render "SnagSoup Rust 2026" in dark red on crisp white background
        let w = 480u32;
        let h = 100u32;
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::WHITE);
        let opts = TextRenderOptions {
            font_family: "Arial".to_string(),
            font_size: 32.0,
            color: ColorRgba::rgb(180, 20, 20),
            bold: true,
            h_align: HorizontalAlign::Center,
            v_align: VerticalAlign::Center,
            ..TextRenderOptions::default()
        };
        render_text_box(
            &mut pm,
            "SnagSoup Rust 2026",
            10.0,
            10.0,
            460.0,
            80.0,
            &opts,
            Transform::identity(),
        );
        let bmp = TiledBitmap::from_rgba(w, h, pm.data()).unwrap();

        let t0 = std::time::Instant::now();
        let grabbed = grab_text(&bmp, None).unwrap();
        let _elapsed = t0.elapsed();
        let lower = grabbed.to_ascii_lowercase();
        assert!(
            lower.contains("snagsoup") && lower.contains("2026"),
            "Unexpected OCR result: '{grabbed}'"
        );

        // Edit Text: erase text from white background and spawn editable ToolMode::Text object
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);
        let spawned = edit_text_in_session(&mut session, None).unwrap();
        assert!(!spawned.is_empty());
        assert_eq!(
            session.page.capture_objects[spawned[0]].tool_mode,
            Some(ToolMode::Text)
        );
        // Center pixel where text used to be should now be clean white background!
        let center_px = session.bitmap.get_pixel(240, 50).unwrap();
        assert!(
            center_px[0] > 240 && center_px[1] > 240 && center_px[2] > 240,
            "Expected inpainted white background, got {center_px:?}"
        );
    }

    #[test]
    fn neural_ocr_vietnamese_small_ui_dark_mode_and_url() {
        assert!(
            is_neural_ocr_available(),
            "Offline Neural OCR models (det.onnx + rec_latin.onnx) must be discovered and loaded"
        );

        // Build a realistic 680x220 UI screenshot combining:
        // Row 1 (13px small browser URL on white): "https://github.com/WildKernels/OnePlus"
        // Row 2 (16px Vietnamese stacked diacritics on white): "Xin chào Việt Nam - Chụp màn hình sắc nét"
        // Row 3 (13px white text inside a dark #111827 terminal/badge bar): "KernelSU Supported 4 days ago"
        let w = 680u32;
        let h = 220u32;
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::WHITE);

        render_text_box(
            &mut pm,
            "https://github.com/WildKernels/OnePlus",
            18.0,
            14.0,
            640.0,
            32.0,
            &TextRenderOptions {
                font_family: "Segoe UI".to_string(),
                font_size: 13.0,
                color: ColorRgba::rgb(31, 41, 55),
                h_align: HorizontalAlign::Left,
                v_align: VerticalAlign::Center,
                ..TextRenderOptions::default()
            },
            Transform::identity(),
        );

        render_text_box(
            &mut pm,
            "Xin chào Việt Nam - Chụp màn hình sắc nét",
            18.0,
            68.0,
            640.0,
            40.0,
            &TextRenderOptions {
                font_family: "Segoe UI".to_string(),
                font_size: 17.0,
                color: ColorRgba::rgb(17, 24, 39),
                h_align: HorizontalAlign::Left,
                v_align: VerticalAlign::Center,
                ..TextRenderOptions::default()
            },
            Transform::identity(),
        );

        let mut bmp = TiledBitmap::from_rgba(w, h, pm.data()).unwrap();
        // Paint dark badge background (#111827) on Row 3, then render white 13px UI text onto it
        let mut dark_pm = Pixmap::new(420, 42).unwrap();
        dark_pm.fill(tiny_skia::Color::from_rgba8(17, 24, 39, 255));
        render_text_box(
            &mut dark_pm,
            "KernelSU Supported 4 days ago",
            14.0,
            4.0,
            392.0,
            34.0,
            &TextRenderOptions {
                font_family: "Segoe UI".to_string(),
                font_size: 13.5,
                color: ColorRgba::WHITE,
                bold: true,
                h_align: HorizontalAlign::Left,
                v_align: VerticalAlign::Center,
                ..TextRenderOptions::default()
            },
            Transform::identity(),
        );
        let dark_data = dark_pm.data();
        bmp.mutate_region(PixelRect::new(18, 140, 420, 42), |x, y, px| {
            let lx = (x - 18) as usize;
            let ly = (y - 140) as usize;
            let off = (ly * 420 + lx) * 4;
            px.copy_from_slice(&dark_data[off..off + 4]);
        });

        let grabbed = grab_text(&bmp, None).expect("neural grab_text");
        println!("=== NEURAL OCR GRABBED ===\n{grabbed}\n==========================");

        assert!(
            grabbed.contains("github.com/WildKernels/OnePlus"),
            "Expected clean URL without extra spaces, got:\n{grabbed}"
        );
        assert!(
            grabbed.contains("Việt Nam") && grabbed.contains("màn hình") && grabbed.contains("sắc nét"),
            "Expected exact Vietnamese diacritics, got:\n{grabbed}"
        );
        assert!(
            grabbed.contains("KernelSU") && grabbed.contains("Supported") && grabbed.contains("4 days ago"),
            "Expected dark-mode badge text recognized cleanly, got:\n{grabbed}"
        );
    }

    #[test]
    fn magic_wand_contiguous_vs_global_and_marching_squares() {
        let mut bmp = TiledBitmap::new_filled(60, 40, ColorRgba::WHITE);
        // Two separate blue squares
        bmp.mutate_region(PixelRect::new(5, 5, 10, 10), |_, _, px| {
            *px = [0, 100, 255, 255];
        });
        bmp.mutate_region(PixelRect::new(35, 15, 10, 10), |_, _, px| {
            *px = [0, 100, 255, 255];
        });

        let contig = magic_wand_select(&bmp, 8, 8, 5.0, MagicWandMode::Contiguous);
        assert_eq!(contig.selected_count(), 100);
        assert_eq!(contig.bounds, Some(PixelRect::new(5, 5, 10, 10)));

        let mut global = magic_wand_select(&bmp, 8, 8, 5.0, MagicWandMode::Global);
        assert_eq!(global.selected_count(), 200);

        let polys = extract_boundary_polylines(&global);
        assert_eq!(polys.len(), 2, "Expected 2 closed boundary contours");

        refine_mask(&mut global);
        // 3x3 majority filter rounds the 4 sharp corners of each 10x10 square (200 - 8 = 192)
        assert_eq!(global.selected_count(), 192);
    }

    #[test]
    fn smart_move_detect_lift_and_linear_gradient_inpaint_accuracy() {
        // Horizontal linear gray gradient background [100..220] with a blue button in the middle
        let w = 120u32;
        let h = 80u32;
        let mut bmp = TiledBitmap::new_empty(w, h);
        bmp.mutate_region(PixelRect::new(0, 0, w, h), |x, _y, px| {
            let v = (100.0 + (x as f32 / (w - 1) as f32) * 120.0).round() as u8;
            *px = [v, v, v, 255];
        });
        let expected_bg = bmp.clone();

        // Draw a solid blue button at (40, 25, 40, 30)
        let btn_rect = PixelRect::new(40, 25, 40, 30);
        bmp.mutate_region(btn_rect, |_, _, px| {
            *px = [20, 100, 240, 255];
        });

        // 1. Detect smart objects
        let detected = detect_smart_objects(&bmp);
        assert!(!detected.is_empty(), "Expected SmartObjectDetector to find button");

        // 2. Lift button + verify gradient inpainting accuracy (< 2/255 mean error!)
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);
        let (obj_idx, tier) = lift_smart_object(&mut session, btn_rect).unwrap();
        assert_eq!(tier, Some(InpaintTierUsed::UiLinearGradientX));
        assert_eq!(
            session.page.capture_objects[obj_idx].tool_mode,
            Some(ToolMode::Image)
        );
        assert!(session.page.capture_objects[obj_idx].image.is_some());

        let mut err_sum = 0.0f64;
        let mut count = 0usize;
        for y in btn_rect.y..btn_rect.bottom() {
            for x in btn_rect.x..btn_rect.right() {
                let got = session.bitmap.get_pixel(x, y).unwrap();
                let exp = expected_bg.get_pixel(x, y).unwrap();
                for c in 0..3 {
                    err_sum += (got[c] as f64 - exp[c] as f64).abs();
                }
                count += 3;
            }
        }
        let mean_err = err_sum / count as f64;
        assert!(
            mean_err < 2.0,
            "Linear gradient inpaint mean error {mean_err:.3} exceeded 2/255"
        );
    }

    #[test]
    fn golden_auto_simplify_editor_overview_snagx() {
        let path = Path::new(r"C:\Program Files\TechSmith\Snagit 2022\en-US\EditorOverview.snagx");
        if !path.exists() {
            return;
        }
        let archive = SnagxArchive::from_file(path).unwrap();
        let bg_png = archive.pages[0].background_png.as_ref().unwrap();
        let bmp = TiledBitmap::from_png_bytes(bg_png).unwrap();

        let pal = SnagPalette {
            id: "test".into(),
            name: "Gray".into(),
            palette_colors: vec![
                ColorRgba::rgb(220, 220, 220),
                ColorRgba::rgb(160, 160, 160),
                ColorRgba::rgb(100, 100, 100),
                ColorRgba::rgb(40, 40, 40),
            ],
            extra: Default::default(),
        };

        let objects = auto_simplify(&bmp, Some(&pal), 6);
        assert!(!objects.is_empty(), "Expected Auto-Simplify SUI blocks");
        for obj in &objects {
            assert_eq!(obj.tool_mode, Some(ToolMode::Simplify));
            let s = obj.start_point.unwrap();
            let e = obj.end_point.unwrap();
            assert!(s.x >= 0.0 && e.x <= bmp.width() as f64);
            assert!(s.y >= 0.0 && e.y <= bmp.height() as f64);
        }
    }

    #[test]
    fn xliff_w2_15_roundtrip_and_overflow_detection() {
        use snagsoup_model::{
            decode_rtf_base64, encode_rtf_base64, extract_plain_text, generate_rtf, CaptureObject,
            PointF, RichTextSpan, RichTextStyle, SnagxIndex, SnagxMetadata, SnagxPageEntry,
        };

        let make_callout = |id: &str, text: &str, h: f64| {
            let rtf = generate_rtf(&[RichTextSpan {
                text: text.to_string(),
                style: RichTextStyle {
                    font_family: Some("Arial".to_string()),
                    font_size_pt: Some(18.0),
                    ..RichTextStyle::default()
                },
            }]);
            CaptureObject {
                object_id: Some(id.to_string()),
                tool_mode: Some(ToolMode::Callout),
                start_point: Some(PointF::new(10.0, 10.0)),
                end_point: Some(PointF::new(130.0, 10.0 + h)),
                font_size: Some(18.0),
                rtf_encoded_text: Some(encode_rtf_base64(&rtf)),
                ..CaptureObject::default()
            }
        };

        let archive = SnagxArchive {
            index: SnagxIndex {
                pages: vec!["{PAGE}.json".into()],
                version: "1.0".into(),
                extra: Default::default(),
            },
            metadata: SnagxMetadata::default(),
            thumbnail_png: None,
            pages: vec![SnagxPageEntry {
                page_filename: "{PAGE}.json".into(),
                page: SnagxPage {
                    capture_objects: vec![
                        make_callout("c1", "Save", 28.0),
                        make_callout("c2", "Click here to open settings", 36.0),
                        make_callout("c3", "Done", 28.0),
                    ],
                    ..SnagxPage::default()
                },
                background_png: None,
                backup_json: None,
            }],
            extra_entries: Default::default(),
        };

        let xml = export_snagx_to_xliff(&archive, "en-US", "vi-VN").unwrap();
        assert!(xml.contains("id=\"c1\"") && xml.contains("id=\"c2\"") && xml.contains("id=\"c3\""));

        // Replace empty targets with Vietnamese translations (c2 deliberately 2.5x longer -> overflow!)
        let translated_xml = xml
            .replacen(
                "<target state=\"needs-translation\"/>",
                "<target state=\"translated\">Lưu</target>",
                1,
            )
            .replacen(
                "<target state=\"needs-translation\"/>",
                "<target state=\"translated\">Nhấn vào nút này để mở toàn bộ bảng cài đặt nâng cao cho ứng dụng SnagSoup của bạn</target>",
                1,
            )
            .replacen(
                "<target state=\"needs-translation\"/>",
                "<target state=\"translated\">Xong</target>",
                1,
            );

        let (vi_archive, issues) = import_xliff_to_snagx(&archive, &translated_xml).unwrap();
        let c1_rtf = decode_rtf_base64(
            vi_archive.pages[0].page.capture_objects[0]
                .rtf_encoded_text
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(extract_plain_text(&c1_rtf), "Lưu");

        assert!(
            issues.iter().any(|i| matches!(i, TranslationIssue::TextOverflow { object_id, .. } if object_id == "c2")),
            "Expected TextOverflow warning for c2, got {issues:?}"
        );
    }

    #[test]
    fn ocr_progress_callback_reports_monotonic_stages_and_cancellation() {
        if available_ocr_languages().is_empty() {
            return;
        }
        let w = 360u32;
        let h = 90u32;
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::WHITE);
        render_text_box(
            &mut pm,
            "Progress Bar Verification",
            12.0,
            12.0,
            336.0,
            64.0,
            &TextRenderOptions {
                font_family: "Segoe UI".to_string(),
                font_size: 20.0,
                color: ColorRgba::rgb(17, 24, 39),
                bold: true,
                h_align: HorizontalAlign::Left,
                v_align: VerticalAlign::Center,
                ..TextRenderOptions::default()
            },
            Transform::identity(),
        );

        let mut updates: Vec<(f32, String)> = Vec::new();
        let doc = recognize_rgba_with_progress(w, h, pm.data(), None, |frac, stage| {
            updates.push((frac, stage.to_string()));
            true
        })
        .expect("recognize_rgba_with_progress");

        assert!(doc.text.to_ascii_lowercase().contains("progress"));
        assert!(updates.len() >= 2, "expected multiple progress stages, got {updates:?}");
        for pair in updates.windows(2) {
            assert!(
                pair[1].0 >= pair[0].0,
                "progress must be non-decreasing: {} -> {}",
                pair[0].0,
                pair[1].0
            );
        }
        assert!((updates.last().unwrap().0 - 1.0).abs() < 1e-5);

        // Returning false cancels immediately with CvOcrError::Cancelled
        let cancel_res = recognize_rgba_with_progress(w, h, pm.data(), None, |frac, _| frac < 0.25);
        assert!(matches!(cancel_res, Err(CvOcrError::Cancelled)));
    }
}
