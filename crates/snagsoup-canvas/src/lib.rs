//! SnagSoup Canvas: Copy-on-Write 256x256 tiled bitmap, undo/redo command history,
//! dirty-region GPU upload tracking, vector tools (Group 1 & 2), raster tools, and scene compositor.

pub mod compositor;
pub mod effects_color;
pub mod effects_geom;
pub mod error;
pub mod history;
pub mod raster_tools;
pub mod templates;
pub mod text_render;
pub mod tile_bitmap;
pub mod vector_g1;
pub mod vector_g2;

pub use compositor::{downscale_rgba_bilinear, SceneCompositor};
pub use effects_color::{
    apply_color_adjustment, apply_color_replacement, apply_filter, apply_spotlight_and_magnify,
};
pub use effects_geom::{
    apply_border, apply_caption, apply_edges, apply_effect, apply_effects, apply_flip,
    apply_page_curl, apply_perspective, apply_rotate, apply_scale, apply_shadow, apply_trim,
    apply_watermark, preview_effect_on_rgba,
};
pub use error::{CanvasError, Result};
pub use history::{CommandHistory, DirtyRegion, DocumentSession, HistoryCommand};
pub use raster_tools::{
    apply_box_blur_3pass_region, apply_pixelate_region, auto_trim_bounds, crop_document,
    cut_out_strip, eraser_stroke, flood_fill, CutOutOrientation,
};
pub use templates::{
    combine_images, CombineLayout, CombineOptions, DropZoneFitMode, TemplateEngine,
};
pub use text_render::{render_text_box, HorizontalAlign, TextRenderOptions, VerticalAlign};
pub use tile_bitmap::{encode_rgba_to_png, PixelRect, TileCoord, TiledBitmap, TILE_BYTES, TILE_SIZE};
pub use vector_g1::{hit_test_object, render_arrow, render_shape, render_step, render_stroke_tool};
pub use vector_g2::{
    build_unified_callout_path, draw_svg_stamp, render_callout, render_svg_stamp_to_pixmap,
    render_svg_to_rgba, render_text_object,
};

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_model::{
        encode_rtf_base64, generate_rtf, CaptureObject, ColorRgba, PointF, RectF, RichTextSpan,
        RichTextStyle, SnagxArchive, SnagxPage, ToolMode,
    };
    use std::path::Path;

    #[test]
    fn cow_4k_30_edits_memory_budget_and_undo_redo_exact() {
        let initial = TiledBitmap::new_filled(3840, 2160, ColorRgba::rgb(240, 240, 240));
        let mut session = DocumentSession::new(SnagxPage::default(), initial.clone());
        let _ = session.take_dirty();

        for step in 1u8..=30u8 {
            let rect = PixelRect::new(256, 256, 128, 128);
            let modified = session.mutate_raster_region(rect, |_x, _y, px| {
                px[0] = step.wrapping_mul(7);
                px[1] = step.wrapping_mul(3);
                px[2] = 200u8.saturating_sub(step);
                px[3] = 255;
            });
            assert_eq!(modified, vec![TileCoord::new(1, 1)]);
        }

        let inc_bytes = session
            .history
            .incremental_tile_bytes_beyond(&initial, &session.bitmap);
        assert!(
            inc_bytes < 10 * 1024 * 1024,
            "Incremental CoW memory {} bytes exceeded 10 MB budget",
            inc_bytes
        );

        let final_snapshot = session.bitmap.clone();
        assert_eq!(session.history.undo_len(), 30);

        for _ in 0..30 {
            assert!(session.undo());
        }
        assert!(!session.undo());
        assert_eq!(session.bitmap, initial);

        for _ in 0..30 {
            assert!(session.redo());
        }
        assert!(!session.redo());
        assert_eq!(session.bitmap, final_snapshot);
    }

    #[test]
    fn vector_history_and_dirty_region_tracking() {
        let bmp = TiledBitmap::new_empty(640, 480);
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);
        let initial_dirty = session.take_dirty();
        assert!(initial_dirty.is_dirty());
        assert!(!session.dirty.is_dirty());

        let obj1 = CaptureObject {
            object_id: Some("a1".to_string()),
            tool_mode: Some(ToolMode::Arrow),
            start_point: Some(PointF::new(10.0, 10.0)),
            end_point: Some(PointF::new(100.0, 80.0)),
            ..CaptureObject::default()
        };
        let idx = session.add_object(obj1.clone());
        assert_eq!(idx, 0);
        assert_eq!(session.page.capture_objects.len(), 1);
        assert!(session.take_dirty().is_dirty());

        let mut obj1_moved = obj1.clone();
        obj1_moved.end_point = Some(PointF::new(200.0, 150.0));
        assert!(session.update_object(0, obj1_moved.clone()));
        assert_eq!(
            session.page.capture_objects[0].end_point,
            Some(PointF::new(200.0, 150.0))
        );

        assert!(session.undo());
        assert_eq!(
            session.page.capture_objects[0].end_point,
            Some(PointF::new(100.0, 80.0))
        );
        assert!(session.undo());
        assert!(session.page.capture_objects.is_empty());

        assert!(session.redo());
        assert!(session.redo());
        assert_eq!(
            session.page.capture_objects[0].end_point,
            Some(PointF::new(200.0, 150.0))
        );
    }

    #[test]
    fn tiled_bitmap_png_crop_and_resize_roundtrip() {
        let mut bmp = TiledBitmap::new_filled(300, 200, ColorRgba::rgb(10, 20, 30));
        bmp.mutate_region(PixelRect::new(50, 40, 20, 20), |_x, _y, px| {
            *px = [255, 0, 128, 255];
        });

        let png_bytes = bmp.to_png_bytes().unwrap();
        let decoded = TiledBitmap::from_png_bytes(&png_bytes).unwrap();
        assert_eq!(decoded, bmp);

        let cropped = bmp.crop(PixelRect::new(50, 40, 20, 20));
        assert_eq!(cropped.width(), 20);
        assert_eq!(cropped.height(), 20);
        assert_eq!(cropped.get_pixel(0, 0), Some([255, 0, 128, 255]));

        let resized = cropped.resize(40, 40, 10, 10, ColorRgba::WHITE);
        assert_eq!(resized.get_pixel(0, 0), Some([255, 255, 255, 255]));
        assert_eq!(resized.get_pixel(10, 10), Some([255, 0, 128, 255]));
    }

    #[test]
    fn compositor_viewport_and_thumbnail() {
        let bmp = TiledBitmap::new_filled(400, 300, ColorRgba::rgb(255, 255, 255));
        let page = SnagxPage {
            capture_canvas_width: Some(400.0),
            capture_canvas_height: Some(300.0),
            capture_background_color: Some(ColorRgba::WHITE),
            capture_objects: vec![CaptureObject {
                tool_mode: Some(ToolMode::Shape),
                start_point: Some(PointF::new(50.0, 50.0)),
                end_point: Some(PointF::new(150.0, 150.0)),
                background_color: Some(ColorRgba::rgb(255, 0, 0)),
                foreground_color: Some(ColorRgba::rgb(0, 0, 0)),
                stroke_width: Some(4.0),
                opacity: Some(100.0),
                ..CaptureObject::default()
            }],
            ..SnagxPage::default()
        };

        let comp = SceneCompositor::new(&page, Some(&bmp));
        let (w, h, rgba) = comp.flatten_to_rgba().unwrap();
        assert_eq!((w, h), (400, 300));
        let idx = ((100 * w + 100) as usize) * 4;
        assert_eq!(&rgba[idx..idx + 4], &[255, 0, 0, 255]);

        let (vw, vh, _vrgba) = comp
            .render_viewport(RectF::new(50.0, 50.0, 100.0, 100.0), 2.0)
            .unwrap();
        assert_eq!((vw, vh), (200, 200));

        let thumb_png = comp.generate_thumbnail_png(128, 128).unwrap();
        let thumb_bmp = TiledBitmap::from_png_bytes(&thumb_png).unwrap();
        assert!(thumb_bmp.width() <= 128 && thumb_bmp.height() <= 128);
    }

    #[test]
    fn vector_g1_curved_arrow_hit_test_and_render() {
        // Quadratic Bezier curved arrow from (20, 100) via control (100, 10) to (180, 100)
        // Midpoint at t=0.5 is (100, 55)
        let arrow = CaptureObject {
            tool_mode: Some(ToolMode::Arrow),
            points_array: Some(vec![
                PointF::new(20.0, 100.0),
                PointF::new(100.0, 10.0),
                PointF::new(180.0, 100.0),
            ]),
            stroke_width: Some(6.0),
            foreground_color: Some(ColorRgba::rgb(255, 0, 0)),
            drop_shadow_enabled: Some(true),
            opacity: Some(100.0),
            ..CaptureObject::default()
        };

        // Hit on curve midpoint (100, 55), miss in empty space below (100, 100)
        assert!(hit_test_object(&arrow, PointF::new(100.0, 55.0), 4.0));
        assert!(!hit_test_object(&arrow, PointF::new(100.0, 100.0), 4.0));

        let page = SnagxPage {
            capture_canvas_width: Some(220.0),
            capture_canvas_height: Some(140.0),
            capture_background_color: Some(ColorRgba::WHITE),
            capture_objects: vec![
                arrow,
                CaptureObject {
                    tool_mode: Some(ToolMode::Step),
                    start_point: Some(PointF::new(10.0, 10.0)),
                    end_point: Some(PointF::new(42.0, 42.0)),
                    step_style: Some("Circle-Pointer".to_string()),
                    step_value: Some("1".to_string()),
                    background_color: Some(ColorRgba::rgb(0, 120, 215)),
                    opacity: Some(100.0),
                    ..CaptureObject::default()
                },
            ],
            ..SnagxPage::default()
        };
        let comp = SceneCompositor::new(&page, None);
        let (w, h, rgba) = comp.flatten_to_rgba().unwrap();
        assert_eq!((w, h), (220, 140));
        // Curve midpoint (100, 55) should be red
        let idx = ((55 * w + 100) as usize) * 4;
        assert!(rgba[idx] > 200 && rgba[idx + 1] < 50);
    }

    #[test]
    fn vector_g2_callout_text_outline_and_svg_stamp() {
        let rtf = generate_rtf(&[RichTextSpan {
            text: "SnagSoup".to_string(),
            style: RichTextStyle {
                font_family: Some("Arial".to_string()),
                font_size_pt: Some(22.0),
                color: Some(ColorRgba::WHITE),
                bold: true,
                italic: false,
                underline: false,
            },
        }]);
        let callout = CaptureObject {
            tool_mode: Some(ToolMode::Callout),
            callout_shape: Some("CTRoundedRectWithArrow".to_string()),
            start_point: Some(PointF::new(30.0, 20.0)),
            end_point: Some(PointF::new(230.0, 100.0)),
            tail_points: Some(vec![PointF::new(130.0, 140.0)]),
            background_color: Some(ColorRgba::rgb(244, 189, 33)),
            foreground_color: Some(ColorRgba::rgb(40, 40, 40)),
            stroke_width: Some(3.0),
            text_outline_width: Some(2.0),
            text_outline_color: Some(ColorRgba::BLACK),
            rtf_encoded_text: Some(encode_rtf_base64(&rtf)),
            opacity: Some(100.0),
            ..CaptureObject::default()
        };

        let page = SnagxPage {
            capture_canvas_width: Some(260.0),
            capture_canvas_height: Some(160.0),
            capture_background_color: Some(ColorRgba::WHITE),
            capture_objects: vec![callout],
            ..SnagxPage::default()
        };
        let comp = SceneCompositor::new(&page, None);
        let (w, h, rgba) = comp.flatten_to_rgba().unwrap();
        assert_eq!((w, h), (260, 160));
        // Tail interior pixel around (130, 118) should be yellow fill, not white
        let tail_idx = ((118 * w + 130) as usize) * 4;
        assert!(rgba[tail_idx] > 200 && rgba[tail_idx + 2] < 80);

        // SVG Stamp rendering via resvg
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><circle cx="32" cy="32" r="28" fill="rgb(0,200,0)"/></svg>"#;
        let pm = render_svg_stamp_to_pixmap(svg, 64, 64).expect("SVG stamp rendered");
        let center_idx = ((32 * 64 + 32) as usize) * 4;
        assert!(pm.data()[center_idx + 1] > 180);
    }

    #[test]
    fn raster_tools_crop_shifts_annotations_exact() {
        let bmp = TiledBitmap::new_filled(800, 600, ColorRgba::WHITE);
        let arrow = CaptureObject {
            object_id: Some("ptr".to_string()),
            tool_mode: Some(ToolMode::Arrow),
            start_point: Some(PointF::new(200.0, 150.0)),
            end_point: Some(PointF::new(500.0, 400.0)),
            ..CaptureObject::default()
        };
        let mut session = DocumentSession::new(
            SnagxPage {
                capture_canvas_width: Some(800.0),
                capture_canvas_height: Some(600.0),
                capture_objects: vec![arrow],
                ..SnagxPage::default()
            },
            bmp,
        );

        // Crop off (100, 50) from top-left
        assert!(crop_document(&mut session, PixelRect::new(100, 50, 500, 400)));
        assert_eq!(session.bitmap.width(), 500);
        assert_eq!(session.bitmap.height(), 400);
        assert_eq!(
            session.page.capture_objects[0].start_point,
            Some(PointF::new(100.0, 100.0))
        );
        assert_eq!(
            session.page.capture_objects[0].end_point,
            Some(PointF::new(400.0, 350.0))
        );

        // Undo must restore original dimensions and arrow coordinates
        assert!(session.undo());
        assert_eq!(session.bitmap.width(), 800);
        assert_eq!(
            session.page.capture_objects[0].end_point,
            Some(PointF::new(500.0, 400.0))
        );
    }

    #[test]
    fn raster_tools_eraser_flood_fill_blur_pixelate_cutout() {
        // 1. Eraser + Flood Fill
        let mut bmp = TiledBitmap::new_filled(100, 100, ColorRgba::WHITE);
        bmp.mutate_region(PixelRect::new(20, 20, 60, 60), |_x, _y, px| {
            *px = [0, 0, 255, 255];
        });
        let dirty = flood_fill(&mut bmp, 50, 50, ColorRgba::rgb(0, 255, 0), 5.0, false);
        assert!(dirty.is_some());
        assert_eq!(bmp.get_pixel(50, 50), Some([0, 255, 0, 255]));
        assert_eq!(bmp.get_pixel(5, 5), Some([255, 255, 255, 255]));

        eraser_stroke(
            &mut bmp,
            &[PointF::new(30.0, 50.0), PointF::new(70.0, 50.0)],
            4.0,
            ColorRgba::TRANSPARENT,
        );
        assert_eq!(bmp.get_pixel(50, 50), Some([0, 0, 0, 0]));

        // 2. Cut Out removes a 20px horizontal strip
        let cut = cut_out_strip(&bmp, CutOutOrientation::Horizontal, 40, 60, 12, 42);
        assert_eq!(cut.width(), 100);
        assert_eq!(cut.height(), 80);

        // 3. Auto-Trim
        let mut trim_bmp = TiledBitmap::new_filled(100, 100, ColorRgba::WHITE);
        trim_bmp.mutate_region(PixelRect::new(15, 25, 30, 40), |_x, _y, px| {
            *px = [10, 10, 10, 255];
        });
        assert_eq!(
            auto_trim_bounds(&trim_bmp, 2),
            Some(PixelRect::new(15, 25, 30, 40))
        );

        // 4. Non-destructive Blur & Pixelate through SceneCompositor
        let mut checker = TiledBitmap::new_filled(80, 80, ColorRgba::WHITE);
        checker.mutate_region(PixelRect::new(0, 0, 40, 80), |_x, _y, px| {
            *px = [0, 0, 0, 255];
        });
        let mut blur_obj = CaptureObject {
            tool_mode: Some(ToolMode::Blur),
            start_point: Some(PointF::new(20.0, 20.0)),
            end_point: Some(PointF::new(60.0, 60.0)),
            ..CaptureObject::default()
        };
        blur_obj
            .extra
            .insert("BlurIntensity".to_string(), serde_json::json!(20.0));
        let page = SnagxPage {
            capture_canvas_width: Some(80.0),
            capture_canvas_height: Some(80.0),
            capture_objects: vec![blur_obj],
            ..SnagxPage::default()
        };
        let comp = SceneCompositor::new(&page, Some(&checker));
        let (w, _h, rgba) = comp.flatten_to_rgba().unwrap();
        // Boundary pixel (40, 40) was pure white (255) before blur; after blur it should be gray!
        let mid_r = rgba[((40 * w + 40) as usize) * 4];
        assert!(mid_r > 40 && mid_r < 220, "Expected blurred gray, got {mid_r}");
    }

    #[test]
    fn golden_composite_editor_overview_snagx() {
        let path = Path::new(r"C:\Program Files\TechSmith\Snagit 2022\en-US\EditorOverview.snagx");
        if !path.exists() {
            return;
        }
        let archive = SnagxArchive::from_file(path).unwrap();
        let page_entry = &archive.pages[0];
        let bg_png = page_entry
            .background_png
            .as_ref()
            .expect("EditorOverview has background PNG");
        let bmp = TiledBitmap::from_png_bytes(bg_png).unwrap();
        assert!(bmp.width() > 0 && bmp.height() > 0);

        let comp = SceneCompositor::new(&page_entry.page, Some(&bmp));
        let (fw, fh, rgba) = comp.flatten_to_rgba().unwrap();
        assert_eq!(rgba.len(), (fw as usize) * (fh as usize) * 4);

        let thumb_png = comp.generate_thumbnail_png(256, 256).unwrap();
        let thumb_bmp = TiledBitmap::from_png_bytes(&thumb_png).unwrap();
        assert!(thumb_bmp.width() > 0 && thumb_bmp.width() <= 256);
        assert!(thumb_bmp.height() > 0 && thumb_bmp.height() <= 256);
    }

    #[test]
    fn golden_composite_template_onboarding_snagx() {
        let path =
            Path::new(r"C:\Program Files\TechSmith\Snagit 2022\en-US\TemplateOnboarding.snagx");
        if !path.exists() {
            return;
        }
        let archive = SnagxArchive::from_file(path).unwrap();
        let page_entry = &archive.pages[0];
        let bmp = page_entry
            .background_png
            .as_ref()
            .and_then(|b| TiledBitmap::from_png_bytes(b).ok());

        let comp = SceneCompositor::new(&page_entry.page, bmp.as_ref());
        let (fw, fh, rgba) = comp.flatten_to_rgba().unwrap();
        assert!(fw > 0 && fh > 0);
        assert_eq!(rgba.len(), (fw as usize) * (fh as usize) * 4);

        let thumb_png = comp.generate_thumbnail_png(256, 256).unwrap();
        let thumb_bmp = TiledBitmap::from_png_bytes(&thumb_png).unwrap();
        assert!(thumb_bmp.width() > 0 && thumb_bmp.width() <= 256);
    }

    #[test]
    fn effects_w2_08_color_filters_and_4k_lut_speed() {
        use snagsoup_model::EffectConfig;

        // 1. Filters: Grayscale, Sepia, Invert, Monochrome, Sharpen
        let base = vec![200, 100, 50, 255].repeat(16); // 4x4
        let (_, _, gray) = apply_effect(
            4,
            4,
            base.clone(),
            &EffectConfig::Filters {
                kind: "Grayscale".into(),
                intensity: 100.0,
            },
        );
        assert_eq!(gray[0], gray[1]);
        assert_eq!(gray[1], gray[2]);

        let (_, _, inv) = apply_effect(
            4,
            4,
            base.clone(),
            &EffectConfig::Filters {
                kind: "Invert".into(),
                intensity: 100.0,
            },
        );
        assert_eq!(&inv[0..4], &[55, 155, 205, 255]);

        let (_, _, sep) = apply_effect(
            4,
            4,
            base.clone(),
            &EffectConfig::Filters {
                kind: "Sepia".into(),
                intensity: 100.0,
            },
        );
        assert!(sep[0] >= sep[1] && sep[1] >= sep[2]);

        let (_, _, mono) = apply_effect(
            4,
            4,
            base.clone(),
            &EffectConfig::Filters {
                kind: "Monochrome".into(),
                intensity: 50.0,
            },
        );
        assert!(mono[0] == 0 || mono[0] == 255);

        let (_, _, _) = apply_effect(
            4,
            4,
            base.clone(),
            &EffectConfig::Filters {
                kind: "Sharpen".into(),
                intensity: 80.0,
            },
        );

        // 2. Color Replacement preserving luminance
        let (_, _, rep) = apply_effect(
            4,
            4,
            base.clone(),
            &EffectConfig::ColorReplacement {
                source: ColorRgba::rgb(200, 100, 50),
                target: ColorRgba::rgb(50, 100, 200),
                tolerance: 10.0,
                preserve_luminance: true,
            },
        );
        assert!(rep[2] > rep[0], "Expected blue-dominant replacement, got {:?}", &rep[0..4]);

        // 3. Spotlight & Magnify
        let spot_src = vec![200u8, 200, 200, 255].repeat(20 * 20);
        let (_, _, spot) = apply_effect(
            20,
            20,
            spot_src,
            &EffectConfig::SpotlightAndMagnify {
                x: 5.0,
                y: 5.0,
                width: 10.0,
                height: 10.0,
                dim_opacity: 0.5,
                blur_radius: 1,
                magnify_scale: 1.5,
            },
        );
        // Outside (0,0) dimmed ~100; inside (10,10) bright 200
        assert!(spot[0] < 130);
        assert!(spot[(10 * 20 + 10) * 4] >= 190);

        // 4. 4K (3840x2160) LUT ColorAdjustment speed check
        let mut buf_4k = vec![128u8; 3840 * 2160 * 4];
        let t0 = std::time::Instant::now();
        apply_color_adjustment(&mut buf_4k, 15.0, 20.0, 0.0, 0.0, 1.1);
        let elapsed_ms = t0.elapsed().as_millis();
        // Even in dev mode (opt-level=0 for workspace crate), rayon LUT should be very fast
        assert!(elapsed_ms < 500, "4K LUT ColorAdjustment unexpectedly slow: {elapsed_ms}ms");
    }

    #[test]
    fn effects_w2_09_all_geom_effects_and_edge_sizes() {
        use snagsoup_model::EffectConfig;

        let effects = vec![
            EffectConfig::Border {
                width: 4,
                color: ColorRgba::rgb(255, 0, 0),
                inside: false,
            },
            EffectConfig::Shadow {
                offset_x: 4,
                offset_y: 4,
                blur_radius: 3,
                opacity: 0.6,
                color: ColorRgba::BLACK,
            },
            EffectConfig::Edges {
                style: "Torn".into(),
                size: 6,
                edges_mask: 0x0F,
                seed: 7,
            },
            EffectConfig::Edges {
                style: "Wave".into(),
                size: 4,
                edges_mask: 1 | 4,
                seed: 1,
            },
            EffectConfig::Edges {
                style: "Sawtooth".into(),
                size: 4,
                edges_mask: 2 | 8,
                seed: 1,
            },
            EffectConfig::Edges {
                style: "Bevel".into(),
                size: 4,
                edges_mask: 0,
                seed: 0,
            },
            EffectConfig::Edges {
                style: "Fade".into(),
                size: 4,
                edges_mask: 0,
                seed: 0,
            },
            EffectConfig::PageCurl {
                corner: "BottomRight".into(),
                radius: 12.0,
                background_color: ColorRgba::TRANSPARENT,
            },
            EffectConfig::Perspective {
                angle_x: 20.0,
                angle_y: 0.0,
                reflection: true,
                reflection_opacity: 0.5,
            },
            EffectConfig::Caption {
                text: "SnagSoup Caption".into(),
                position: "Bottom".into(),
                font_family: "Arial".into(),
                font_size: 14.0,
                text_color: ColorRgba::WHITE,
                background_color: ColorRgba::rgb(30, 30, 30),
            },
            EffectConfig::Trim {
                tolerance: 2,
                auto_trim: true,
            },
            EffectConfig::Rotate {
                angle_degrees: 90.0,
                background_color: ColorRgba::TRANSPARENT,
            },
            EffectConfig::Rotate {
                angle_degrees: 15.0,
                background_color: ColorRgba::TRANSPARENT,
            },
            EffectConfig::Flip {
                horizontal: true,
                vertical: false,
            },
            EffectConfig::Scale {
                width: 32,
                height: 32,
                keep_aspect_ratio: true,
                dpi: 96,
            },
        ];

        // Test on 1x1, 3x3, and 40x30 images -> must NEVER panic!
        for &(w, h) in &[(1u32, 1u32), (3, 3), (40, 30)] {
            let src = vec![100u8, 150, 200, 255].repeat((w * h) as usize);
            for eff in &effects {
                let (nw, nh, out) = apply_effect(w, h, src.clone(), eff);
                assert!(nw >= 1 && nh >= 1);
                assert_eq!(out.len(), (nw as usize) * (nh as usize) * 4);
            }
        }

        // Test Watermark directly on RGBA buffer
        let base = vec![255u8; 40 * 40 * 4];
        let wm = vec![255u8, 0, 0, 200].repeat(10 * 10);
        let (ww, wh, wm_out) =
            apply_watermark(40, 40, &base, &wm, 10, 10, "Center", 1.0, 0.8, true);
        assert_eq!((ww, wh), (40, 40));
        assert_eq!(wm_out.len(), 40 * 40 * 4);

        // Test preview_effect_on_rgba + undoable DocumentSession::apply_effect
        let (pw, ph, _) = preview_effect_on_rgba(
            40,
            40,
            &base,
            &EffectConfig::Border {
                width: 2,
                color: ColorRgba::BLACK,
                inside: false,
            },
            20,
        );
        assert!(pw <= 24 && ph <= 24);

        let bmp = TiledBitmap::new_filled(40, 30, ColorRgba::WHITE);
        let mut session = DocumentSession::new(SnagxPage::default(), bmp);
        assert!(session.apply_effect(&EffectConfig::Rotate {
            angle_degrees: 90.0,
            background_color: ColorRgba::TRANSPARENT,
        }));
        assert_eq!((session.bitmap.width(), session.bitmap.height()), (30, 40));
        assert!(session.undo());
        assert_eq!((session.bitmap.width(), session.bitmap.height()), (40, 30));
    }

    #[test]
    fn templates_w2_14_combine_images_and_meme_1_snagt() {
        let a = TiledBitmap::new_filled(100, 80, ColorRgba::rgb(255, 0, 0));
        let b = TiledBitmap::new_filled(120, 60, ColorRgba::rgb(0, 255, 0));
        let c = TiledBitmap::new_filled(80, 100, ColorRgba::rgb(0, 0, 255));

        let combined = combine_images(
            &[&a, &b, &c],
            &CombineOptions {
                layout: CombineLayout::Grid { columns: 2 },
                spacing: 10,
                padding: 15,
                background: ColorRgba::WHITE,
            },
        );
        // Col0=max(100,80)=100, Col1=120 -> width = 30 + 220 + 10 = 260
        // Row0=max(80,60)=80, Row1=100 -> height = 30 + 180 + 10 = 220
        assert_eq!((combined.width(), combined.height()), (260, 220));

        // Load real Meme_1.snagt if present, assign image to DropZone, duplicate section, renumber steps
        let path = Path::new(r"C:\ProgramData\TechSmith\Snagit 22\Templates\Meme_1.snagt");
        if path.exists() {
            let tmpl = snagsoup_model::SnagTemplate::from_template_file(path).unwrap();
            let mut engine = TemplateEngine::from_template(tmpl);
            assert!(engine.dropzone_count() >= 1);
            assert!(engine
                .assign_image_to_dropzone(0, &a, DropZoneFitMode::Fill, PointF::new(0.0, 0.0))
                .unwrap());
            let before_secs = engine.page.sections.as_ref().map_or(0, |s| s.len());
            if before_secs > 0 {
                assert!(engine.duplicate_section(0));
                assert_eq!(engine.page.sections.as_ref().unwrap().len(), before_secs + 1);
                assert!(engine.swap_dropzones(0, 1));
                assert!(engine.delete_section(1).is_some());
            }
        }
    }
}
