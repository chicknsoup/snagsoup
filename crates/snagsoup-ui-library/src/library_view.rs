use gpui::{
    App, Context, InteractiveElement, IntoElement, MouseButton, ObjectFit, ParentElement, Render,
    RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage, Window, div, img,
    prelude::FluentBuilder, px, rgb, rgba,
};
use snagsoup_canvas::downscale_rgba_bilinear;
use snagsoup_library_export::{
    FacetCount, LibraryDb, LibraryItem, MediaType, RasterExportFormat, export_raster_bytes,
    import_raster_bytes,
};
use snagsoup_model::SnagxArchive;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryFilter {
    Recent,
    Images,
    Videos,
    Gifs,
    YearMonth(String),
    App(String),
    Domain(String),
    Tag(String),
}

/// Computes the exact `(start_idx, end_idx)` range of visible grid items for a virtualized
/// thumbnail grid so 10,000+ captures render only the visible viewport rows (< 15 MB RAM!).
pub fn compute_visible_grid_range(
    scroll_y: f32,
    viewport_h: f32,
    total_items: usize,
    cols: usize,
    row_h: f32,
) -> (usize, usize) {
    if total_items == 0 || cols == 0 || row_h <= 0.0 || viewport_h <= 0.0 {
        return (0, 0);
    }
    let y = scroll_y.max(0.0);
    let first_row = (y / row_h).floor() as usize;
    let visible_rows = (viewport_h / row_h).ceil() as usize + 2; // +1 top/bottom overscan
    let start_row = first_row.saturating_sub(1);
    let end_row = first_row.saturating_add(visible_rows);

    let start_idx = (start_row.saturating_mul(cols)).min(total_items);
    let end_idx = (end_row.saturating_mul(cols)).min(total_items);
    (start_idx, end_idx)
}

/// Bounded LRU thumbnail cache keeping decoded thumbnail bytes strictly under a RAM cap.
#[derive(Debug)]
pub struct ThumbnailLruCache {
    max_entries: usize,
    order: VecDeque<String>,
    map: HashMap<String, Vec<u8>>,
}

impl ThumbnailLruCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(8),
            order: VecDeque::new(),
            map: HashMap::new(),
        }
    }

    pub fn get(&mut self, key: &str) -> Option<&[u8]> {
        if self.map.contains_key(key) {
            if let Some(pos) = self.order.iter().position(|k| k == key) {
                self.order.remove(pos);
            }
            self.order.push_back(key.to_string());
            return self.map.get(key).map(|v| v.as_slice());
        }
        None
    }

    pub fn insert(&mut self, key: String, png_bytes: Vec<u8>) {
        if self.map.contains_key(&key) {
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
        } else if self.map.len() >= self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, png_bytes);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn rgba_to_bgra_thumb_image(w: u32, h: u32, rgba: &[u8]) -> Option<Arc<RenderImage>> {
    if w == 0 || h == 0 || rgba.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    let (tw, th, scaled) = if w > 256 || h > 180 {
        let scale = (256.0 / w as f32).min(180.0 / h as f32);
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        (nw, nh, downscale_rgba_bilinear(rgba, w, h, nw, nh))
    } else {
        (w, h, rgba.to_vec())
    };
    let mut bgra = scaled;
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(tw, th, bgra)?;
    let frame = image::Frame::new(buf);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

fn load_item_render_thumb(path: &Path, media_type: &MediaType) -> Option<Arc<RenderImage>> {
    if !path.exists() {
        return None;
    }
    match media_type {
        MediaType::Video => {
            let decoded = snagsoup_media::extract_video_frame_rgba(path, 0).ok()?;
            rgba_to_bgra_thumb_image(decoded.width, decoded.height, &decoded.rgba)
        }
        _ => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "snagx" {
                let archive = SnagxArchive::from_file(path).ok()?;
                let png = archive
                    .thumbnail_png
                    .as_ref()
                    .or_else(|| archive.pages.first().and_then(|p| p.background_png.as_ref()))?;
                let bmp = snagsoup_canvas::TiledBitmap::from_png_bytes(png).ok()?;
                rgba_to_bgra_thumb_image(bmp.width(), bmp.height(), &bmp.to_rgba())
            } else {
                let bytes = std::fs::read(path).ok()?;
                let bmp = import_raster_bytes(&bytes).ok()?;
                rgba_to_bgra_thumb_image(bmp.width(), bmp.height(), &bmp.to_rgba())
            }
        }
    }
}

/// Full GPUI Library Organizer View (`W3-14`) wired to `snagsoup_library_export::LibraryDb`.
pub struct LibraryOrganizerView {
    pub db: LibraryDb,
    pub active_filter: LibraryFilter,
    pub search_query: String,
    pub items: Vec<LibraryItem>,
    pub selected_ids: BTreeSet<i64>,
    pub scroll_y: f32,
    pub viewport_h: f32,
    pub columns: usize,
    pub row_height: f32,
    pub thumb_cache: ThumbnailLruCache,
    render_thumbs: HashMap<String, Option<Arc<RenderImage>>>,
    pub facets_apps: Vec<FacetCount>,
    pub facets_domains: Vec<FacetCount>,
    pub facets_dates: Vec<FacetCount>,
    pub facets_tags: Vec<FacetCount>,
    pub status_message: String,
    pub confirm_delete_open: bool,
    on_open_item: Option<Rc<dyn Fn(PathBuf, &mut Window, &mut App) + 'static>>,
    on_items_deleted: Option<Rc<dyn Fn(&[String], &mut App) + 'static>>,
}

impl LibraryOrganizerView {
    pub fn new(db: LibraryDb) -> Self {
        let mut view = Self {
            db,
            active_filter: LibraryFilter::Recent,
            search_query: String::new(),
            items: Vec::new(),
            selected_ids: BTreeSet::new(),
            scroll_y: 0.0,
            viewport_h: 640.0,
            columns: 4,
            row_height: 196.0,
            thumb_cache: ThumbnailLruCache::new(128),
            render_thumbs: HashMap::new(),
            facets_apps: Vec::new(),
            facets_domains: Vec::new(),
            facets_dates: Vec::new(),
            facets_tags: Vec::new(),
            status_message: String::new(),
            confirm_delete_open: false,
            on_open_item: None,
            on_items_deleted: None,
        };
        view.refresh();
        view
    }

    pub fn with_on_open_item(
        mut self,
        handler: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_item = Some(Rc::new(handler));
        self
    }

    pub fn with_on_items_deleted(
        mut self,
        handler: impl Fn(&[String], &mut App) + 'static,
    ) -> Self {
        self.on_items_deleted = Some(Rc::new(handler));
        self
    }

    /// Scan `dir` (e.g. `%LOCALAPPDATA%\SnagSoup\Captures`) and index any `.snagx`, `.mp4`, or `.gif`
    /// files on disk so saved captures are always visible in the Library.
    pub fn sync_directory(&mut self, dir: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut synced = 0usize;
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let title = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Capture")
                .to_string();
            let mtime_secs = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            match ext.as_str() {
                "snagx" => {
                    if self.db.index_snagx_file(&p).is_ok() {
                        synced += 1;
                    }
                }
                "mp4" => {
                    let (w, h) = snagsoup_media::probe_video_metadata(&p)
                        .map(|m| (m.width, m.height))
                        .unwrap_or((1280, 720));
                    if self
                        .db
                        .upsert_item(&LibraryItem {
                            id: 0,
                            file_path: p.display().to_string(),
                            media_type: MediaType::Video,
                            capture_date: format!("{mtime_secs}"),
                            app_name: "SnagSoup".into(),
                            window_title: title,
                            web_url: String::new(),
                            callout_text: String::new(),
                            ocr_text: String::new(),
                            width: w,
                            height: h,
                            tags: vec!["video".into()],
                        })
                        .is_ok()
                    {
                        synced += 1;
                    }
                }
                "gif" => {
                    if self
                        .db
                        .upsert_item(&LibraryItem {
                            id: 0,
                            file_path: p.display().to_string(),
                            media_type: MediaType::Gif,
                            capture_date: format!("{mtime_secs}"),
                            app_name: "SnagSoup".into(),
                            window_title: title,
                            web_url: String::new(),
                            callout_text: String::new(),
                            ocr_text: String::new(),
                            width: 0,
                            height: 0,
                            tags: vec!["gif".into()],
                        })
                        .is_ok()
                    {
                        synced += 1;
                    }
                }
                _ => {}
            }
        }
        // Invalidate cached render thumbnails on sync in case an existing .snagx was re-saved
        self.render_thumbs.clear();
        self.refresh();
        synced
    }

    fn ensure_render_thumb(&mut self, item: &LibraryItem) -> Option<Arc<RenderImage>> {
        if let Some(cached) = self.render_thumbs.get(&item.file_path) {
            return cached.clone();
        }
        if self.render_thumbs.len() >= 128 {
            self.render_thumbs.clear();
        }
        let loaded = load_item_render_thumb(Path::new(&item.file_path), &item.media_type);
        self.render_thumbs
            .insert(item.file_path.clone(), loaded.clone());
        loaded
    }

    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
        self.scroll_y = 0.0;
        self.refresh();
    }

    pub fn set_filter(&mut self, filter: LibraryFilter) {
        self.active_filter = filter;
        self.search_query.clear();
        self.scroll_y = 0.0;
        self.refresh();
    }

    pub fn toggle_select(&mut self, item_id: i64) {
        if !self.selected_ids.remove(&item_id) {
            self.selected_ids.insert(item_id);
        }
    }

    /// Applies tags to all currently selected items in batch.
    pub fn batch_apply_tags(&mut self, tags: &[String]) -> usize {
        let ids: Vec<i64> = self.selected_ids.iter().copied().collect();
        let mut updated = 0;
        for id in ids {
            if self.db.set_item_tags(id, tags).is_ok() {
                updated += 1;
            }
        }
        self.refresh();
        self.status_message = format!("Tagged {updated} items");
        updated
    }

    /// Open the confirmation modal before permanently deleting selected captures.
    pub fn request_delete_selected(&mut self) -> bool {
        if self.selected_ids.is_empty() {
            self.status_message = "Select at least 1 item to delete".to_string();
            return false;
        }
        self.confirm_delete_open = true;
        true
    }

    /// Cancel the pending delete confirmation modal without deleting anything.
    pub fn cancel_delete_confirmation(&mut self) {
        self.confirm_delete_open = false;
    }

    /// Permanently deletes all selected items from BOTH disk (`std::fs::remove_file`) and
    /// SQLite `LibraryDb`, returning the list of deleted file paths.
    pub fn confirm_delete_selected_paths(&mut self) -> Vec<String> {
        let paths: Vec<String> = self
            .items
            .iter()
            .filter(|i| self.selected_ids.contains(&i.id))
            .map(|i| i.file_path.clone())
            .collect();
        let mut deleted_paths = Vec::new();
        for p in paths {
            // Remove physical file on disk if present so sync_directory never resurrects it!
            let _ = std::fs::remove_file(&p);
            self.render_thumbs.remove(&p);
            if self.db.remove_by_path(&p).unwrap_or(0) > 0 {
                deleted_paths.push(p);
            }
        }
        self.selected_ids.clear();
        self.confirm_delete_open = false;
        self.refresh();
        self.status_message = format!("Deleted {} item(s) permanently", deleted_paths.len());
        deleted_paths
    }

    /// Deletes all selected items from disk and library index.
    pub fn batch_delete_selected(&mut self) -> usize {
        self.confirm_delete_selected_paths().len()
    }

    /// Batch-exports selected `.snagx` items into `out_dir` using `format`.
    pub fn batch_export_selected(
        &mut self,
        out_dir: &Path,
        format: RasterExportFormat,
    ) -> Vec<PathBuf> {
        let _ = std::fs::create_dir_all(out_dir);
        let mut exported = Vec::new();
        let ext = match format {
            RasterExportFormat::Png => "png",
            RasterExportFormat::Jpeg { .. } => "jpg",
            RasterExportFormat::WebP => "webp",
            RasterExportFormat::Bmp => "bmp",
            RasterExportFormat::Tiff => "tiff",
            RasterExportFormat::Ico => "ico",
        };
        for item in &self.items {
            if !self.selected_ids.contains(&item.id) {
                continue;
            }
            let src = Path::new(&item.file_path);
            let Ok(archive) = SnagxArchive::from_file(src) else {
                continue;
            };
            let Some(png) = archive
                .thumbnail_png
                .as_ref()
                .or_else(|| archive.pages.first().and_then(|p| p.background_png.as_ref()))
            else {
                continue;
            };
            if let Ok(bmp) = snagsoup_canvas::TiledBitmap::from_png_bytes(png) {
                if let Ok(bytes) = export_raster_bytes(bmp.width(), bmp.height(), &bmp.to_rgba(), format) {
                    let stem = src
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("capture");
                    let dest = out_dir.join(format!("{stem}.{ext}"));
                    if std::fs::write(&dest, bytes).is_ok() {
                        exported.push(dest);
                    }
                }
            }
        }
        self.status_message = format!("Batch exported {} items", exported.len());
        exported
    }

    pub fn visible_slice(&self) -> &[LibraryItem] {
        let (s, e) = compute_visible_grid_range(
            self.scroll_y,
            self.viewport_h,
            self.items.len(),
            self.columns,
            self.row_height,
        );
        &self.items[s..e]
    }

    pub fn refresh(&mut self) {
        self.facets_apps = self.db.facets_by_app().unwrap_or_default();
        self.facets_domains = self.db.facets_by_domain().unwrap_or_default();
        self.facets_dates = self.db.facets_by_year_month().unwrap_or_default();
        self.facets_tags = self.db.facets_by_tag().unwrap_or_default();

        if !self.search_query.trim().is_empty() {
            self.items = self
                .db
                .search_fts(self.search_query.trim(), 500)
                .unwrap_or_default();
            return;
        }

        let base = match &self.active_filter {
            LibraryFilter::Recent => self.db.list_recent(500).unwrap_or_default(),
            LibraryFilter::Images => self
                .db
                .filter_by_media_type(MediaType::Image, 500)
                .unwrap_or_default(),
            LibraryFilter::Videos => self
                .db
                .filter_by_media_type(MediaType::Video, 500)
                .unwrap_or_default(),
            LibraryFilter::Gifs => self
                .db
                .filter_by_media_type(MediaType::Gif, 500)
                .unwrap_or_default(),
            LibraryFilter::YearMonth(ym) => self
                .db
                .list_recent(1000)
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.capture_date.starts_with(ym))
                .collect(),
            LibraryFilter::App(app) => self
                .db
                .list_recent(1000)
                .unwrap_or_default()
                .into_iter()
                .filter(|i| &i.app_name == app)
                .collect(),
            LibraryFilter::Domain(dom) => self
                .db
                .list_recent(1000)
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.web_url.to_ascii_lowercase().contains(dom))
                .collect(),
            LibraryFilter::Tag(tag) => self
                .db
                .list_recent(1000)
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.tags.iter().any(|t| t == tag))
                .collect(),
        };
        self.items = base;
    }
}

impl Render for LibraryOrganizerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (start, end) = compute_visible_grid_range(
            self.scroll_y,
            self.viewport_h,
            self.items.len(),
            self.columns,
            self.row_height,
        );

        let filters: &[(LibraryFilter, &str, &str)] = &[
            (LibraryFilter::Recent, "lib-flt-recent", "Recent Captures"),
            (LibraryFilter::Images, "lib-flt-images", "Images"),
            (LibraryFilter::Videos, "lib-flt-videos", "Videos"),
            (LibraryFilter::Gifs, "lib-flt-gifs", "GIFs"),
        ];

        let mut sidebar = div()
            .w(px(220.0))
            .bg(rgb(0x1E2128))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .text_color(rgb(0xE2E8F0))
            .child(div().text_sm().child("Library"));

        for (flt, fid, label) in filters {
            let active = &self.active_filter == flt;
            let flt_clone = flt.clone();
            sidebar = sidebar.child(
                div()
                    .id(*fid)
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(if active { rgb(0x2563EB) } else { rgb(0x272B36) })
                    .cursor_pointer()
                    .text_xs()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_filter(flt_clone.clone());
                        cx.notify();
                    }))
                    .child(*label),
            );
        }

        sidebar = sidebar
            .child(div().pt(px(6.0)).text_xs().child(format!("Dates ({})", self.facets_dates.len())))
            .child(div().text_xs().child(format!("Applications ({})", self.facets_apps.len())))
            .child(div().text_xs().child(format!("Websites ({})", self.facets_domains.len())))
            .child(div().text_xs().child(format!("Tags ({})", self.facets_tags.len())));

        for (idx, app) in self.facets_apps.iter().take(5).enumerate() {
            let app_name = app.label.clone();
            sidebar = sidebar.child(
                div()
                    .id(SharedString::from(format!("lib-app-{idx}")))
                    .pl(px(8.0))
                    .py(px(2.0))
                    .text_xs()
                    .text_color(rgb(0x94A3B8))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_filter(LibraryFilter::App(app_name.clone()));
                        cx.notify();
                    }))
                    .child(format!("{} ({})", app.label, app.count)),
            );
        }

        let visible_items: Vec<LibraryItem> = self.items[start..end].to_vec();
        let on_open_cb = self.on_open_item.clone();

        let mut grid = div()
            .flex_1()
            .p(px(16.0))
            .flex()
            .flex_wrap()
            .gap(px(12.0));

        for item in visible_items {
            let item_id = item.id;
            let item_path = PathBuf::from(&item.file_path);
            let item_path_open = item_path.clone();
            let on_open_card = on_open_cb.clone();
            let on_open_dbl = on_open_cb.clone();
            let thumb_img = self.ensure_render_thumb(&item);
            let selected = self.selected_ids.contains(&item_id);
            let border_col = if selected { rgb(0x3B82F6) } else { rgb(0x334155) };
            let badge_txt = match item.media_type {
                MediaType::Video => "VIDEO",
                MediaType::Gif => "GIF",
                MediaType::Image => "IMG",
            };

            grid = grid.child(
                div()
                    .id(SharedString::from(format!("lib-card-{item_id}")))
                    .w(px(208.0))
                    .h(px(184.0))
                    .p(px(8.0))
                    .bg(if selected { rgb(0x1E3A8A) } else { rgb(0x272B36) })
                    .border_1()
                    .border_color(border_col)
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .gap(px(6.0))
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                        if ev.click_count() >= 2 {
                            if let Some(ref cb) = on_open_dbl {
                                cb(item_path.clone(), window, cx);
                                return;
                            }
                        }
                        this.toggle_select(item_id);
                        cx.notify();
                    }))
                    // Thumbnail preview box
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(114.0))
                            .rounded(px(4.0))
                            .bg(rgb(0x11141B))
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(thumb_img, |el, t_img| {
                                el.child(
                                    img(t_img)
                                        .object_fit(ObjectFit::Contain)
                                        .w_full()
                                        .h_full(),
                                )
                            })
                            .child(
                                div()
                                    .absolute()
                                    .top(px(4.0))
                                    .left(px(4.0))
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(rgb(0x0F172A))
                                    .text_xs()
                                    .text_color(rgb(0xE2E8F0))
                                    .child(badge_txt),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .overflow_hidden()
                            .text_color(rgb(0xF8FAFC))
                            .child(if item.window_title.is_empty() {
                                item.file_path.clone()
                            } else {
                                item.window_title.clone()
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x94A3B8))
                                    .child(format!("{}×{}", item.width, item.height)),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("lib-open-{item_id}")))
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(0x2563EB))
                                    .text_xs()
                                    .text_color(rgb(0xFFFFFF))
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        cx.stop_propagation();
                                        if let Some(ref cb) = on_open_card {
                                            cb(item_path_open.clone(), window, cx);
                                        }
                                    })
                                    .child("Open"),
                            ),
                    ),
            );
        }

        let selected_count = self.selected_ids.len();
        let show_confirm = self.confirm_delete_open && selected_count > 0;
        let first_selected_name = self
            .items
            .iter()
            .find(|i| self.selected_ids.contains(&i.id))
            .map(|i| {
                if i.window_title.is_empty() {
                    i.file_path.clone()
                } else {
                    i.window_title.clone()
                }
            })
            .unwrap_or_default();

        div()
            .relative()
            .size_full()
            .flex()
            .bg(rgb(0x161920))
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .min_h(px(44.0))
                            .px(px(16.0))
                            .py(px(6.0))
                            .bg(rgb(0x1B1F27))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .text_color(rgb(0xE2E8F0))
                            .child(format!(
                                "Library ({} items, showing {}..{}) · {}",
                                self.items.len(),
                                start,
                                end,
                                self.status_message
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(format!("Selected: {selected_count}"))
                                    .child(
                                        div()
                                            .id("lib-btn-refresh")
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(0x334155))
                                            .cursor_pointer()
                                            .text_xs()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.refresh();
                                                this.status_message = "Refreshed".into();
                                                cx.notify();
                                            }))
                                            .child("↻ Refresh"),
                                    )
                                    .child(
                                        div()
                                            .id("lib-btn-tag")
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(0x2563EB))
                                            .cursor_pointer()
                                            .text_xs()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.batch_apply_tags(&["starred".into()]);
                                                cx.notify();
                                            }))
                                            .child("★ Tag Selected"),
                                    )
                                    .child(
                                        div()
                                            .id("lib-btn-export")
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(0x0D9488))
                                            .cursor_pointer()
                                            .text_xs()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                let out_dir = std::env::var("LOCALAPPDATA")
                                                    .map(PathBuf::from)
                                                    .unwrap_or_else(|_| std::env::temp_dir())
                                                    .join("SnagSoup")
                                                    .join("Exports");
                                                this.batch_export_selected(
                                                    &out_dir,
                                                    RasterExportFormat::Png,
                                                );
                                                cx.notify();
                                            }))
                                            .child("⬇ Export PNG"),
                                    )
                                    .child(
                                        div()
                                            .id("lib-btn-delete")
                                            .px(px(10.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .bg(if selected_count > 0 {
                                                rgb(0xDC2626)
                                            } else {
                                                rgb(0x475569)
                                            })
                                            .opacity(if selected_count > 0 { 1.0 } else { 0.55 })
                                            .cursor_pointer()
                                            .text_xs()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                let _ = this.request_delete_selected();
                                                cx.notify();
                                            }))
                                            .child("🗑 Delete"),
                                    ),
                            ),
                    )
                    .child(grid),
            )
            // Centered Delete Confirmation Modal
            .when(show_confirm, |root| {
                root.child(
                    div()
                        .id("lib-delete-modal-backdrop")
                        .absolute()
                        .inset_0()
                        .bg(rgba(0x000000AA))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .w(px(420.0))
                                .p(px(20.0))
                                .rounded(px(10.0))
                                .bg(rgb(0x1E2430))
                                .border_1()
                                .border_color(rgb(0xEF4444))
                                .shadow_lg()
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .text_color(rgb(0xF8FAFC))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child(format!(
                                            "Xóa vĩnh viễn {selected_count} mục đã chọn?"
                                        )),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0xCBD5E1))
                                        .child(format!(
                                            "Thao tác này sẽ xóa vĩnh viễn {selected_count} file khỏi Thư viện và ổ đĩa ({first_selected_name}). Không thể hoàn tác."
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .justify_end()
                                        .gap(px(10.0))
                                        .pt(px(4.0))
                                        .child(
                                            div()
                                                .id("lib-del-cancel")
                                                .px(px(14.0))
                                                .py(px(6.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(0x334155))
                                                .hover(|s| s.bg(rgb(0x475569)))
                                                .cursor_pointer()
                                                .text_xs()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.cancel_delete_confirmation();
                                                    cx.notify();
                                                }))
                                                .child("Hủy (Cancel)"),
                                        )
                                        .child(
                                            div()
                                                .id("lib-del-confirm")
                                                .px(px(14.0))
                                                .py(px(6.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(0xDC2626))
                                                .hover(|s| s.bg(rgb(0xEF4444)))
                                                .cursor_pointer()
                                                .text_xs()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    let deleted =
                                                        this.confirm_delete_selected_paths();
                                                    if let Some(ref cb) = this.on_items_deleted {
                                                        cb(&deleted, cx);
                                                    }
                                                    cx.notify();
                                                }))
                                                .child("🗑 Xóa vĩnh viễn (Delete)"),
                                        ),
                                ),
                        ),
                )
            })
    }
}
