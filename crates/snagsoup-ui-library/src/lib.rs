//! SnagSoup Wave 5 GPUI Views (`W3-14` + `W3-15`):
//! Library Organizer (FTS5 + Virtualized Grid), Create Image from Template,
//! Combine Images, XLIFF Callout Translation Review, and Local-First Share Menu.

pub mod combine_view;
pub mod library_view;
pub mod share_menu;
pub mod template_view;
pub mod xliff_view;

pub use combine_view::CombineImagesView;
pub use library_view::{
    LibraryFilter, LibraryOrganizerView, ThumbnailLruCache, compute_visible_grid_range,
    display_item_title,
};
pub use share_menu::{ShareMenuView, ShareTargetKind};
pub use template_view::TemplateBuilderView;
pub use xliff_view::XliffTranslationView;

#[cfg(test)]
mod tests {
    use super::*;
    use snagsoup_canvas::{CombineLayout, TiledBitmap};
    use snagsoup_library_export::{LibraryDb, LibraryItem, MediaType};
    use snagsoup_model::{
        CaptureObject, ColorRgba, PointF, RichTextSpan, RichTextStyle, SnagTemplate, SnagxArchive,
        SnagxIndex, SnagxMetadata, SnagxPage, SnagxPageEntry, ToolMode, encode_rtf_base64,
        generate_rtf,
    };

    #[test]
    fn w3_14_virtual_grid_10000_items_and_fts5_filter() {
        // 10,000 items, 4 columns, 180px row height, 720px viewport (4 visible rows + overscan)
        let (s0, e0) = compute_visible_grid_range(0.0, 720.0, 10_000, 4, 180.0);
        assert_eq!(s0, 0);
        assert!(e0 <= 24, "Expected <= 24 items rendered at top, got {e0}");

        let (sm, em) = compute_visible_grid_range(90_000.0, 720.0, 10_000, 4, 180.0);
        assert!(sm > 1900 && em <= 10_000);
        assert!(em - sm <= 32, "Virtualized window too large: {}", em - sm);

        // Bounded LRU cache respects max_entries cap
        let mut cache = ThumbnailLruCache::new(16);
        for i in 0..50 {
            cache.insert(format!("k{i}"), vec![i as u8; 64]);
        }
        assert_eq!(cache.len(), 16);

        // Live LibraryOrganizerView wired to LibraryDb FTS5 + batch tagging + delete
        let mut db = LibraryDb::open_in_memory().unwrap();
        let id1 = db
            .upsert_item(&LibraryItem {
                id: 0,
                file_path: r"C:\Caps\a.snagx".into(),
                media_type: MediaType::Image,
                capture_date: "2026-11-21 10:00:00".into(),
                app_name: "Chrome".into(),
                window_title: "Docs".into(),
                web_url: "https://example.com/guide".into(),
                callout_text: "Hướng dẫn".into(),
                ocr_text: "SnagSoup Library".into(),
                width: 1920,
                height: 1080,
                tags: vec!["work".into()],
            })
            .unwrap();
        let _id2 = db
            .upsert_item(&LibraryItem {
                id: 0,
                file_path: r"C:\Caps\b.mp4".into(),
                media_type: MediaType::Video,
                capture_date: "2026-11-20 09:00:00".into(),
                app_name: "VSCode".into(),
                window_title: "Demo".into(),
                web_url: String::new(),
                callout_text: String::new(),
                ocr_text: String::new(),
                width: 1280,
                height: 720,
                tags: vec![],
            })
            .unwrap();

        let mut view = LibraryOrganizerView::new(db);
        assert_eq!(view.items.len(), 2);

        view.set_search_query("Huong dan");
        assert_eq!(view.items.len(), 1);
        assert_eq!(view.items[0].id, id1);

        view.toggle_select(id1);
        assert_eq!(view.batch_apply_tags(&["docs".into(), "rust".into()]), 1);
        view.set_filter(LibraryFilter::Videos);
        assert_eq!(view.items.len(), 1);

        // Clean card display title falls back to basename when window_title is empty
        let untitled = LibraryItem {
            id: 99,
            file_path: r"C:\Users\demo\AppData\Local\SnagSoup\Captures\SnagSoup_179533.snagx".into(),
            media_type: MediaType::Image,
            capture_date: String::new(),
            app_name: "SnagSoup".into(),
            window_title: String::new(),
            web_url: String::new(),
            callout_text: String::new(),
            ocr_text: String::new(),
            width: 800,
            height: 600,
            tags: vec![],
        };
        assert_eq!(display_item_title(&untitled), "SnagSoup_179533.snagx");

        // `sync_directory` prunes orphaned rows whose physical files were deleted
        view.set_filter(LibraryFilter::Recent);
        assert_eq!(view.items.len(), 2);
        let empty_dir = std::env::temp_dir().join("snagsoup_lib_sync_prune_test");
        let _ = std::fs::remove_dir_all(&empty_dir);
        std::fs::create_dir_all(&empty_dir).unwrap();
        view.sync_directory(&empty_dir);
        assert_eq!(view.items.len(), 0, "orphaned missing files must be pruned on sync_directory");
        let _ = std::fs::remove_dir_all(&empty_dir);
    }

    #[test]
    fn w3_15_template_combine_xliff_and_share_dispatch() {
        // 1. TemplateBuilderView + DropZone assignment & swap
        let tmpl = SnagTemplate {
            capture_objects: vec![
                CaptureObject {
                    tool_mode: Some(ToolMode::DropZone),
                    drop_zone_group_id: Some(1),
                    ..CaptureObject::default()
                },
                CaptureObject {
                    tool_mode: Some(ToolMode::DropZone),
                    drop_zone_group_id: Some(2),
                    ..CaptureObject::default()
                },
            ],
            ..SnagTemplate::default()
        };
        let mut tv = TemplateBuilderView::new(tmpl, "TwoUp");
        let img_a = TiledBitmap::new_filled(40, 30, ColorRgba::rgb(255, 0, 0));
        let img_b = TiledBitmap::new_filled(50, 30, ColorRgba::rgb(0, 255, 0));
        assert!(tv.assign_image(0, &img_a));
        assert!(tv.assign_image(1, &img_b));
        assert!(tv.drag_to_swap(0, 1));
        assert_eq!(
            tv.engine.page.capture_objects[0].image_size,
            Some(PointF::new(50.0, 30.0))
        );

        // 2. CombineImagesView
        let mut cv = CombineImagesView::new(vec![("A".into(), img_a), ("B".into(), img_b)]);
        cv.set_layout(CombineLayout::Vertical);
        cv.set_spacing(10);
        assert!(cv.move_item(0, 1));
        let combined = cv.build_combined();
        assert_eq!(combined.height(), 20 * 2 + 30 + 30 + 10);

        // 3. XliffTranslationView + overflow focus
        let rtf = generate_rtf(&[RichTextSpan {
            text: "OK".into(),
            style: RichTextStyle {
                font_size_pt: Some(18.0),
                ..RichTextStyle::default()
            },
        }]);
        let archive = SnagxArchive {
            index: SnagxIndex {
                pages: vec!["{P}.json".into()],
                version: "1.0".into(),
                extra: Default::default(),
            },
            metadata: SnagxMetadata::default(),
            thumbnail_png: None,
            pages: vec![SnagxPageEntry {
                page_filename: "{P}.json".into(),
                page: SnagxPage {
                    capture_objects: vec![CaptureObject {
                        object_id: Some("btn-ok".into()),
                        tool_mode: Some(ToolMode::Callout),
                        start_point: Some(PointF::new(0.0, 0.0)),
                        end_point: Some(PointF::new(60.0, 24.0)),
                        font_size: Some(18.0),
                        rtf_encoded_text: Some(encode_rtf_base64(&rtf)),
                        ..CaptureObject::default()
                    }],
                    ..SnagxPage::default()
                },
                background_png: None,
                backup_json: None,
            }],
            extra_entries: Default::default(),
        };
        let mut xv = XliffTranslationView::new(archive, "en-US", "vi-VN");
        let xml = xv.export_xliff().unwrap();
        let vi = xml.replace(
            "<target state=\"needs-translation\"/>",
            "<target state=\"translated\">Đồng ý và xác nhận toàn bộ thay đổi ngay lập tức</target>",
        );
        assert!(xv.import_xliff(&vi).unwrap() >= 1);
        assert_eq!(xv.focus_issue(0), Some("btn-ok".to_string()));

        // 4. ShareMenuView FileAutoSave + ShareHistory persistence
        let tmp = std::env::temp_dir().join("snagsoup_ui_library_share_test");
        let _ = std::fs::remove_dir_all(&tmp);
        let hist_path = tmp.join("history.json");
        let mut sm = ShareMenuView::new(&tmp, Some(hist_path.clone()));
        let rgba = vec![10u8, 20, 30, 255].repeat(16 * 16);
        let receipt = sm
            .execute_share(ShareTargetKind::FileAutoSave, 16, 16, &rgba, "shot")
            .unwrap();
        assert!(std::path::Path::new(receipt.target_path.as_ref().unwrap()).exists());
        assert!(hist_path.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn library_delete_requires_confirmation_and_deletes_disk_file_permanently() {
        let tmp_dir = std::env::temp_dir().join("snagsoup_lib_delete_confirm_test");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let cap_file = tmp_dir.join("SnagSoup_test_del.snagx");
        let archive = SnagxArchive {
            index: SnagxIndex {
                pages: vec!["{P}.json".into()],
                version: "1.0".into(),
                extra: Default::default(),
            },
            metadata: SnagxMetadata::default(),
            thumbnail_png: None,
            pages: vec![SnagxPageEntry {
                page_filename: "{P}.json".into(),
                page: SnagxPage::default(),
                background_png: None,
                backup_json: None,
            }],
            extra_entries: Default::default(),
        };
        archive.to_file(&cap_file).unwrap();

        let db = LibraryDb::open_in_memory().unwrap();
        let mut view = LibraryOrganizerView::new(db);
        assert_eq!(view.sync_directory(&tmp_dir), 1);
        assert_eq!(view.items.len(), 1);
        let item_id = view.items[0].id;

        // 1. Selecting & clicking Delete opens confirmation modal without deleting yet
        view.toggle_select(item_id);
        assert!(view.request_delete_selected());
        assert!(view.confirm_delete_open);
        assert!(cap_file.exists(), "file must NOT be deleted before confirmation");

        // 2. Canceling confirmation keeps both file on disk and DB record intact
        view.cancel_delete_confirmation();
        assert!(!view.confirm_delete_open);
        assert!(cap_file.exists());
        assert_eq!(view.items.len(), 1);

        // 3. Confirming deletion removes physical file on disk AND DB entry permanently,
        //    so reopening/re-syncing the folder never resurrects the deleted capture!
        assert!(view.request_delete_selected());
        let deleted = view.confirm_delete_selected_paths();
        assert_eq!(deleted.len(), 1);
        assert!(!view.confirm_delete_open);
        assert!(!cap_file.exists(), "physical capture file must be deleted from disk");
        assert_eq!(view.items.len(), 0);
        assert_eq!(view.sync_directory(&tmp_dir), 0);
        assert_eq!(view.items.len(), 0);

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
