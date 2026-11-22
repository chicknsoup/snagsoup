//! Full GPUI Snagit Editor Window Shell (`W3-05`) wired to `snagsoup-canvas::DocumentSession`
//! and `.snagx` open/save.

use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    AppContext, Bounds, Context, CursorStyle, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Point, Render, RenderImage,
    ScrollDelta, ScrollEvent, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    UTF16Selection, Window, canvas, div, img, prelude::FluentBuilder, px, rgb, rgba,
};
use snagsoup_canvas::{
    CutOutOrientation, DocumentSession, HistoryCommand, PixelRect, SceneCompositor, TiledBitmap,
    downscale_rgba_bilinear, hit_test_object,
};
use snagsoup_library_export::{
    LibraryDb, LibraryItem, MediaType, PdfPageSize, RasterExportFormat, export_pdf_bytes,
    export_raster_bytes, export_svg_string,
};
use snagsoup_cv_ocr::MagicWandMode;
use snagsoup_model::{
    CaptureObject, ColorRgba, EffectConfig, PointF, RectF, SnagTemplate, SnagxArchive, SnagxIndex,
    SnagxMetadata, SnagxPage, SnagxPageEntry, ToolMode,
};
use snagsoup_ui_huds::{SlideItem, VideoFromImagesView};
use snagsoup_ui_library::{
    CombineImagesView, LibraryOrganizerView, ShareMenuView, ShareTargetKind, TemplateBuilderView,
    XliffTranslationView,
};

use crate::editor_shell::canvas_element::{
    CanvasGpuCache, CanvasInteractiveOverlays, CanvasViewportState, render_canvas_surface,
};
use crate::editor_shell::effects_panel::{EffectGizmoPart, EffectsGalleryState};
use crate::editor_shell::gif_dialog::CreateGifDialogState;
use crate::editor_shell::gizmos_g1::GizmoControllerG1;
use crate::editor_shell::gizmos_g2::{InlineTextEditor, StampSearcher};
use crate::editor_shell::quickstyles::QuickStyleManager;
use crate::editor_shell::raster_ui::RasterToolController;
use crate::editor_shell::smart_cv_ui::{
    GrabEditTextUiState, MagicWandUiState, SimplifyUiFloatyState, SmartMoveUiState,
};
use crate::editor_shell::tool_cursor::cached_tool_hcursor;
use crate::editor_shell::video_player::{
    VideoPlayerState, format_timestamp_ms, render_video_stage, render_video_timeline,
};
use crate::icons::SnagIcon;
use crate::theme::{DEFAULT_SWATCHES, SnagTheme, ThemeMode};
use crate::widgets::{
    Button, ButtonVariant, ColorSwatchPicker, IconButton, NumericStepper, Popover, Slider,
};

/// Top-level workspace mode (`Editor`, `Library`, `Template`, `Combine`, `VideoFromImages`, `Xliff`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkspaceView {
    #[default]
    Editor,
    Library,
    Template,
    Combine,
    VideoFromImages,
    Xliff,
}

/// Active tab in the collapsible right side panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidePanelTab {
    #[default]
    ToolProperties,
    Effects,
}

/// Entry displayed in the collapsible bottom Recent Captures Tray.
#[derive(Debug, Clone)]
pub struct RecentCaptureItem {
    pub id: String,
    pub title: String,
    pub path: Option<PathBuf>,
    pub dimensions: (u32, u32),
    pub thumbnail: Option<Arc<RenderImage>>,
    pub is_video: bool,
}

fn normalize_capture_id(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn make_tray_thumb_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Arc<RenderImage>> {
    if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    let max_w = 192.0f32;
    let max_h = 116.0f32;
    let scale = (max_w / width as f32).min(max_h / height as f32).min(1.0);
    let dst_w = ((width as f32) * scale).round().max(1.0) as u32;
    let dst_h = ((height as f32) * scale).round().max(1.0) as u32;
    let scaled = downscale_rgba_bilinear(rgba, width, height, dst_w, dst_h);
    crate::rgba_to_bgra_render_image(dst_w, dst_h, &scaled)
}

fn make_tray_thumb_from_bitmap(bmp: &TiledBitmap) -> Option<Arc<RenderImage>> {
    make_tray_thumb_from_rgba(bmp.width(), bmp.height(), &bmp.to_rgba())
}

fn make_tray_thumb_from_png_bytes(png_bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let bmp = TiledBitmap::from_png_bytes(png_bytes).ok()?;
    make_tray_thumb_from_bitmap(&bmp)
}

fn load_recent_item_from_path(path: &Path) -> Option<RecentCaptureItem> {
    let ext = path.extension()?.to_str()?;
    let title = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Capture")
        .to_string();
    let id = normalize_capture_id(path);

    if ext.eq_ignore_ascii_case("snagx") {
        let archive = SnagxArchive::from_file(path).ok()?;
        let first = archive.pages.first()?;
        let (dims, thumb) = if let Some(ref bg_png) = first.background_png {
            let bg_bmp = TiledBitmap::from_png_bytes(bg_png).ok();
            let d = bg_bmp
                .as_ref()
                .map(|b| (b.width(), b.height()))
                .unwrap_or_else(|| {
                    (
                        first.page.capture_canvas_width.unwrap_or(800.0).max(1.0) as u32,
                        first.page.capture_canvas_height.unwrap_or(600.0).max(1.0) as u32,
                    )
                });
            let th = archive
                .thumbnail_png
                .as_deref()
                .and_then(make_tray_thumb_from_png_bytes)
                .or_else(|| bg_bmp.as_ref().and_then(make_tray_thumb_from_bitmap));
            (d, th)
        } else {
            let w = first.page.capture_canvas_width.unwrap_or(800.0).max(1.0) as u32;
            let h = first.page.capture_canvas_height.unwrap_or(600.0).max(1.0) as u32;
            let th = archive
                .thumbnail_png
                .as_deref()
                .and_then(make_tray_thumb_from_png_bytes);
            ((w, h), th)
        };
        Some(RecentCaptureItem {
            id,
            title,
            path: Some(path.to_path_buf()),
            dimensions: dims,
            thumbnail: thumb,
            is_video: false,
        })
    } else if ext.eq_ignore_ascii_case("mp4") {
        let meta = snagsoup_media::probe_video_metadata(path).ok()?;
        let thumb = snagsoup_media::extract_video_frame_rgba(path, 0)
            .ok()
            .and_then(|f| make_tray_thumb_from_rgba(f.width, f.height, &f.rgba));
        Some(RecentCaptureItem {
            id,
            title,
            path: Some(path.to_path_buf()),
            dimensions: (meta.width, meta.height),
            thumbnail: thumb,
            is_video: true,
        })
    } else {
        None
    }
}

/// Metadata for a single tool button in the customizable Top Tool Bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolToolbarEntry {
    pub mode: ToolMode,
    pub icon: SnagIcon,
    pub label: &'static str,
    pub shortcut: Option<&'static str>,
}

/// Default ordered list of all 21 Snagit 2022 tools (`Selection` placed right near the front).
pub fn default_all_21_tools() -> Vec<ToolToolbarEntry> {
    vec![
        ToolToolbarEntry { mode: ToolMode::Move, icon: SnagIcon::Move, label: "Move", shortcut: Some("V") },
        ToolToolbarEntry { mode: ToolMode::Selection, icon: SnagIcon::Selection, label: "Selection", shortcut: Some("M") },
        ToolToolbarEntry { mode: ToolMode::Arrow, icon: SnagIcon::Arrow, label: "Arrow", shortcut: Some("A") },
        ToolToolbarEntry { mode: ToolMode::Text, icon: SnagIcon::Text, label: "Text", shortcut: Some("T") },
        ToolToolbarEntry { mode: ToolMode::Callout, icon: SnagIcon::Callout, label: "Callout", shortcut: Some("C") },
        ToolToolbarEntry { mode: ToolMode::Shape, icon: SnagIcon::Shape, label: "Shape", shortcut: Some("S") },
        ToolToolbarEntry { mode: ToolMode::Stamp, icon: SnagIcon::Stamp, label: "Stamp", shortcut: None },
        ToolToolbarEntry { mode: ToolMode::Fill, icon: SnagIcon::Fill, label: "Fill", shortcut: Some("F") },
        ToolToolbarEntry { mode: ToolMode::Blur, icon: SnagIcon::Blur, label: "Blur", shortcut: Some("B") },
        ToolToolbarEntry { mode: ToolMode::Crop, icon: SnagIcon::Crop, label: "Crop", shortcut: Some("K") },
        ToolToolbarEntry { mode: ToolMode::Step, icon: SnagIcon::Step, label: "Step", shortcut: Some("N") },
        ToolToolbarEntry { mode: ToolMode::Pen, icon: SnagIcon::Pen, label: "Pen", shortcut: Some("P") },
        ToolToolbarEntry { mode: ToolMode::Highlighter, icon: SnagIcon::Highlighter, label: "Highlighter", shortcut: Some("H") },
        ToolToolbarEntry { mode: ToolMode::Line, icon: SnagIcon::Arrow, label: "Line", shortcut: Some("L") },
        ToolToolbarEntry { mode: ToolMode::Eraser, icon: SnagIcon::Eraser, label: "Eraser", shortcut: Some("E") },
        ToolToolbarEntry { mode: ToolMode::Magnify, icon: SnagIcon::Magnify, label: "Magnify", shortcut: None },
        ToolToolbarEntry { mode: ToolMode::CutOut, icon: SnagIcon::Crop, label: "Cut Out", shortcut: None },
        ToolToolbarEntry { mode: ToolMode::Simplify, icon: SnagIcon::Effects, label: "Simplify", shortcut: None },
        ToolToolbarEntry { mode: ToolMode::SmartMove, icon: SnagIcon::Move, label: "Smart Move", shortcut: None },
        ToolToolbarEntry { mode: ToolMode::MagicWand, icon: SnagIcon::Effects, label: "Magic Wand", shortcut: None },
        ToolToolbarEntry { mode: ToolMode::Eyedropper, icon: SnagIcon::Pipette, label: "Eyedropper", shortcut: Some("I") },
    ]
}

/// Default local directory for auto-saved `.snagx` captures (`%LOCALAPPDATA%\SnagSoup\Captures`).
pub fn capture_storage_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("SnagSoup").join("Captures")
}

/// Default SQLite WAL+FTS5 Library database path (`%LOCALAPPDATA%\SnagSoup\library.db`).
pub fn library_db_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("SnagSoup").join("library.db")
}

/// Tracks how an active inline text editing session (`editing_text_obj`) commits to Undo history.
#[derive(Debug, Clone)]
pub enum TextEditBaseline {
    /// Newly created `Text` / `Callout`: live typing updates its `AddObject` history entry in place.
    NewObject,
    /// Existing `Text` / `Callout`: pre-edit snapshot committed as a single `UpdateObject` on finish.
    ExistingObject(CaptureObject),
}

/// Active interactive mouse drag operation on the Editor Canvas.
#[derive(Debug, Clone)]
pub enum CanvasDragOp {
    CreateObject {
        obj_idx: usize,
        origin: PointF,
    },
    MoveObject {
        last_canvas: PointF,
        before_objects: Vec<(usize, CaptureObject)>,
        smart_move_lift: Option<(TiledBitmap, PixelRect, usize)>,
    },
    EraseStroke {
        before_bitmap: TiledBitmap,
        points: Vec<PointF>,
    },
    RegionTool {
        start: PointF,
        current: PointF,
    },
    Pan {
        last_screen: (f32, f32),
    },
    EffectSpotlight {
        effect_idx: usize,
        part: EffectGizmoPart,
        start_canvas: PointF,
        orig_rect: RectF,
        before_bitmap: TiledBitmap,
        before_objects: Vec<CaptureObject>,
    },
}

/// Pure, unit-testable state behind [`EditorShellView`].
pub struct EditorShellState {
    pub workspace_view: WorkspaceView,
    pub side_panel_tab: SidePanelTab,
    pub show_side_panel: bool,
    pub show_recent_tray: bool,
    pub show_more_tools: bool,
    pub show_create_menu: bool,
    pub show_share_menu: bool,
    pub active_tool: ToolMode,
    pub tools: Vec<ToolToolbarEntry>,
    pub pinned_tool_count: usize,
    pub stroke_width: f32,
    pub opacity: f32,
    pub selected_color: u32,
    pub zoom_percent: f32,
    pub session: DocumentSession,
    pub active_path: Option<PathBuf>,
    pub metadata: SnagxMetadata,
    pub recent_captures: Vec<RecentCaptureItem>,
    pub status_message: String,
    pub applied_effects_count: usize,
    pub viewport: CanvasViewportState,
    pub gpu_cache: CanvasGpuCache,
    pub gizmos_g1: GizmoControllerG1,
    pub raster: RasterToolController,
    pub quickstyles: QuickStyleManager,
    pub stamps: StampSearcher,
    pub effects_gallery: EffectsGalleryState,
    pub smart_move: SmartMoveUiState,
    pub simplify_ui: SimplifyUiFloatyState,
    pub magic_wand: MagicWandUiState,
    pub grab_edit_text: GrabEditTextUiState,
    pub video_player: VideoPlayerState,
    pub gif_dialog: CreateGifDialogState,
    pub active_drag: Option<CanvasDragOp>,
    pub selected_shape_kind: String,
    pub shape_filled: bool,
    pub shape_dashed: bool,
    pub selected_stamp_id: String,
    pub stamp_category_filter: String,
    pub font_size: f32,
    pub inline_text: InlineTextEditor,
    pub editing_text_obj: Option<usize>,
    pub editing_text_baseline: Option<TextEditBaseline>,
    pub active_selection_rect: Option<RectF>,
    pub pending_crop_rect: Option<RectF>,
    pub pending_fit_canvas: bool,
    pub copy_feedback_active: bool,
    pub copy_feedback_seq: u64,
    pub hover_local_pos: Option<(f32, f32)>,
}

impl Default for EditorShellState {
    fn default() -> Self {
        let bmp = TiledBitmap::new_filled(1280, 720, ColorRgba::WHITE);
        let page = SnagxPage {
            capture_canvas_width: Some(1280.0),
            capture_canvas_height: Some(720.0),
            dpi: Some(96.0),
            capture_background_color: Some(ColorRgba::WHITE),
            ..SnagxPage::default()
        };
        Self {
            workspace_view: WorkspaceView::Editor,
            side_panel_tab: SidePanelTab::ToolProperties,
            show_side_panel: true,
            show_recent_tray: true,
            show_more_tools: false,
            show_create_menu: false,
            show_share_menu: false,
            active_tool: ToolMode::Selection,
            tools: default_all_21_tools(),
            pinned_tool_count: 10,
            stroke_width: 4.0,
            opacity: 100.0,
            selected_color: DEFAULT_SWATCHES[0],
            zoom_percent: 100.0,
            session: DocumentSession::new(page, bmp),
            active_path: None,
            metadata: SnagxMetadata {
                app_name: Some("SnagSoup".to_string()),
                ..SnagxMetadata::default()
            },
            recent_captures: vec![RecentCaptureItem {
                id: "untitled-1".to_string(),
                title: "Untitled Capture".to_string(),
                path: None,
                dimensions: (1280, 720),
                thumbnail: None,
                is_video: false,
            }],
            status_message: "Ready".to_string(),
            applied_effects_count: 0,
            viewport: CanvasViewportState::default(),
            gpu_cache: CanvasGpuCache::default(),
            gizmos_g1: GizmoControllerG1::default(),
            raster: RasterToolController::default(),
            quickstyles: QuickStyleManager::default(),
            stamps: StampSearcher::default(),
            effects_gallery: EffectsGalleryState::default(),
            smart_move: SmartMoveUiState::default(),
            simplify_ui: SimplifyUiFloatyState::default(),
            magic_wand: MagicWandUiState::default(),
            grab_edit_text: GrabEditTextUiState::default(),
            video_player: VideoPlayerState::default(),
            gif_dialog: CreateGifDialogState::default(),
            active_drag: None,
            selected_shape_kind: "Rectangle".to_string(),
            shape_filled: false,
            shape_dashed: false,
            selected_stamp_id: "sym-check".to_string(),
            stamp_category_filter: "All".to_string(),
            font_size: 20.0,
            inline_text: InlineTextEditor::default(),
            editing_text_obj: None,
            editing_text_baseline: None,
            active_selection_rect: None,
            pending_crop_rect: None,
            pending_fit_canvas: true,
            copy_feedback_active: false,
            copy_feedback_seq: 0,
            hover_local_pos: None,
        }
    }
}

impl EditorShellState {
    /// Resolve the Win32/GPUI [`CursorStyle`] matching the active Editor tool & interaction state.
    pub fn canvas_cursor_style(&self) -> CursorStyle {
        if self.viewport.space_panning
            || matches!(
                self.active_drag,
                Some(CanvasDragOp::Pan { .. } | CanvasDragOp::MoveObject { .. })
            )
        {
            return CursorStyle::ClosedHand; // Maps to Win32 4-way move cursor (`IDC_SIZEALL`)
        }
        if let Some(CanvasDragOp::EffectSpotlight { part, .. }) = self.active_drag {
            return Self::cursor_for_effect_gizmo_part(part);
        }
        if self.side_panel_tab == SidePanelTab::Effects {
            if let (Some((_, spot_r)), Some((lx, ly))) = (
                self.effects_gallery.active_spotlight_rect(),
                self.hover_local_pos,
            ) {
                let eff_z = self.viewport.effective_logical_zoom().max(0.05);
                let cpt = self.viewport.screen_to_canvas(lx, ly);
                if let Some(part) = EffectsGalleryState::hit_test_spotlight_gizmo(
                    spot_r,
                    cpt,
                    (9.0 / eff_z).max(4.0) as f64,
                ) {
                    return Self::cursor_for_effect_gizmo_part(part);
                }
            }
        }
        if self.editing_text_obj.is_some() {
            return CursorStyle::IBeam;
        }
        match self.active_tool {
            ToolMode::Move | ToolMode::SmartMove => CursorStyle::OpenHand, // `IDC_SIZEALL`
            ToolMode::Text => CursorStyle::IBeam,                          // `IDC_IBEAM`
            ToolMode::Stamp | ToolMode::Step => CursorStyle::PointingHand, // `IDC_HAND`
            ToolMode::Selection
            | ToolMode::Crop
            | ToolMode::Arrow
            | ToolMode::Line
            | ToolMode::Shape
            | ToolMode::Callout
            | ToolMode::Pen
            | ToolMode::Highlighter
            | ToolMode::Eraser
            | ToolMode::Fill
            | ToolMode::Blur
            | ToolMode::Magnify
            | ToolMode::CutOut
            | ToolMode::Simplify
            | ToolMode::MagicWand
            | ToolMode::Eyedropper
            | ToolMode::Image
            | ToolMode::Cursor
            | ToolMode::DropZone
            | ToolMode::Unknown(_) => CursorStyle::Crosshair, // `IDC_CROSS`
        }
    }

    /// Resolve whether the Editor Canvas should display a custom tool-icon hardware cursor
    /// (`Precision Crosshair + Tool Icon Badge`) or defer to a transient interaction cursor
    /// (`ClosedHand` pan/move, `Spotlight` 8-handle resize arrows, `IBeam` active text caret,
    /// `IDC_SIZEALL` Move / hovered Smart Move box, or open popovers/modals).
    pub fn active_custom_tool_cursor_mode(&self) -> Option<ToolMode> {
        if self.workspace_view != WorkspaceView::Editor
            || self.video_player.path.is_some()
            || self.show_create_menu
            || self.show_share_menu
            || self.show_more_tools
            || self.grab_edit_text.modal_open
        {
            return None;
        }
        if self.viewport.space_panning
            || matches!(
                self.active_drag,
                Some(
                    CanvasDragOp::Pan { .. }
                        | CanvasDragOp::MoveObject { .. }
                        | CanvasDragOp::EffectSpotlight { .. }
                )
            )
            || self.editing_text_obj.is_some()
        {
            return None;
        }
        if self.side_panel_tab == SidePanelTab::Effects {
            if let (Some((_, spot_r)), Some((lx, ly))) = (
                self.effects_gallery.active_spotlight_rect(),
                self.hover_local_pos,
            ) {
                let eff_z = self.viewport.effective_logical_zoom().max(0.05);
                let cpt = self.viewport.screen_to_canvas(lx, ly);
                if EffectsGalleryState::hit_test_spotlight_gizmo(
                    spot_r,
                    cpt,
                    (9.0 / eff_z).max(4.0) as f64,
                )
                .is_some()
                {
                    return None;
                }
            }
        }
        if self.active_tool == ToolMode::Move {
            return None;
        }
        if self.active_tool == ToolMode::SmartMove && self.smart_move.hovered_idx.is_some() {
            return None;
        }
        crate::editor_shell::tool_cursor::tool_cursor_svg(&self.active_tool)
            .map(|_| self.active_tool.clone())
    }

    fn cursor_for_effect_gizmo_part(part: EffectGizmoPart) -> CursorStyle {
        match part {
            EffectGizmoPart::MoveInside => CursorStyle::OpenHand,
            EffectGizmoPart::NW | EffectGizmoPart::SE => CursorStyle::ResizeUpLeftDownRight,
            EffectGizmoPart::NE | EffectGizmoPart::SW => CursorStyle::ResizeUpRightDownLeft,
            EffectGizmoPart::N | EffectGizmoPart::S => CursorStyle::ResizeUpDown,
            EffectGizmoPart::E | EffectGizmoPart::W => CursorStyle::ResizeLeftRight,
        }
    }
    pub const SHAPE_LIBRARY: &'static [(&'static str, &'static str)] = &[
        ("Rectangle", "▭ Rectangle"),
        ("RoundedRectangle", "▢ Rounded"),
        ("Ellipse", "◯ Ellipse"),
        ("Triangle", "△ Triangle"),
        ("Diamond", "◇ Diamond"),
        ("Polygon", "⬡ Hexagon"),
        ("Star", "☆ Star"),
    ];

    pub fn close_all_popovers(&mut self) {
        self.show_create_menu = false;
        self.show_more_tools = false;
        self.show_share_menu = false;
    }

    /// Select an active tool, clean up transient modes, and initialize Crop preview when entering `Crop`.
    pub fn select_tool(&mut self, mode: ToolMode) {
        self.close_all_popovers();
        self.finish_text_editing();
        if mode != ToolMode::Crop {
            self.pending_crop_rect = None;
        } else if self.pending_crop_rect.is_none() {
            let cw = self.session.bitmap.width().max(16) as f64;
            let ch = self.session.bitmap.height().max(16) as f64;
            let mx = (cw * 0.08).round();
            let my = (ch * 0.08).round();
            self.pending_crop_rect = Some(RectF::new(
                mx,
                my,
                (cw - mx * 2.0).max(8.0),
                (ch - my * 2.0).max(8.0),
            ));
            self.status_message =
                "Drag to adjust Crop box, then click ✓ Crop or press Enter".to_string();
        }
        if matches!(
            mode,
            ToolMode::MagicWand
                | ToolMode::Move
                | ToolMode::SmartMove
                | ToolMode::Simplify
                | ToolMode::Shape
                | ToolMode::Stamp
                | ToolMode::Text
                | ToolMode::Callout
                | ToolMode::Crop
        ) {
            self.side_panel_tab = SidePanelTab::ToolProperties;
        }
        match mode {
            ToolMode::MagicWand => {
                self.status_message =
                    "Magic Wand: click any color on canvas to select matching pixels (Del to clear)".into();
            }
            ToolMode::Move | ToolMode::SmartMove => {
                self.status_message =
                    "Move: drag annotations, or toggle Smart Move ON in Properties to move UI buttons".into();
            }
            ToolMode::Simplify => {
                self.status_message =
                    "Simplify: toggle Auto-Simplify in Properties or drag on canvas to cover areas".into();
            }
            _ => {}
        }
        self.active_tool = mode;
    }

    /// Active RGB swatch converted to opaque `ColorRgba`.
    pub fn active_color_rgba(&self) -> ColorRgba {
        ColorRgba::rgb(
            ((self.selected_color >> 16) & 0xFF) as u8,
            ((self.selected_color >> 8) & 0xFF) as u8,
            (self.selected_color & 0xFF) as u8,
        )
    }

    /// Execute Undo (`Ctrl+Z`), keeping selection indices, Step numbering & active masks consistent.
    pub fn perform_undo(&mut self) -> bool {
        self.finish_text_editing();
        self.active_drag = None;
        if self.session.undo() {
            let len = self.session.page.capture_objects.len();
            self.gizmos_g1.selected_indices.retain(|&i| i < len);
            self.gizmos_g1
                .step_counter
                .sync_with_objects(&self.session.page.capture_objects);
            self.simplify_ui.sync_from_session(&self.session);
            self.magic_wand.clear_selection();
            self.status_message = "Undid last action (Ctrl+Z)".to_string();
            true
        } else {
            false
        }
    }

    /// Execute Redo (`Ctrl+Y` / `Ctrl+Shift+Z`), keeping selection indices & Step numbering consistent.
    pub fn perform_redo(&mut self) -> bool {
        self.finish_text_editing();
        self.active_drag = None;
        if self.session.redo() {
            let len = self.session.page.capture_objects.len();
            self.gizmos_g1.selected_indices.retain(|&i| i < len);
            self.gizmos_g1
                .step_counter
                .sync_with_objects(&self.session.page.capture_objects);
            self.simplify_ui.sync_from_session(&self.session);
            self.magic_wand.clear_selection();
            self.status_message = "Redid action (Ctrl+Y)".to_string();
            true
        } else {
            false
        }
    }

    pub fn start_editing_text_object(&mut self, obj_idx: usize) {
        self.start_editing_text_object_with_baseline(obj_idx, false);
    }

    fn start_editing_text_object_with_baseline(&mut self, obj_idx: usize, is_new_object: bool) {
        if self.editing_text_obj.is_some() && self.editing_text_obj != Some(obj_idx) {
            self.finish_text_editing();
        }
        if let Some(obj) = self.session.page.capture_objects.get(obj_idx).cloned() {
            self.inline_text.style.font_size_pt = Some(self.font_size);
            self.inline_text.load_from_object(obj_idx, &obj);
            self.editing_text_obj = Some(obj_idx);
            self.editing_text_baseline = Some(if is_new_object {
                TextEditBaseline::NewObject
            } else {
                TextEditBaseline::ExistingObject(obj)
            });
            self.gizmos_g1.selected_indices = vec![obj_idx];
            self.status_message =
                "Typing text — Enter for new line, Esc or click outside to finish".to_string();
        }
    }

    /// Sync inline text changes to `session` in real time **without** pushing one Undo step per keystroke.
    pub fn sync_active_text_to_session(&mut self) {
        if let Some(idx) = self.editing_text_obj {
            if let Some(mut obj) = self.session.page.capture_objects.get(idx).cloned() {
                self.inline_text.style.font_size_pt = Some(self.font_size);
                self.inline_text.sync_to_object(&mut obj);
                self.session.set_object_unrecorded(idx, obj.clone());
                if matches!(self.editing_text_baseline, Some(TextEditBaseline::NewObject)) {
                    let _ = self.session.history.update_top_added_object(idx, obj);
                }
            }
        }
    }

    /// Commit inline text editing as a single Undo step (when editing an existing object) and exit edit mode.
    pub fn finish_text_editing(&mut self) {
        let Some(idx) = self.editing_text_obj else {
            self.editing_text_baseline = None;
            return;
        };
        self.sync_active_text_to_session();
        if let Some(TextEditBaseline::ExistingObject(before)) = self.editing_text_baseline.take() {
            if let Some(after) = self.session.page.capture_objects.get(idx).cloned() {
                if before != after {
                    self.session.history.push(HistoryCommand::UpdateObject {
                        index: idx,
                        before,
                        after,
                    });
                }
            }
        }
        self.editing_text_obj = None;
        self.editing_text_baseline = None;
    }

    /// Begin an interactive left-mouse gesture on the Editor Canvas (`local_x`, `local_y` relative to canvas well).
    /// All multi-frame drag gestures mutate live preview state unrecorded and commit **1** atomic Undo entry on [`Self::pointer_up`].
    pub fn pointer_down(
        &mut self,
        local_x: f32,
        local_y: f32,
        shift: bool,
        click_count: usize,
    ) {
        let canvas_pt = self.viewport.screen_to_canvas(local_x, local_y);
        let color = self.selected_color_rgba();
        let sw = self.stroke_width as f64;
        let op = self.opacity as f64;

        if self.viewport.space_panning {
            self.active_drag = Some(CanvasDragOp::Pan {
                last_screen: (local_x, local_y),
            });
            return;
        }

        // Finish active inline text editing if user clicks outside it
        if let Some(edit_idx) = self.editing_text_obj {
            let clicked_inside = self
                .session
                .page
                .capture_objects
                .get(edit_idx)
                .is_some_and(|o| hit_test_object(o, canvas_pt, 8.0));
            if !clicked_inside {
                self.finish_text_editing();
            } else {
                return;
            }
        }

        // Interactive Spotlight & Magnify on-canvas Move + 8-Handle Resize
        if self.side_panel_tab == SidePanelTab::Effects {
            if let Some((eff_idx, spot_r)) = self.effects_gallery.active_spotlight_rect() {
                let eff_z = self.viewport.effective_logical_zoom().max(0.05);
                let tol = (9.0 / eff_z).max(4.0) as f64;
                let (part, init_rect) = if let Some(hit_p) =
                    EffectsGalleryState::hit_test_spotlight_gizmo(spot_r, canvas_pt, tol)
                {
                    (hit_p, spot_r)
                } else {
                    (
                        EffectGizmoPart::SE,
                        RectF::new(canvas_pt.x, canvas_pt.y, 24.0, 24.0),
                    )
                };
                self.active_drag = Some(CanvasDragOp::EffectSpotlight {
                    effect_idx: eff_idx,
                    part,
                    start_canvas: canvas_pt,
                    orig_rect: init_rect,
                    before_bitmap: self.session.bitmap.clone(),
                    before_objects: self.session.page.capture_objects.clone(),
                });
                return;
            }
        }

        match self.active_tool.clone() {
            ToolMode::Move | ToolMode::SmartMove => {
                let hit = self
                    .gizmos_g1
                    .click_select(&self.session, canvas_pt, 8.0, shift);
                if click_count >= 2 {
                    if let Some(h_idx) = hit {
                        let is_txt = self
                            .session
                            .page
                            .capture_objects
                            .get(h_idx)
                            .and_then(|o| o.tool_mode.as_ref())
                            .is_some_and(|m| matches!(m, ToolMode::Text | ToolMode::Callout));
                        if is_txt {
                            self.start_editing_text_object(h_idx);
                            return;
                        }
                    }
                }
                let mut smart_move_lift = None;
                if hit.is_none() && self.smart_move.enabled {
                    if let Some(det_idx) = self.smart_move.hit_test_at(canvas_pt) {
                        if let Ok((obj_idx, before_bmp, clipped)) = self
                            .smart_move
                            .lift_detected_index_unrecorded(&mut self.session, det_idx)
                        {
                            self.gizmos_g1.selected_indices = vec![obj_idx];
                            smart_move_lift = Some((before_bmp, clipped, obj_idx));
                            self.status_message =
                                "Smart Move: lifted UI element & auto-filled background — drag to reposition!"
                                    .into();
                        }
                    }
                }
                if !self.gizmos_g1.selected_indices.is_empty() {
                    let before_objects = if smart_move_lift.is_some() {
                        Vec::new()
                    } else {
                        self.gizmos_g1
                            .selected_indices
                            .iter()
                            .filter_map(|&idx| {
                                self.session
                                    .page
                                    .capture_objects
                                    .get(idx)
                                    .cloned()
                                    .map(|o| (idx, o))
                            })
                            .collect()
                    };
                    self.active_drag = Some(CanvasDragOp::MoveObject {
                        last_canvas: canvas_pt,
                        before_objects,
                        smart_move_lift,
                    });
                }
            }
            ToolMode::Shape => {
                let fill_col = if self.shape_filled {
                    color
                } else {
                    ColorRgba::TRANSPARENT
                };
                let mut obj = self.gizmos_g1.build_new_object(
                    ToolMode::Shape,
                    canvas_pt,
                    PointF::new(canvas_pt.x + 4.0, canvas_pt.y + 4.0),
                    color,
                    fill_col,
                    sw,
                    op,
                );
                obj.tool_shape = Some(self.selected_shape_kind.clone());
                if self.selected_shape_kind == "RoundedRectangle" {
                    obj.corner_radius_ratio = Some(0.22);
                }
                if self.shape_dashed {
                    obj.dash_type = Some("Dash".to_string());
                }
                let idx = self.session.add_object_unrecorded(obj);
                self.gizmos_g1.selected_indices = vec![idx];
                self.active_drag = Some(CanvasDragOp::CreateObject {
                    obj_idx: idx,
                    origin: canvas_pt,
                });
            }
            ToolMode::Arrow
            | ToolMode::Line
            | ToolMode::Step
            | ToolMode::Pen
            | ToolMode::Highlighter => {
                let idx = self.gizmos_g1.create_object_unrecorded(
                    &mut self.session,
                    self.active_tool.clone(),
                    canvas_pt,
                    PointF::new(canvas_pt.x + 2.0, canvas_pt.y + 2.0),
                    color,
                    color,
                    sw,
                    op,
                );
                self.active_drag = Some(CanvasDragOp::CreateObject {
                    obj_idx: idx,
                    origin: canvas_pt,
                });
            }
            ToolMode::Callout | ToolMode::Text => {
                let existing_hit = self
                    .session
                    .page
                    .capture_objects
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, o)| {
                        matches!(o.tool_mode, Some(ToolMode::Text | ToolMode::Callout))
                            && hit_test_object(o, canvas_pt, 6.0)
                    })
                    .map(|(i, _)| i);
                if let Some(hit_idx) = existing_hit {
                    self.start_editing_text_object(hit_idx);
                } else {
                    let is_callout = self.active_tool == ToolMode::Callout;
                    let (fg_c, bg_c, tails) = if is_callout {
                        (
                            ColorRgba::rgb(24, 24, 27),
                            color,
                            Some(vec![PointF::new(canvas_pt.x + 36.0, canvas_pt.y + 94.0)]),
                        )
                    } else {
                        (color, ColorRgba::TRANSPARENT, None)
                    };
                    let obj = CaptureObject {
                        tool_mode: Some(self.active_tool.clone()),
                        start_point: Some(canvas_pt),
                        end_point: Some(PointF::new(canvas_pt.x + 190.0, canvas_pt.y + 64.0)),
                        tail_points: tails,
                        foreground_color: Some(fg_c),
                        background_color: Some(bg_c),
                        font_size: Some(self.font_size as f64),
                        stroke_width: Some(if is_callout { sw.max(2.0) } else { 0.0 }),
                        opacity: Some(op),
                        ..CaptureObject::default()
                    };
                    let idx = self.session.add_object_unrecorded(obj);
                    self.start_editing_text_object_with_baseline(idx, true);
                    self.active_drag = Some(CanvasDragOp::CreateObject {
                        obj_idx: idx,
                        origin: canvas_pt,
                    });
                }
            }
            ToolMode::Fill => {
                let x = canvas_pt.x.round().max(0.0) as u32;
                let y = canvas_pt.y.round().max(0.0) as u32;
                let _ = self.raster.apply_flood_fill(&mut self.session, x, y, color);
            }
            ToolMode::Eraser => {
                let before_bitmap = self.session.bitmap.clone();
                let points = vec![canvas_pt];
                let _ = self.raster.preview_eraser_stroke_from_base(
                    &mut self.session,
                    &before_bitmap,
                    &points,
                    ColorRgba::TRANSPARENT,
                );
                self.active_drag = Some(CanvasDragOp::EraseStroke {
                    before_bitmap,
                    points,
                });
            }
            ToolMode::Blur
            | ToolMode::Magnify
            | ToolMode::Crop
            | ToolMode::CutOut
            | ToolMode::Selection => {
                if self.active_tool == ToolMode::Selection {
                    self.active_selection_rect = None;
                }
                self.active_drag = Some(CanvasDragOp::RegionTool {
                    start: canvas_pt,
                    current: canvas_pt,
                });
            }
            ToolMode::Stamp => {
                let mut extra = std::collections::BTreeMap::new();
                extra.insert(
                    "StampId".to_string(),
                    serde_json::Value::String(self.selected_stamp_id.clone()),
                );
                let obj = CaptureObject {
                    tool_mode: Some(ToolMode::Stamp),
                    tool_shape: Some(self.selected_stamp_id.clone()),
                    start_point: Some(PointF::new(
                        (canvas_pt.x - 28.0).max(0.0),
                        (canvas_pt.y - 28.0).max(0.0),
                    )),
                    end_point: Some(PointF::new(canvas_pt.x + 28.0, canvas_pt.y + 28.0)),
                    foreground_color: Some(color),
                    stroke_width: Some(sw),
                    opacity: Some(op),
                    drop_shadow_enabled: Some(true),
                    extra,
                    ..CaptureObject::default()
                };
                let idx = self.session.add_object_unrecorded(obj);
                self.gizmos_g1.selected_indices = vec![idx];
                self.active_drag = Some(CanvasDragOp::CreateObject {
                    obj_idx: idx,
                    origin: canvas_pt,
                });
                self.status_message = format!("Placed Stamp `{}`", self.selected_stamp_id);
            }
            ToolMode::Eyedropper => {
                let x = canvas_pt.x.round().max(0.0) as u32;
                let y = canvas_pt.y.round().max(0.0) as u32;
                let _ = self.pick_canvas_color(x, y);
            }
            ToolMode::MagicWand => {
                let x = canvas_pt.x.round().max(0.0) as u32;
                let y = canvas_pt.y.round().max(0.0) as u32;
                let count = self.magic_wand.select_at(&self.session, x, y);
                self.status_message = format!(
                    "Magic Wand selected {count} px — press Delete to clear or use Properties"
                );
            }
            ToolMode::Simplify => {
                self.active_drag = Some(CanvasDragOp::RegionTool {
                    start: canvas_pt,
                    current: canvas_pt,
                });
            }
            _ => {}
        }
    }

    /// Update an active mouse gesture or hover state; returns `true` when the view should repaint.
    pub fn pointer_move(&mut self, local_x: f32, local_y: f32) -> bool {
        let is_brush_tool = matches!(
            self.active_tool,
            ToolMode::Pen | ToolMode::Highlighter | ToolMode::Eraser
        );
        let has_spot_gizmo = self.side_panel_tab == SidePanelTab::Effects
            && self.effects_gallery.active_spotlight_rect().is_some();
        if is_brush_tool || has_spot_gizmo {
            self.hover_local_pos = Some((local_x, local_y));
        } else if self.hover_local_pos.is_some() {
            self.hover_local_pos = None;
        }

        let Some(drag) = self.active_drag.clone() else {
            let mut needs_redraw = is_brush_tool || has_spot_gizmo;
            if matches!(self.active_tool, ToolMode::Move | ToolMode::SmartMove)
                && self.smart_move.enabled
            {
                let hover_pt = self.viewport.screen_to_canvas(local_x, local_y);
                let next_hover = self.smart_move.hit_test_at(hover_pt);
                if self.smart_move.hovered_idx != next_hover {
                    self.smart_move.hovered_idx = next_hover;
                    needs_redraw = true;
                }
            }
            return needs_redraw;
        };

        let canvas_pt = self.viewport.screen_to_canvas(local_x, local_y);

        match drag {
            CanvasDragOp::Pan { last_screen } => {
                self.viewport
                    .pan_by(local_x - last_screen.0, local_y - last_screen.1);
                self.active_drag = Some(CanvasDragOp::Pan {
                    last_screen: (local_x, local_y),
                });
            }
            CanvasDragOp::CreateObject { obj_idx, origin } => {
                if matches!(self.active_tool, ToolMode::Pen | ToolMode::Highlighter) {
                    self.gizmos_g1.append_stroke_point_unrecorded(
                        &mut self.session,
                        obj_idx,
                        canvas_pt,
                    );
                } else if let Some(mut obj) =
                    self.session.page.capture_objects.get(obj_idx).cloned()
                {
                    let drag_dist = (canvas_pt.x - origin.x).hypot(canvas_pt.y - origin.y);
                    let min_drag_met = (canvas_pt.x - origin.x).abs() >= 12.0
                        || (canvas_pt.y - origin.y).abs() >= 12.0;

                    match obj.tool_mode {
                        Some(ToolMode::Step) => {
                            if drag_dist >= 12.0 {
                                let r = drag_dist.clamp(12.0, 120.0);
                                obj.start_point = Some(PointF::new(origin.x - r, origin.y - r));
                                obj.end_point = Some(PointF::new(origin.x + r, origin.y + r));
                            }
                        }
                        Some(ToolMode::Stamp) => {
                            if min_drag_met {
                                obj.start_point = Some(origin);
                                obj.end_point = Some(canvas_pt);
                            }
                        }
                        Some(ToolMode::Text | ToolMode::Callout) => {
                            if min_drag_met {
                                obj.end_point = Some(canvas_pt);
                                if obj.tool_mode == Some(ToolMode::Callout) {
                                    let r = RectF::from_points(origin, canvas_pt);
                                    obj.tail_points = Some(vec![PointF::new(
                                        r.x + r.width * 0.25,
                                        r.y + r.height + 28.0,
                                    )]);
                                }
                            }
                        }
                        Some(ToolMode::Arrow) => {
                            obj.end_point = Some(canvas_pt);
                            obj.points_array = Some(vec![
                                origin,
                                PointF::new(
                                    (origin.x + canvas_pt.x) * 0.5,
                                    (origin.y + canvas_pt.y) * 0.5,
                                ),
                                canvas_pt,
                            ]);
                        }
                        _ => {
                            obj.end_point = Some(canvas_pt);
                        }
                    }
                    self.session.set_object_unrecorded(obj_idx, obj);
                }
            }
            CanvasDragOp::MoveObject {
                last_canvas,
                before_objects,
                smart_move_lift,
            } => {
                let dx = canvas_pt.x - last_canvas.x;
                let dy = canvas_pt.y - last_canvas.y;
                self.gizmos_g1
                    .translate_selected_unrecorded(&mut self.session, dx, dy);
                self.active_drag = Some(CanvasDragOp::MoveObject {
                    last_canvas: canvas_pt,
                    before_objects,
                    smart_move_lift,
                });
            }
            CanvasDragOp::EraseStroke {
                before_bitmap,
                mut points,
            } => {
                let moved_enough = points
                    .last()
                    .map_or(true, |prev| (canvas_pt.x - prev.x).hypot(canvas_pt.y - prev.y) >= 0.5);
                if moved_enough {
                    points.push(canvas_pt);
                    let _ = self.raster.preview_eraser_stroke_from_base(
                        &mut self.session,
                        &before_bitmap,
                        &points,
                        ColorRgba::TRANSPARENT,
                    );
                }
                self.active_drag = Some(CanvasDragOp::EraseStroke {
                    before_bitmap,
                    points,
                });
            }
            CanvasDragOp::RegionTool { start, .. } => {
                self.active_drag = Some(CanvasDragOp::RegionTool {
                    start,
                    current: canvas_pt,
                });
            }
            CanvasDragOp::EffectSpotlight {
                effect_idx,
                part,
                start_canvas,
                orig_rect,
                ..
            } => {
                let (bw, bh) = self.effects_gallery.base_dimensions(&self.session);
                let next_r = EffectsGalleryState::drag_spotlight_rect(
                    orig_rect,
                    part,
                    canvas_pt.x - start_canvas.x,
                    canvas_pt.y - start_canvas.y,
                    bw,
                    bh,
                );
                if let Some(EffectConfig::SpotlightAndMagnify {
                    dim_opacity,
                    blur_radius,
                    magnify_scale,
                    ..
                }) = self
                    .effects_gallery
                    .entries
                    .get(effect_idx)
                    .map(|e| e.config.clone())
                {
                    self.effects_gallery.update_effect_config(
                        &mut self.session,
                        effect_idx,
                        EffectConfig::SpotlightAndMagnify {
                            x: next_r.x,
                            y: next_r.y,
                            width: next_r.width,
                            height: next_r.height,
                            dim_opacity,
                            blur_radius,
                            magnify_scale,
                        },
                        false,
                    );
                    self.status_message = format!(
                        "Spotlight: ({:.0}, {:.0}) · {:.0}×{:.0} px",
                        next_r.x, next_r.y, next_r.width, next_r.height
                    );
                }
            }
        }
        true
    }

    /// Finish an active left-mouse gesture, committing **at most one** atomic [`HistoryCommand`].
    pub fn pointer_up(&mut self) {
        if let Some(drag) = self.active_drag.take() {
            match drag {
                CanvasDragOp::CreateObject { obj_idx, .. } => {
                    if let Some(object) = self.session.page.capture_objects.get(obj_idx).cloned() {
                        self.session
                            .history
                            .push(HistoryCommand::AddObject { index: obj_idx, object });
                    }
                }
                CanvasDragOp::MoveObject {
                    before_objects,
                    smart_move_lift,
                    ..
                } => {
                    if let Some((before_bmp, clipped, obj_idx)) = smart_move_lift {
                        if let Some(object) =
                            self.session.page.capture_objects.get(obj_idx).cloned()
                        {
                            let after_bmp = self.session.bitmap.clone();
                            self.session.history.push(HistoryCommand::Batch(vec![
                                HistoryCommand::RasterSnapshot {
                                    before: before_bmp,
                                    after: after_bmp,
                                    dirty_rect: clipped,
                                },
                                HistoryCommand::AddObject {
                                    index: obj_idx,
                                    object,
                                },
                            ]));
                        }
                    } else {
                        let mut updates = Vec::new();
                        for (idx, before) in before_objects {
                            if let Some(after) = self.session.page.capture_objects.get(idx).cloned()
                            {
                                if before != after {
                                    updates.push(HistoryCommand::UpdateObject {
                                        index: idx,
                                        before,
                                        after,
                                    });
                                }
                            }
                        }
                        if updates.len() == 1 {
                            self.session.history.push(updates.pop().expect("len == 1"));
                        } else if updates.len() > 1 {
                            self.session.history.push(HistoryCommand::Batch(updates));
                        }
                    }
                }
                CanvasDragOp::EraseStroke {
                    before_bitmap,
                    points,
                } => {
                    if let Some(dirty_rect) = self.raster.preview_eraser_stroke_from_base(
                        &mut self.session,
                        &before_bitmap,
                        &points,
                        ColorRgba::TRANSPARENT,
                    ) {
                        let after = self.session.bitmap.clone();
                        if before_bitmap != after {
                            self.session.history.push(HistoryCommand::RasterSnapshot {
                                before: before_bitmap,
                                after,
                                dirty_rect,
                            });
                        }
                    }
                }
                CanvasDragOp::EffectSpotlight {
                    before_bitmap,
                    before_objects,
                    ..
                } => {
                    if self.session.bitmap != before_bitmap {
                        let after_bitmap = self.session.bitmap.clone();
                        let after_objects = self.session.page.capture_objects.clone();
                        self.session.history.push(HistoryCommand::CanvasResize {
                            before_bitmap,
                            after_bitmap,
                            before_objects,
                            after_objects,
                        });
                    }
                }
                CanvasDragOp::RegionTool { start, current } => {
                    let cw_max = self.session.bitmap.width().max(1) as f64;
                    let ch_max = self.session.bitmap.height().max(1) as f64;
                    let x = start.x.min(current.x).clamp(0.0, cw_max - 1.0);
                    let y = start.y.min(current.y).clamp(0.0, ch_max - 1.0);
                    let x1 = start.x.max(current.x).clamp(x + 1.0, cw_max);
                    let y1 = start.y.max(current.y).clamp(y + 1.0, ch_max);
                    let w = x1 - x;
                    let h = y1 - y;
                    if w >= 4.0 && h >= 4.0 {
                        let rect = RectF::new(x, y, w, h);
                        match self.active_tool {
                            ToolMode::Blur => {
                                self.raster.place_blur_region(&mut self.session, rect);
                                self.status_message =
                                    format!("Blurred {:.0}×{:.0} px region", w, h);
                            }
                            ToolMode::Magnify => {
                                self.raster.place_magnify_region(&mut self.session, rect);
                                self.status_message =
                                    format!("Magnified {:.0}×{:.0} px region", w, h);
                            }
                            ToolMode::Crop => {
                                self.pending_crop_rect = Some(rect);
                                self.status_message = format!(
                                    "Crop {:.0}×{:.0} px ready — click ✓ Crop or press Enter",
                                    w, h
                                );
                            }
                            ToolMode::CutOut => {
                                if w >= h {
                                    self.raster.cutout.orientation = CutOutOrientation::Horizontal;
                                    self.raster.cutout.start = y.round().max(0.0) as u32;
                                    self.raster.cutout.end = (y + h).round().max(1.0) as u32;
                                } else {
                                    self.raster.cutout.orientation = CutOutOrientation::Vertical;
                                    self.raster.cutout.start = x.round().max(0.0) as u32;
                                    self.raster.cutout.end = (x + w).round().max(1.0) as u32;
                                }
                                if self.raster.apply_cut_out(&mut self.session) {
                                    self.status_message = "Applied Cut Out strip".to_string();
                                }
                            }
                            ToolMode::Selection => {
                                self.active_selection_rect = Some(rect);
                                self.raster.selection_points =
                                    vec![PointF::new(x, y), PointF::new(x + w, y + h)];
                                self.status_message = format!(
                                    "Selected {:.0}×{:.0} px (Del to clear, Ctrl+C to copy)",
                                    w, h
                                );
                            }
                            ToolMode::Simplify => {
                                let fill_col = self
                                    .simplify_ui
                                    .palette
                                    .palette_colors
                                    .first()
                                    .copied()
                                    .unwrap_or_else(|| self.active_color_rgba());
                                let obj = CaptureObject {
                                    tool_mode: Some(ToolMode::Simplify),
                                    start_point: Some(PointF::new(x, y)),
                                    end_point: Some(PointF::new(x + w, y + h)),
                                    background_color: Some(fill_col),
                                    foreground_color: Some(ColorRgba::TRANSPARENT),
                                    stroke_width: Some(0.0),
                                    corner_radius_ratio: Some(0.12),
                                    opacity: Some(100.0),
                                    ..CaptureObject::default()
                                };
                                let idx = self.session.add_object(obj);
                                self.gizmos_g1.selected_indices = vec![idx];
                                self.status_message = format!(
                                    "Placed {:.0}×{:.0} px Simplify block (Ctrl+Z to undo)",
                                    w, h
                                );
                            }
                            _ => {}
                        }
                    } else if self.active_tool == ToolMode::Selection {
                        if self.active_selection_rect.take().is_some() {
                            self.raster.selection_points.clear();
                            self.status_message = "Selection cleared".to_string();
                        }
                    }
                }
                CanvasDragOp::Pan { .. } => {}
            }
        }
        self.viewport.active_guides.clear();
    }

    /// Confirm and execute `pending_crop_rect` (triggered by `✓ Crop` button or `Enter` key).
    pub fn confirm_pending_crop(&mut self) -> bool {
        let Some(rect) = self.pending_crop_rect.take() else {
            return false;
        };
        let cw = self.session.bitmap.width().max(1);
        let ch = self.session.bitmap.height().max(1);
        let x = (rect.x.round().max(0.0) as u32).min(cw.saturating_sub(1));
        let y = (rect.y.round().max(0.0) as u32).min(ch.saturating_sub(1));
        let w = (rect.width.round().max(1.0) as u32).min(cw.saturating_sub(x)).max(1);
        let h = (rect.height.round().max(1.0) as u32).min(ch.saturating_sub(y)).max(1);

        self.raster.crop.rect = PixelRect::new(x, y, w, h);
        if self.raster.apply_crop(&mut self.session) {
            let nw = self.session.bitmap.width();
            let nh = self.session.bitmap.height();
            self.viewport.fit_canvas(nw, nh);
            self.zoom_percent = self.viewport.zoom_percent();
            self.status_message = format!("Cropped canvas to {nw}×{nh} px");
            true
        } else {
            false
        }
    }

    pub fn cancel_pending_crop(&mut self) {
        self.pending_crop_rect = None;
        self.status_message = "Canceled Crop".to_string();
    }

    pub fn delete_selected_region(&mut self) -> bool {
        let Some(sel) = self.active_selection_rect.take() else {
            return false;
        };
        let cw = self.session.bitmap.width();
        let ch = self.session.bitmap.height();
        let x = (sel.x.round().max(0.0) as u32).min(cw.saturating_sub(1));
        let y = (sel.y.round().max(0.0) as u32).min(ch.saturating_sub(1));
        let w = (sel.width.round().max(1.0) as u32).min(cw.saturating_sub(x)).max(1);
        let h = (sel.height.round().max(1.0) as u32).min(ch.saturating_sub(y)).max(1);
        let pr = PixelRect::new(x, y, w, h);
        self.session.mutate_raster_region(pr, |_, _, px| {
            *px = [255, 255, 255, 255];
        });
        self.status_message = format!("Cleared {w}×{h} px selection");
        true
    }

    pub fn copy_selected_region_to_clipboard(&mut self) -> Result<(), String> {
        let Some(sel) = self.active_selection_rect else {
            return self.copy_all_to_clipboard();
        };
        let comp = SceneCompositor::new(&self.session.page, Some(&self.session.bitmap));
        let (fw, fh, rgba) = comp
            .flatten_to_rgba()
            .map_err(|e| format!("flatten failed: {e}"))?;
        let x = (sel.x.round().max(0.0) as u32).min(fw.saturating_sub(1));
        let y = (sel.y.round().max(0.0) as u32).min(fh.saturating_sub(1));
        let w = (sel.width.round().max(1.0) as u32).min(fw.saturating_sub(x)).max(1);
        let h = (sel.height.round().max(1.0) as u32).min(fh.saturating_sub(y)).max(1);
        let mut sub = vec![0u8; (w as usize) * (h as usize) * 4];
        for row in 0..h as usize {
            let s_off = ((y as usize + row) * fw as usize + x as usize) * 4;
            let d_off = row * (w as usize) * 4;
            sub[d_off..d_off + (w as usize) * 4]
                .copy_from_slice(&rgba[s_off..s_off + (w as usize) * 4]);
        }
        let img = snagsoup_win_platform::clipboard::RgbaImage {
            width: w,
            height: h,
            data: sub,
        };
        snagsoup_win_platform::clipboard::write_image_to_clipboard(&img)
            .map_err(|e| format!("clipboard write failed: {e}"))?;
        self.status_message = format!("Copied {w}×{h} px selection to Clipboard");
        Ok(())
    }
    /// Clamp zoom to Snagit's `10% – 800%` range.
    pub fn set_zoom(&mut self, percent: f32) {
        self.zoom_percent = percent.clamp(10.0, 800.0);
        self.viewport.zoom = self.zoom_percent / 100.0;
    }

    /// Reorder a tool in the customizable top toolbar.
    pub fn reorder_tool(&mut self, from: usize, to: usize) -> bool {
        if from < self.tools.len() && to < self.tools.len() && from != to {
            let entry = self.tools.remove(from);
            self.tools.insert(to, entry);
            true
        } else {
            false
        }
    }

    /// Upsert an entry in the Recent Captures Tray without ever creating duplicate cards
    /// for the same file path, removing any synthetic `"untitled-1"` placeholder once real files exist.
    pub fn upsert_recent_capture(&mut self, item: RecentCaptureItem, move_to_front: bool) {
        if item.path.is_some() {
            self.recent_captures.retain(|existing| existing.path.is_some());
        }
        if let Some(pos) = self.recent_captures.iter().position(|x| x.id == item.id) {
            self.recent_captures[pos].title = item.title;
            self.recent_captures[pos].dimensions = item.dimensions;
            self.recent_captures[pos].is_video = item.is_video;
            if item.path.is_some() {
                self.recent_captures[pos].path = item.path;
            }
            if item.thumbnail.is_some() {
                self.recent_captures[pos].thumbnail = item.thumbnail;
            }
            if move_to_front && pos > 0 {
                let updated = self.recent_captures.remove(pos);
                self.recent_captures.insert(0, updated);
            }
        } else {
            self.recent_captures.insert(0, item);
            if self.recent_captures.len() > 24 {
                self.recent_captures.truncate(24);
            }
        }
    }

    /// Scan `dir` for the most recently modified `.snagx` and `.mp4` captures and populate
    /// `recent_captures` with real decoded thumbnails.
    pub fn load_recent_captures_from_dir(&mut self, dir: &Path, limit: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<(SystemTime, PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let ext = p.extension()?.to_str()?;
                if !ext.eq_ignore_ascii_case("snagx") && !ext.eq_ignore_ascii_case("mp4") {
                    return None;
                }
                let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
                Some((mtime, p))
            })
            .collect();
        entries.sort_by(|a, b| b.0.cmp(&a.0));

        let mut loaded = Vec::new();
        for (_, p) in entries.into_iter().take(limit.max(1)) {
            if let Some(item) = load_recent_item_from_path(&p) {
                loaded.push(item);
            }
        }
        if !loaded.is_empty() {
            self.recent_captures = loaded;
        }
    }

    /// Open a `.snagx` archive into `DocumentSession`.
    pub fn open_snagx(&mut self, path: &Path) -> Result<(), String> {
        let archive =
            SnagxArchive::from_file(path).map_err(|e| format!("failed to open .snagx: {e}"))?;
        let first = archive
            .pages
            .first()
            .ok_or_else(|| "archive contains no pages".to_string())?;

        let bmp = if let Some(ref png) = first.background_png {
            TiledBitmap::from_png_bytes(png)
                .map_err(|e| format!("failed to decode page PNG: {e}"))?
        } else {
            let w = first.page.capture_canvas_width.unwrap_or(800.0).max(1.0) as u32;
            let h = first.page.capture_canvas_height.unwrap_or(600.0).max(1.0) as u32;
            TiledBitmap::new_filled(
                w,
                h,
                first.page.capture_background_color.unwrap_or(ColorRgba::WHITE),
            )
        };

        let dims = (bmp.width(), bmp.height());
        let thumb = archive
            .thumbnail_png
            .as_deref()
            .and_then(make_tray_thumb_from_png_bytes)
            .or_else(|| make_tray_thumb_from_bitmap(&bmp));

        self.finish_text_editing();
        self.active_drag = None;
        self.video_player.close_video();
        self.effects_gallery.reset_for_new_document(dims.0, dims.1);
        self.applied_effects_count = 0;
        self.session = DocumentSession::new(first.page.clone(), bmp);
        self.gizmos_g1
            .step_counter
            .sync_with_objects(&self.session.page.capture_objects);
        self.simplify_ui.sync_from_session(&self.session);
        self.magic_wand.clear_selection();
        self.viewport.fit_canvas(dims.0, dims.1);
        self.zoom_percent = self.viewport.zoom_percent();
        self.pending_fit_canvas = true;
        self.metadata = archive.metadata;
        self.active_path = Some(path.to_path_buf());
        self.workspace_view = WorkspaceView::Editor;
        self.status_message = format!("Opened {}", path.display());

        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Capture.snagx")
            .to_string();
        self.upsert_recent_capture(
            RecentCaptureItem {
                id: normalize_capture_id(path),
                title,
                path: Some(path.to_path_buf()),
                dimensions: dims,
                thumbnail: thumb,
                is_video: false,
            },
            false,
        );
        Ok(())
    }

    /// Save the active `DocumentSession` to a `.snagx` archive (with regenerated thumbnail PNG).
    pub fn save_snagx(&mut self, path: &Path) -> Result<(), String> {
        self.finish_text_editing();
        let bg_png = self
            .session
            .bitmap
            .to_png_bytes()
            .map_err(|e| format!("failed to encode background PNG: {e}"))?;
        let comp = SceneCompositor::new(&self.session.page, Some(&self.session.bitmap));
        let thumb_png = comp
            .generate_thumbnail_png(256, 256)
            .map_err(|e| format!("failed to generate thumbnail: {e}"))?;
        let tray_thumb = make_tray_thumb_from_png_bytes(&thumb_png)
            .or_else(|| make_tray_thumb_from_bitmap(&self.session.bitmap));

        let page_filename = "{SNAGSOUP-PAGE-0001}.json".to_string();
        let archive = SnagxArchive {
            index: SnagxIndex {
                pages: vec![page_filename.clone()],
                version: "1.0".to_string(),
                extra: Default::default(),
            },
            metadata: self.metadata.clone(),
            thumbnail_png: Some(thumb_png),
            pages: vec![SnagxPageEntry {
                page_filename,
                page: self.session.page.clone(),
                background_png: Some(bg_png),
                backup_json: None,
            }],
            extra_entries: Default::default(),
        };

        archive
            .to_file(path)
            .map_err(|e| format!("failed to write .snagx: {e}"))?;
        self.active_path = Some(path.to_path_buf());
        self.status_message = format!("Saved {}", path.display());

        // Always index saved .snagx into persistent SQLite FTS5 LibraryDb immediately!
        let db_path = library_db_path();
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut db) = LibraryDb::open_file(&db_path) {
            let _ = db.index_snagx_file(path);
        }
        let dims = (self.session.bitmap.width(), self.session.bitmap.height());
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Capture.snagx")
            .to_string();
        self.upsert_recent_capture(
            RecentCaptureItem {
                id: normalize_capture_id(path),
                title,
                path: Some(path.to_path_buf()),
                dimensions: dims,
                thumbnail: tray_thumb,
                is_video: false,
            },
            false,
        );
        Ok(())
    }

    /// Flatten the active canvas and copy BOTH `"PNG"` (alpha) + `CF_DIBV5` to the Windows Clipboard.
    pub fn copy_all_to_clipboard(&mut self) -> Result<(), String> {
        let comp = SceneCompositor::new(&self.session.page, Some(&self.session.bitmap));
        let (w, h, rgba) = comp
            .flatten_to_rgba()
            .map_err(|e| format!("failed to flatten canvas: {e}"))?;
        let img = snagsoup_win_platform::clipboard::RgbaImage {
            width: w,
            height: h,
            data: rgba,
        };
        snagsoup_win_platform::clipboard::write_image_to_clipboard(&img)
            .map_err(|e| format!("clipboard write failed: {e}"))?;
        self.status_message = format!("Copied {w}×{h} image to Clipboard");
        Ok(())
    }

    /// Activate temporary `"Copied"` button state and return its monotonic sequence token
    /// so the 2s auto-revert timer only clears its own activation.
    pub fn activate_copy_feedback(&mut self) -> u64 {
        self.copy_feedback_seq = self.copy_feedback_seq.wrapping_add(1);
        self.copy_feedback_active = true;
        self.copy_feedback_seq
    }

    /// Copy the flattened canvas to Clipboard and activate temporary `"Copied"` button state,
    /// returning the monotonic sequence token so the 2s timer only clears its own activation.
    pub fn trigger_copy_all_with_feedback(&mut self) -> Result<u64, String> {
        self.copy_all_to_clipboard()?;
        Ok(self.activate_copy_feedback())
    }

    /// Revert `"Copied"` back to `"Copy All"` if no newer copy click occurred since `seq`.
    pub fn clear_copy_feedback_if_seq(&mut self, seq: u64) -> bool {
        if self.copy_feedback_active && self.copy_feedback_seq == seq {
            self.copy_feedback_active = false;
            true
        } else {
            false
        }
    }

    /// Load a freshly captured bitmap + optional movable cursor into the Editor session
    /// instantaneously (`< 1ms` on the UI thread), prepend to the Recent Captures Tray,
    /// and asynchronously persist `.snagx` + `LibraryDb` index on a background thread.
    pub fn load_captured_bitmap(
        &mut self,
        bmp: TiledBitmap,
        cursor_obj: Option<CaptureObject>,
    ) {
        let w = bmp.width();
        let h = bmp.height();
        let mut objects = Vec::new();
        if let Some(cursor) = cursor_obj {
            objects.push(cursor);
        }
        let page = SnagxPage {
            capture_canvas_width: Some(w as f64),
            capture_canvas_height: Some(h as f64),
            dpi: Some(96.0),
            capture_background_color: Some(ColorRgba::WHITE),
            capture_objects: objects,
            ..SnagxPage::default()
        };
        self.finish_text_editing();
        self.active_drag = None;
        self.video_player.close_video();
        self.effects_gallery.reset_for_new_document(w, h);
        self.applied_effects_count = 0;
        self.session = DocumentSession::new(page.clone(), bmp.clone());
        self.gizmos_g1
            .step_counter
            .sync_with_objects(&self.session.page.capture_objects);
        self.simplify_ui.sync_from_session(&self.session);
        self.magic_wand.clear_selection();
        self.viewport.fit_canvas(w, h);
        self.zoom_percent = self.viewport.zoom_percent();
        self.pending_fit_canvas = true;
        self.workspace_view = WorkspaceView::Editor;

        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let ts_secs = (ts_ms / 1000) as u64;
        let dir = capture_storage_dir();
        let file_name = format!("SnagSoup_{ts_ms}.snagx");
        let save_path = dir.join(&file_name);
        let save_path_str = save_path.display().to_string();

        let tray_thumb = make_tray_thumb_from_bitmap(&self.session.bitmap);
        self.active_path = Some(save_path.clone());
        self.upsert_recent_capture(
            RecentCaptureItem {
                id: normalize_capture_id(&save_path),
                title: file_name.clone(),
                path: Some(save_path.clone()),
                dimensions: (w, h),
                thumbnail: tray_thumb,
                is_video: false,
            },
            true,
        );
        self.status_message = format!("Captured {w}×{h} & auto-saved to Library");

        let metadata = self.metadata.clone();
        let _ = std::thread::Builder::new()
            .name("snagsoup-autosave".into())
            .spawn(move || {
                let _ = std::fs::create_dir_all(&dir);
                let Ok(bg_png) = bmp.to_png_bytes() else {
                    return;
                };
                let comp = SceneCompositor::new(&page, Some(&bmp));
                let thumb_png = comp.generate_thumbnail_png(256, 256).ok();
                let page_filename = "{SNAGSOUP-PAGE-0001}.json".to_string();
                let archive = SnagxArchive {
                    index: SnagxIndex {
                        pages: vec![page_filename.clone()],
                        version: "1.0".to_string(),
                        extra: Default::default(),
                    },
                    metadata,
                    thumbnail_png: thumb_png,
                    pages: vec![SnagxPageEntry {
                        page_filename,
                        page,
                        background_png: Some(bg_png),
                        backup_json: None,
                    }],
                    extra_entries: Default::default(),
                };
                if archive.to_file(&save_path).is_ok() {
                    let db_path = library_db_path();
                    if let Some(parent) = db_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Ok(mut db) = LibraryDb::open_file(&db_path) {
                        let _ = db.upsert_item(&LibraryItem {
                            id: 0,
                            file_path: save_path_str,
                            media_type: MediaType::Image,
                            capture_date: format!("{ts_secs}"),
                            app_name: "SnagSoup".into(),
                            window_title: file_name,
                            web_url: String::new(),
                            callout_text: String::new(),
                            ocr_text: String::new(),
                            width: w,
                            height: h,
                            tags: vec!["capture".into()],
                        });
                    }
                }
            });
    }

    /// Open a recorded `.mp4` video in the Editor Video Player, index in `LibraryDb` if needed,
    /// and prepend to the Recent Captures Tray.
    pub fn open_video_file(&mut self, path: &Path) -> Result<(), String> {
        let meta = self.video_player.open_video(path)?;
        self.active_path = Some(path.to_path_buf());
        self.workspace_view = WorkspaceView::Editor;
        self.status_message = format!(
            "Opened video {} ({}×{}, {:.1}s)",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("video.mp4"),
            meta.width,
            meta.height,
            meta.duration_ms as f64 / 1000.0
        );

        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Recording.mp4")
            .to_string();
        let tray_thumb = self
            .video_player
            .current_frame
            .as_ref()
            .and_then(|f| make_tray_thumb_from_rgba(f.width, f.height, &f.rgba))
            .or_else(|| {
                snagsoup_media::extract_video_frame_rgba(path, 0)
                    .ok()
                    .and_then(|f| make_tray_thumb_from_rgba(f.width, f.height, &f.rgba))
            });
        self.upsert_recent_capture(
            RecentCaptureItem {
                id: normalize_capture_id(path),
                title,
                path: Some(path.to_path_buf()),
                dimensions: (meta.width, meta.height),
                thumbnail: tray_thumb,
                is_video: true,
            },
            false,
        );
        Ok(())
    }

    /// Save to `active_path` (or auto-generate a timestamped `.snagx` in `capture_storage_dir`).
    pub fn quick_save(&mut self) -> Result<PathBuf, String> {
        let target = if let Some(ref p) = self.active_path {
            p.clone()
        } else {
            let dir = capture_storage_dir();
            let _ = std::fs::create_dir_all(&dir);
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            dir.join(format!("SnagSoup_{ts}.snagx"))
        };
        self.save_snagx(&target)?;
        Ok(target)
    }

    /// Current selected stroke/fill color as [`ColorRgba`].
    pub fn selected_color_rgba(&self) -> ColorRgba {
        ColorRgba::rgb(
            ((self.selected_color >> 16) & 0xFF) as u8,
            ((self.selected_color >> 8) & 0xFF) as u8,
            (self.selected_color & 0xFF) as u8,
        )
    }

    /// Apply an [`EffectConfig`] to the active document with Undo support.
    pub fn apply_effect(&mut self, effect: &EffectConfig, label: &str) {
        if self.session.apply_effect(effect) {
            self.applied_effects_count += 1;
            self.status_message = format!("Applied effect: {label}");
        }
    }

    /// Reset to a clean blank white canvas (`width × height`) in Image Editor mode.
    pub fn create_new_blank_canvas(&mut self, width: u32, height: u32) {
        let w = width.max(1);
        let h = height.max(1);
        let bmp = TiledBitmap::new_filled(w, h, ColorRgba::WHITE);
        let page = SnagxPage {
            capture_canvas_width: Some(w as f64),
            capture_canvas_height: Some(h as f64),
            dpi: Some(96.0),
            capture_background_color: Some(ColorRgba::WHITE),
            ..SnagxPage::default()
        };
        self.finish_text_editing();
        self.active_drag = None;
        self.video_player.close_video();
        self.effects_gallery.reset_for_new_document(w, h);
        self.applied_effects_count = 0;
        self.session = DocumentSession::new(page, bmp);
        self.gizmos_g1.step_counter.sync_with_objects(&[]);
        self.simplify_ui.sync_from_session(&self.session);
        self.magic_wand.clear_selection();
        self.active_path = None;
        self.viewport.fit_canvas(w, h);
        self.zoom_percent = self.viewport.zoom_percent();
        self.pending_fit_canvas = true;
        self.workspace_view = WorkspaceView::Editor;
        self.status_message = format!("Created new {w}×{h} image");
    }

    /// Pick a pixel color at `(x, y)` on the active bitmap (`Eyedropper` tool).
    pub fn pick_canvas_color(&mut self, x: u32, y: u32) -> Option<u32> {
        let [r, g, b, _] = self.session.bitmap.get_pixel(x, y)?;
        let hex = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        self.selected_color = hex;
        self.status_message = format!("Picked color #{hex:06X} at ({x}, {y})");
        Some(hex)
    }

    /// Run Offline Multilingual Neural OCR `Grab Text` on the active selection (if any) or full
    /// flattened canvas, copy the extracted text to the Clipboard, and open the Grab Text modal.
    pub fn grab_text_to_clipboard(&mut self) -> Result<String, String> {
        let sel = self.active_selection_rect;
        let text = self
            .grab_edit_text
            .run_grab_text_region(&self.session, sel)?
            .to_string();
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            let _ = snagsoup_win_platform::clipboard::write_text_to_clipboard(&trimmed);
            self.grab_edit_text.copied_feedback = true;
            let line_cnt = trimmed.lines().count();
            self.status_message = format!(
                "Grabbed {} chars ({} lines) from {} & copied to Clipboard",
                trimmed.chars().count(),
                line_cnt,
                self.grab_edit_text.scope_label
            );
        } else {
            self.grab_edit_text.copied_feedback = false;
            self.status_message = "OCR completed: no text detected in image".to_string();
        }
        Ok(trimmed)
    }

    /// Export flattened canvas to `png`, `jpg`, `pdf`, or `svg` inside `Exports/`.
    pub fn export_flattened_as(&mut self, ext: &str) -> Result<PathBuf, String> {
        let comp = SceneCompositor::new(&self.session.page, Some(&self.session.bitmap));
        let (w, h, rgba) = comp
            .flatten_to_rgba()
            .map_err(|e| format!("flatten failed: {e}"))?;
        let base = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("SnagSoup")
            .join("Exports");
        let _ = std::fs::create_dir_all(&base);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let out_path = base.join(format!("SnagSoup_{ts}.{ext}"));
        let bytes = match ext {
            "png" => export_raster_bytes(w, h, &rgba, RasterExportFormat::Png)
                .map_err(|e| e.to_string())?,
            "jpg" => export_raster_bytes(w, h, &rgba, RasterExportFormat::Jpeg { quality: 90 })
                .map_err(|e| e.to_string())?,
            "pdf" => export_pdf_bytes(&[(w, h, rgba)], PdfPageSize::A4)
                .map_err(|e| e.to_string())?,
            "svg" => export_svg_string(&self.session.page, Some(&self.session.bitmap))
                .map_err(|e| e.to_string())?
                .into_bytes(),
            other => return Err(format!("unsupported format: {other}")),
        };
        std::fs::write(&out_path, bytes).map_err(|e| e.to_string())?;
        self.status_message = format!("Exported {}", out_path.display());
        Ok(out_path)
    }

    /// Dispatch flattened canvas to a local `ShareTargetKind` (`Clipboard`, `Word`, `PPT`, `Excel`, `Email`, `Printer`).
    pub fn share_flattened_to(&mut self, target: ShareTargetKind) -> Result<String, String> {
        let comp = SceneCompositor::new(&self.session.page, Some(&self.session.bitmap));
        let (w, h, rgba) = comp
            .flatten_to_rgba()
            .map_err(|e| format!("flatten failed: {e}"))?;
        let out_dir = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("SnagSoup")
            .join("Exports");
        let mut sm = ShareMenuView::new(out_dir, None);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let receipt = sm.execute_share(target, w, h, &rgba, &format!("SnagSoup_{ts}"))?;
        self.status_message = receipt.summary.clone();
        Ok(receipt.summary)
    }
}

/// Full Snagit 2022 GPUI Editor Window Shell (`W3-05`..`W3-16`).
pub struct EditorShellView {
    pub state: EditorShellState,
    focus_handle: Option<FocusHandle>,
    canvas_origin: Rc<Cell<(f32, f32)>>,
    canvas_size_cell: Rc<Cell<(f32, f32)>>,
    create_btn_bounds: Rc<Cell<(f32, f32, f32)>>,
    more_btn_bounds: Rc<Cell<(f32, f32, f32)>>,
    share_btn_bounds: Rc<Cell<(f32, f32, f32)>>,
    library_view: Option<Entity<LibraryOrganizerView>>,
    template_view: Option<Entity<TemplateBuilderView>>,
    combine_view: Option<Entity<CombineImagesView>>,
    vfi_view: Option<Entity<VideoFromImagesView>>,
    xliff_view: Option<Entity<XliffTranslationView>>,
}

impl Default for EditorShellView {
    fn default() -> Self {
        Self::new(None)
    }
}

impl EntityInputHandler for EditorShellView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.state.inline_text.range_from_utf16(range_utf16);
        *adjusted_range = Some(self.state.inline_text.range_to_utf16(range.clone()));
        Some(self.state.inline_text.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self
                .state
                .inline_text
                .range_to_utf16(self.state.inline_text.selected_range.clone()),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.state
            .inline_text
            .marked_range
            .clone()
            .map(|r| self.state.inline_text.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.state.inline_text.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.editing_text_obj.is_none() {
            return;
        }
        // Ignore raw control chars (Backspace/Enter/Esc handled in `on_key_down`)
        let filtered: String = new_text
            .chars()
            .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
            .collect();
        if filtered.is_empty() && range_utf16.is_none() && self.state.inline_text.marked_range.is_none() {
            return;
        }
        self.state
            .inline_text
            .replace_utf16_range(range_utf16, &filtered, false, None);
        self.state.sync_active_text_to_session();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.editing_text_obj.is_none() {
            return;
        }
        self.state.inline_text.replace_utf16_range(
            range_utf16,
            new_text,
            true,
            new_selected_range_utf16,
        );
        self.state.sync_active_text_to_session();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(
            gpui::point(element_bounds.origin.x, element_bounds.origin.y),
            gpui::size(px(2.0), px(22.0)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(
            self.state
                .inline_text
                .offset_to_utf16(self.state.inline_text.selected_range.end),
        )
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.state.editing_text_obj.is_some()
    }
}

impl EditorShellView {
    pub fn new(initial_file: Option<PathBuf>) -> Self {
        let mut state = EditorShellState::default();
        state.load_recent_captures_from_dir(&capture_storage_dir(), 16);
        let file_to_open = initial_file.or_else(|| {
            state
                .recent_captures
                .first()
                .and_then(|item| item.path.clone())
        });
        if let Some(path) = file_to_open {
            let is_mp4 = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mp4"))
                .unwrap_or(false);
            let res = if is_mp4 {
                state.open_video_file(&path)
            } else {
                state.open_snagx(&path)
            };
            if let Err(err) = res {
                state.status_message = err;
            }
        }
        Self {
            state,
            focus_handle: None,
            canvas_origin: Rc::new(Cell::new((0.0, 44.0))),
            canvas_size_cell: Rc::new(Cell::new((0.0, 0.0))),
            create_btn_bounds: Rc::new(Cell::new((350.0, 38.0, 80.0))),
            more_btn_bounds: Rc::new(Cell::new((640.0, 38.0, 68.0))),
            share_btn_bounds: Rc::new(Cell::new((1180.0, 38.0, 76.0))),
            library_view: None,
            template_view: None,
            combine_view: None,
            vfi_view: None,
            xliff_view: None,
        }
    }

    fn ensure_library_view(&mut self, cx: &mut Context<Self>) -> Entity<LibraryOrganizerView> {
        if let Some(ref view) = self.library_view {
            return view.clone();
        }
        let db_path = library_db_path();
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let db = LibraryDb::open_file(&db_path)
            .or_else(|_| LibraryDb::open_in_memory())
            .expect("LibraryDb");
        let weak_editor = cx.entity().downgrade();
        let weak_editor_del = weak_editor.clone();
        let view = cx.new(move |_| {
            let mut lib = LibraryOrganizerView::new(db)
                .with_on_open_item(move |path, _window, cx| {
                    let _ = weak_editor.update(cx, |ed, cx| {
                        let is_mp4 = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.eq_ignore_ascii_case("mp4"))
                            .unwrap_or(false);
                        let res = if is_mp4 {
                            ed.state.open_video_file(&path)
                        } else {
                            ed.state.open_snagx(&path)
                        };
                        if let Err(err) = res {
                            ed.state.status_message = err;
                        }
                        ed.state.workspace_view = WorkspaceView::Editor;
                        cx.notify();
                    });
                })
                .with_on_items_deleted(move |deleted_paths, cx| {
                    let _ = weak_editor_del.update(cx, |ed, cx| {
                        ed.state.recent_captures.retain(|item| {
                            !deleted_paths.iter().any(|dp| {
                                let norm_dp = normalize_capture_id(Path::new(dp));
                                item.id == norm_dp
                                    || item
                                        .path
                                        .as_ref()
                                        .is_some_and(|p| normalize_capture_id(p) == norm_dp)
                            })
                        });
                        if ed
                            .state
                            .active_path
                            .as_ref()
                            .is_some_and(|ap| {
                                let norm_ap = normalize_capture_id(ap);
                                deleted_paths
                                    .iter()
                                    .any(|dp| normalize_capture_id(Path::new(dp)) == norm_ap)
                            })
                        {
                            ed.state.active_path = None;
                        }
                        cx.notify();
                    });
                });
            lib.sync_directory(&capture_storage_dir());
            lib
        });
        self.library_view = Some(view.clone());
        view
    }

    fn ensure_template_view(&mut self, cx: &mut Context<Self>) -> Entity<TemplateBuilderView> {
        if let Some(ref view) = self.template_view {
            return view.clone();
        }
        let view = cx.new(|_| TemplateBuilderView::new(SnagTemplate::default(), "Basic 2 Steps"));
        self.template_view = Some(view.clone());
        view
    }

    fn ensure_combine_view(&mut self, cx: &mut Context<Self>) -> Entity<CombineImagesView> {
        if let Some(ref view) = self.combine_view {
            return view.clone();
        }
        let bmp = self.state.session.bitmap.clone();
        let view = cx.new(|_| CombineImagesView::new(vec![("Current Capture".into(), bmp)]));
        self.combine_view = Some(view.clone());
        view
    }

    fn ensure_vfi_view(&mut self, cx: &mut Context<Self>) -> Entity<VideoFromImagesView> {
        if let Some(ref view) = self.vfi_view {
            return view.clone();
        }
        let bmp = self.state.session.bitmap.clone();
        let slide = SlideItem {
            title: "Slide 1".into(),
            width: bmp.width(),
            height: bmp.height(),
            rgba: bmp.to_rgba(),
        };
        let view = cx.new(|_| VideoFromImagesView::new(vec![slide], 1280, 720, 30));
        self.vfi_view = Some(view.clone());
        view
    }

    fn ensure_xliff_view(&mut self, cx: &mut Context<Self>) -> Entity<XliffTranslationView> {
        if let Some(ref view) = self.xliff_view {
            return view.clone();
        }
        let archive = SnagxArchive {
            index: SnagxIndex {
                pages: vec!["{PAGE}.json".into()],
                version: "1.0".into(),
                extra: Default::default(),
            },
            metadata: self.state.metadata.clone(),
            thumbnail_png: None,
            pages: vec![SnagxPageEntry {
                page_filename: "{PAGE}.json".into(),
                page: self.state.session.page.clone(),
                background_png: None,
                backup_json: None,
            }],
            extra_entries: Default::default(),
        };
        let view = cx.new(|_| XliffTranslationView::new(archive, "en-US", "vi-VN"));
        self.xliff_view = Some(view.clone());
        view
    }

    /// Launch Offline Multilingual Neural OCR (`Grab Text`) asynchronously on a background worker
    /// thread while displaying a live stage/percentage progress bar in the floating modal.
    pub fn start_grab_text_async(&mut self, cx: &mut Context<Self>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        enum OcrWorkerEvent {
            Progress(f32, String),
            Finished(Result<String, snagsoup_cv_ocr::CvOcrError>),
        }

        self.state.grab_edit_text.cancel_running_job();
        let (rw, rh, rgba, scope) = super::smart_cv_ui::GrabEditTextUiState::prepare_grab_text_buffer(
            &self.state.session,
            self.state.active_selection_rect,
        );

        let job_id = self.state.grab_edit_text.active_job_id.wrapping_add(1);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.state.grab_edit_text.active_job_id = job_id;
        self.state.grab_edit_text.cancel_flag = Some(Arc::clone(&cancel_flag));
        self.state.grab_edit_text.is_running = true;
        self.state.grab_edit_text.progress = 0.04;
        self.state.grab_edit_text.progress_stage = "Starting OCR engine...".to_string();
        self.state.grab_edit_text.scope_label = scope.clone();
        self.state.grab_edit_text.grabbed_text.clear();
        self.state.grab_edit_text.copied_feedback = false;
        self.state.grab_edit_text.modal_open = true;
        self.state.status_message = format!("Scanning text in {scope}...");

        let (tx, rx) = std::sync::mpsc::channel::<OcrWorkerEvent>();
        let worker_cancel = Arc::clone(&cancel_flag);
        let _ = std::thread::Builder::new()
            .name("snagsoup-grab-text-ocr".into())
            .spawn(move || {
                let res = snagsoup_cv_ocr::recognize_rgba_with_progress(
                    rw,
                    rh,
                    &rgba,
                    None,
                    |frac, stage| {
                        if worker_cancel.load(Ordering::Relaxed) {
                            return false;
                        }
                        let _ = tx.send(OcrWorkerEvent::Progress(frac, stage.to_string()));
                        true
                    },
                )
                .map(|doc| doc.text);
                let _ = tx.send(OcrWorkerEvent::Finished(res));
            });

        cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(25))
                    .await;

                let mut done = false;
                let mut latest_progress: Option<(f32, String)> = None;
                let mut final_result: Option<Result<String, snagsoup_cv_ocr::CvOcrError>> = None;

                while let Ok(ev) = rx.try_recv() {
                    match ev {
                        OcrWorkerEvent::Progress(p, stg) => {
                            latest_progress = Some((p, stg));
                        }
                        OcrWorkerEvent::Finished(res) => {
                            final_result = Some(res);
                            done = true;
                        }
                    }
                }

                if latest_progress.is_some() || final_result.is_some() {
                    let updated = weak.update(cx, |view, cx| {
                        if view.state.grab_edit_text.active_job_id != job_id {
                            return false;
                        }
                        if let Some((p, stg)) = latest_progress {
                            view.state.grab_edit_text.progress = p.clamp(0.0, 1.0);
                            view.state.grab_edit_text.progress_stage = stg;
                        }
                        if let Some(res) = final_result {
                            view.state.grab_edit_text.is_running = false;
                            view.state.grab_edit_text.cancel_flag = None;
                            match res {
                                Ok(text) => {
                                    let trimmed = text.trim().to_string();
                                    view.state.grab_edit_text.progress = 1.0;
                                    view.state.grab_edit_text.grabbed_text = trimmed.clone();
                                    if !trimmed.is_empty() {
                                        let _ = snagsoup_win_platform::clipboard::write_text_to_clipboard(&trimmed);
                                        view.state.grab_edit_text.copied_feedback = true;
                                        let line_cnt = trimmed.lines().count();
                                        view.state.status_message = format!(
                                            "Grabbed {} chars ({} lines) from {} & copied to Clipboard",
                                            trimmed.chars().count(),
                                            line_cnt,
                                            view.state.grab_edit_text.scope_label
                                        );
                                    } else {
                                        view.state.grab_edit_text.copied_feedback = false;
                                        view.state.status_message =
                                            "OCR completed: no text detected in image".to_string();
                                    }
                                }
                                Err(snagsoup_cv_ocr::CvOcrError::Cancelled) => {
                                    view.state.grab_edit_text.modal_open = false;
                                    view.state.status_message = "OCR cancelled".to_string();
                                }
                                Err(err) => {
                                    view.state.grab_edit_text.modal_open = false;
                                    view.state.status_message =
                                        format!("Grab Text OCR failed: {err}");
                                }
                            }
                        }
                        cx.notify();
                        true
                    });
                    if updated.map_or(true, |keep| !keep) {
                        break;
                    }
                }

                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// Render inline parameter controls for the selected Effect (`Border`, `Drop Shadow`, `Edges`,
    /// `Page Curl`, `Perspective`, `Color Filter`, `Color Adjustment`, `Color Replacement`,
    /// `Spotlight & Magnify`, `Caption`, `Auto Trim`, `Rotate`, `Flip`, `Scale Image`).
    fn render_effect_config_controls(
        &self,
        idx: usize,
        theme: SnagTheme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(entry) = self.state.effects_gallery.entries.get(idx) else {
            return div().into_any_element();
        };
        let cfg = entry.config.clone();

        let controls = match cfg {
            EffectConfig::Border {
                width,
                color,
                inside,
            } => {
                let hex = ((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        NumericStepper::new("eff-border-w", width as f32, 1.0, 48.0)
                            .label("Width")
                            .step(1.0)
                            .unit("px")
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Border {
                                        width: v.round().max(1.0) as u32,
                                        color,
                                        inside,
                                    },
                                    true,
                                );
                                this.state.applied_effects_count =
                                    this.state.effects_gallery.active_count();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(4.0))
                            .child(
                                Button::new("eff-border-out", "Outside")
                                    .variant(if !inside {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.effects_gallery.update_effect_config(
                                            &mut this.state.session,
                                            idx,
                                            EffectConfig::Border {
                                                width,
                                                color,
                                                inside: false,
                                            },
                                            true,
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("eff-border-in", "Inside")
                                    .variant(if inside {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.effects_gallery.update_effect_config(
                                            &mut this.state.session,
                                            idx,
                                            EffectConfig::Border {
                                                width,
                                                color,
                                                inside: true,
                                            },
                                            true,
                                        );
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        ColorSwatchPicker::new("eff-border-color", hex).on_select(cx.listener(
                            move |this, &rgb_hex: &u32, _, cx| {
                                let nc = ColorRgba::rgb(
                                    ((rgb_hex >> 16) & 0xFF) as u8,
                                    ((rgb_hex >> 8) & 0xFF) as u8,
                                    (rgb_hex & 0xFF) as u8,
                                );
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Border {
                                        width,
                                        color: nc,
                                        inside,
                                    },
                                    true,
                                );
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element()
            }

            EffectConfig::Shadow {
                offset_x,
                offset_y,
                blur_radius,
                opacity,
                color,
            } => {
                let hex = ((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        NumericStepper::new("eff-shd-blur", blur_radius as f32, 0.0, 32.0)
                            .label("Blur")
                            .step(1.0)
                            .unit("px")
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Shadow {
                                        offset_x,
                                        offset_y,
                                        blur_radius: v.round().max(0.0) as u32,
                                        opacity,
                                        color,
                                    },
                                    true,
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        NumericStepper::new("eff-shd-ox", offset_x as f32, -32.0, 32.0)
                            .label("Offset X")
                            .step(2.0)
                            .unit("px")
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Shadow {
                                        offset_x: v.round() as i32,
                                        offset_y,
                                        blur_radius,
                                        opacity,
                                        color,
                                    },
                                    true,
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        NumericStepper::new("eff-shd-oy", offset_y as f32, -32.0, 32.0)
                            .label("Offset Y")
                            .step(2.0)
                            .unit("px")
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Shadow {
                                        offset_x,
                                        offset_y: v.round() as i32,
                                        blur_radius,
                                        opacity,
                                        color,
                                    },
                                    true,
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        Slider::new("eff-shd-op", opacity * 100.0, 10.0, 100.0)
                            .label("Shadow Opacity")
                            .width(px(216.0))
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Shadow {
                                        offset_x,
                                        offset_y,
                                        blur_radius,
                                        opacity: (v / 100.0).clamp(0.05, 1.0),
                                        color,
                                    },
                                    false,
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        ColorSwatchPicker::new("eff-shd-color", hex).on_select(cx.listener(
                            move |this, &rgb_hex: &u32, _, cx| {
                                let nc = ColorRgba::rgb(
                                    ((rgb_hex >> 16) & 0xFF) as u8,
                                    ((rgb_hex >> 8) & 0xFF) as u8,
                                    (rgb_hex & 0xFF) as u8,
                                );
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Shadow {
                                        offset_x,
                                        offset_y,
                                        blur_radius,
                                        opacity,
                                        color: nc,
                                    },
                                    true,
                                );
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element()
            }

            EffectConfig::Edges {
                style,
                size,
                edges_mask,
                seed,
            } => {
                let st_a = style.clone();
                let masks: &[(u8, &str)] = &[
                    (0x0F, "All"),
                    (0x04, "Bottom"),
                    (0x06, "Right+Bot"),
                    (0x05, "Top+Bot"),
                ];
                let mut mask_row = div().flex().flex_row().flex_wrap().gap(px(4.0));
                for &(m_val, m_lbl) in masks {
                    let st_c = style.clone();
                    mask_row = mask_row.child(
                        Button::new(format!("eff-edge-m-{idx}-{m_val}"), m_lbl)
                            .variant(if edges_mask == m_val {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Edges {
                                        style: st_c.clone(),
                                        size,
                                        edges_mask: m_val,
                                        seed,
                                    },
                                    true,
                                );
                                cx.notify();
                            })),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        NumericStepper::new(format!("eff-edge-sz-{idx}"), size as f32, 2.0, 32.0)
                            .label("Size")
                            .step(2.0)
                            .unit("px")
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Edges {
                                        style: st_a.clone(),
                                        size: v.round().max(2.0) as u32,
                                        edges_mask,
                                        seed,
                                    },
                                    true,
                                );
                                cx.notify();
                            })),
                    )
                    .child(mask_row)
                    .into_any_element()
            }

            EffectConfig::PageCurl {
                corner,
                radius,
                background_color,
            } => {
                let corners = ["BottomRight", "BottomLeft", "TopRight", "TopLeft"];
                let mut c_grid = div().flex().flex_row().flex_wrap().gap(px(4.0));
                for c_name in corners {
                    let active_c = corner == c_name;
                    c_grid = c_grid.child(
                        Button::new(format!("eff-curl-{c_name}"), c_name)
                            .variant(if active_c {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::PageCurl {
                                        corner: c_name.to_string(),
                                        radius,
                                        background_color,
                                    },
                                    true,
                                );
                                cx.notify();
                            })),
                    );
                }
                let corner_s = corner.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(c_grid)
                    .child(
                        Slider::new("eff-curl-rad", radius, 12.0, 96.0)
                            .label("Curl Size")
                            .width(px(216.0))
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::PageCurl {
                                        corner: corner_s.clone(),
                                        radius: v,
                                        background_color,
                                    },
                                    false,
                                );
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }

            EffectConfig::Perspective {
                angle_x,
                angle_y,
                reflection,
                reflection_opacity,
            } => div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    Slider::new("eff-persp-ax", angle_x, -45.0, 45.0)
                        .label("Horizontal Tilt")
                        .width(px(216.0))
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::Perspective {
                                    angle_x: v,
                                    angle_y,
                                    reflection,
                                    reflection_opacity,
                                },
                                false,
                            );
                            cx.notify();
                        })),
                )
                .child(
                    Slider::new("eff-persp-ay", angle_y, -30.0, 30.0)
                        .label("Vertical Tilt")
                        .width(px(216.0))
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::Perspective {
                                    angle_x,
                                    angle_y: v,
                                    reflection,
                                    reflection_opacity,
                                },
                                false,
                            );
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(
                        "eff-persp-refl",
                        if reflection {
                            "Reflection: ON"
                        } else {
                            "Reflection: OFF"
                        },
                    )
                    .variant(if reflection {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Secondary
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.effects_gallery.update_effect_config(
                            &mut this.state.session,
                            idx,
                            EffectConfig::Perspective {
                                angle_x,
                                angle_y,
                                reflection: !reflection,
                                reflection_opacity,
                            },
                            true,
                        );
                        cx.notify();
                    })),
                )
                .into_any_element(),

            EffectConfig::Filters { kind, intensity } => {
                let kinds = ["Grayscale", "Sepia", "Invert"];
                let mut k_row = div().flex().flex_row().flex_wrap().gap(px(4.0));
                for k_name in kinds {
                    let is_k = kind.eq_ignore_ascii_case(k_name);
                    k_row = k_row.child(
                        Button::new(format!("eff-flt-{k_name}"), k_name)
                            .variant(if is_k {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Filters {
                                        kind: k_name.to_string(),
                                        intensity,
                                    },
                                    true,
                                );
                                cx.notify();
                            })),
                    );
                }
                let kind_s = kind.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(k_row)
                    .child(
                        Slider::new("eff-flt-int", intensity, 10.0, 100.0)
                            .label("Intensity")
                            .width(px(216.0))
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Filters {
                                        kind: kind_s.clone(),
                                        intensity: v,
                                    },
                                    false,
                                );
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }

            EffectConfig::ColorAdjustment {
                brightness,
                contrast,
                hue,
                saturation,
                gamma,
            } => div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    Slider::new("eff-ca-br", brightness, -100.0, 100.0)
                        .label("Brightness")
                        .width(px(216.0))
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::ColorAdjustment {
                                    brightness: v,
                                    contrast,
                                    hue,
                                    saturation,
                                    gamma,
                                },
                                false,
                            );
                            cx.notify();
                        })),
                )
                .child(
                    Slider::new("eff-ca-ct", contrast, -100.0, 100.0)
                        .label("Contrast")
                        .width(px(216.0))
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::ColorAdjustment {
                                    brightness,
                                    contrast: v,
                                    hue,
                                    saturation,
                                    gamma,
                                },
                                false,
                            );
                            cx.notify();
                        })),
                )
                .child(
                    Slider::new("eff-ca-sat", saturation, -100.0, 100.0)
                        .label("Saturation")
                        .width(px(216.0))
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::ColorAdjustment {
                                    brightness,
                                    contrast,
                                    hue,
                                    saturation: v,
                                    gamma,
                                },
                                false,
                            );
                            cx.notify();
                        })),
                )
                .into_any_element(),

            EffectConfig::ColorReplacement {
                source,
                target,
                tolerance,
                preserve_luminance,
            } => {
                let tgt_hex =
                    ((target.r as u32) << 16) | ((target.g as u32) << 8) | (target.b as u32);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(4.0))
                            .child(
                                Button::new("eff-cr-white", "From White")
                                    .variant(if source == ColorRgba::WHITE {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.effects_gallery.update_effect_config(
                                            &mut this.state.session,
                                            idx,
                                            EffectConfig::ColorReplacement {
                                                source: ColorRgba::WHITE,
                                                target,
                                                tolerance,
                                                preserve_luminance,
                                            },
                                            true,
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("eff-cr-black", "From Black")
                                    .variant(if source == ColorRgba::BLACK {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.effects_gallery.update_effect_config(
                                            &mut this.state.session,
                                            idx,
                                            EffectConfig::ColorReplacement {
                                                source: ColorRgba::BLACK,
                                                target,
                                                tolerance,
                                                preserve_luminance,
                                            },
                                            true,
                                        );
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        Slider::new("eff-cr-tol", tolerance, 2.0, 80.0)
                            .label("Tolerance")
                            .width(px(216.0))
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::ColorReplacement {
                                        source,
                                        target,
                                        tolerance: v,
                                        preserve_luminance,
                                    },
                                    false,
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        ColorSwatchPicker::new("eff-cr-tgt", tgt_hex).on_select(cx.listener(
                            move |this, &rgb_hex: &u32, _, cx| {
                                let nt = ColorRgba::rgb(
                                    ((rgb_hex >> 16) & 0xFF) as u8,
                                    ((rgb_hex >> 8) & 0xFF) as u8,
                                    (rgb_hex & 0xFF) as u8,
                                );
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::ColorReplacement {
                                        source,
                                        target: nt,
                                        tolerance,
                                        preserve_luminance,
                                    },
                                    true,
                                );
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element()
            }

            EffectConfig::SpotlightAndMagnify {
                x,
                y,
                width,
                height,
                dim_opacity,
                blur_radius,
                magnify_scale,
            } => div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .p(px(6.0))
                        .rounded(theme.radius_sm)
                        .bg(theme.panel_bg)
                        .border_1()
                        .border_color(theme.accent_primary)
                        .text_xs()
                        .text_color(theme.text_primary)
                        .child(format!(
                            "Drag box or 8 handles on image ({:.0}, {:.0} · {:.0}×{:.0} px)",
                            x, y, width, height
                        )),
                )
                .child(
                    Button::new("eff-spot-center", "Reset Center Box")
                        .variant(ButtonVariant::Secondary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let (bw, bh) =
                                this.state.effects_gallery.base_dimensions(&this.state.session);
                            let sw = ((bw as f64) * 0.50).max(24.0).round();
                            let sh = ((bh as f64) * 0.50).max(24.0).round();
                            let sx = (((bw as f64) - sw) * 0.5).max(0.0).round();
                            let sy = (((bh as f64) - sh) * 0.5).max(0.0).round();
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::SpotlightAndMagnify {
                                    x: sx,
                                    y: sy,
                                    width: sw,
                                    height: sh,
                                    dim_opacity,
                                    blur_radius,
                                    magnify_scale,
                                },
                                true,
                            );
                            cx.notify();
                        })),
                )
                .child(
                    Slider::new("eff-spot-dim", dim_opacity * 100.0, 0.0, 90.0)
                        .label("Dim Outside (%)")
                        .width(px(216.0))
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::SpotlightAndMagnify {
                                    x,
                                    y,
                                    width,
                                    height,
                                    dim_opacity: (v / 100.0).clamp(0.0, 0.95),
                                    blur_radius,
                                    magnify_scale,
                                },
                                false,
                            );
                            cx.notify();
                        })),
                )
                .child(
                    NumericStepper::new("eff-spot-blur", blur_radius as f32, 0.0, 16.0)
                        .label("Blur Outside")
                        .step(1.0)
                        .unit("px")
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::SpotlightAndMagnify {
                                    x,
                                    y,
                                    width,
                                    height,
                                    dim_opacity,
                                    blur_radius: v.round().max(0.0) as u32,
                                    magnify_scale,
                                },
                                true,
                            );
                            cx.notify();
                        })),
                )
                .child(
                    Slider::new("eff-spot-mag", magnify_scale * 100.0, 100.0, 250.0)
                        .label("Magnify (%)")
                        .width(px(216.0))
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            this.state.effects_gallery.update_effect_config(
                                &mut this.state.session,
                                idx,
                                EffectConfig::SpotlightAndMagnify {
                                    x,
                                    y,
                                    width,
                                    height,
                                    dim_opacity,
                                    blur_radius,
                                    magnify_scale: (v / 100.0).clamp(1.0, 3.0),
                                },
                                false,
                            );
                            cx.notify();
                        })),
                )
                .into_any_element(),

            EffectConfig::Caption {
                text,
                position,
                font_family,
                font_size,
                text_color,
                background_color,
            } => {
                let is_editing = self.state.effects_gallery.editing_caption_text;
                let txt_display = if is_editing {
                    format!("{text}|")
                } else if text.is_empty() {
                    "(Click to type caption...)".to_string()
                } else {
                    text.clone()
                };
                let t1 = text.clone();
                let f1 = font_family.clone();
                let t2 = text.clone();
                let f2 = font_family.clone();
                let t3 = text.clone();
                let p3 = position.clone();
                let f3 = font_family.clone();

                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("eff-caption-input")
                            .p(px(6.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.input_bg)
                            .border_1()
                            .border_color(if is_editing {
                                theme.accent_primary
                            } else {
                                theme.border_strong
                            })
                            .text_xs()
                            .text_color(theme.text_primary)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.effects_gallery.editing_caption_text =
                                    !this.state.effects_gallery.editing_caption_text;
                                cx.notify();
                            }))
                            .child(txt_display),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(4.0))
                            .child(
                                Button::new("eff-cap-bot", "Bottom")
                                    .variant(if position == "Bottom" {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.effects_gallery.update_effect_config(
                                            &mut this.state.session,
                                            idx,
                                            EffectConfig::Caption {
                                                text: t1.clone(),
                                                position: "Bottom".into(),
                                                font_family: f1.clone(),
                                                font_size,
                                                text_color,
                                                background_color,
                                            },
                                            true,
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("eff-cap-top", "Top")
                                    .variant(if position == "Top" {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.effects_gallery.update_effect_config(
                                            &mut this.state.session,
                                            idx,
                                            EffectConfig::Caption {
                                                text: t2.clone(),
                                                position: "Top".into(),
                                                font_family: f2.clone(),
                                                font_size,
                                                text_color,
                                                background_color,
                                            },
                                            true,
                                        );
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        NumericStepper::new("eff-cap-fs", font_size, 12.0, 42.0)
                            .label("Font Size")
                            .step(2.0)
                            .unit("px")
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                this.state.effects_gallery.update_effect_config(
                                    &mut this.state.session,
                                    idx,
                                    EffectConfig::Caption {
                                        text: t3.clone(),
                                        position: p3.clone(),
                                        font_family: f3.clone(),
                                        font_size: v,
                                        text_color,
                                        background_color,
                                    },
                                    true,
                                );
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }

            EffectConfig::Trim {
                tolerance,
                auto_trim,
            } => div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    NumericStepper::new("eff-trim-tol", tolerance as f32, 1.0, 40.0)
                        .label("Tolerance")
                        .step(1.0)
                        .unit("")
                        .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                            if let Some(e) = this.state.effects_gallery.entries.get_mut(idx) {
                                e.config = EffectConfig::Trim {
                                    tolerance: v.round().clamp(1.0, 80.0) as u8,
                                    auto_trim,
                                };
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("eff-trim-run", "Apply Auto Trim")
                        .variant(ButtonVariant::Primary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let c = EffectConfig::Trim {
                                tolerance,
                                auto_trim: true,
                            };
                            if this
                                .state
                                .effects_gallery
                                .apply_one_shot_transform(&mut this.state.session, &c)
                            {
                                this.state.pending_fit_canvas = true;
                                this.state.status_message = "Applied Auto Trim".to_string();
                            }
                            cx.notify();
                        })),
                )
                .into_any_element(),

            EffectConfig::Rotate {
                background_color, ..
            } => div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(px(4.0))
                .child(
                    Button::new("eff-rot-cw", "90° CW ↻")
                        .variant(ButtonVariant::Primary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let c = EffectConfig::Rotate {
                                angle_degrees: 90.0,
                                background_color,
                            };
                            if this
                                .state
                                .effects_gallery
                                .apply_one_shot_transform(&mut this.state.session, &c)
                            {
                                this.state.pending_fit_canvas = true;
                                this.state.status_message = "Rotated 90° CW".to_string();
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("eff-rot-ccw", "90° CCW ↺")
                        .variant(ButtonVariant::Secondary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let c = EffectConfig::Rotate {
                                angle_degrees: -90.0,
                                background_color,
                            };
                            if this
                                .state
                                .effects_gallery
                                .apply_one_shot_transform(&mut this.state.session, &c)
                            {
                                this.state.pending_fit_canvas = true;
                                this.state.status_message = "Rotated 90° CCW".to_string();
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("eff-rot-180", "180°")
                        .variant(ButtonVariant::Secondary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let c = EffectConfig::Rotate {
                                angle_degrees: 180.0,
                                background_color,
                            };
                            if this
                                .state
                                .effects_gallery
                                .apply_one_shot_transform(&mut this.state.session, &c)
                            {
                                this.state.status_message = "Rotated 180°".to_string();
                            }
                            cx.notify();
                        })),
                )
                .into_any_element(),

            EffectConfig::Flip { .. } => div()
                .flex()
                .flex_row()
                .gap(px(4.0))
                .child(
                    Button::new("eff-flip-h", "Horizontal ⇄")
                        .variant(ButtonVariant::Primary)
                        .on_click(cx.listener(|this, _, _, cx| {
                            let c = EffectConfig::Flip {
                                horizontal: true,
                                vertical: false,
                            };
                            if this
                                .state
                                .effects_gallery
                                .apply_one_shot_transform(&mut this.state.session, &c)
                            {
                                this.state.status_message = "Flipped Horizontally".to_string();
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("eff-flip-v", "Vertical ⇅")
                        .variant(ButtonVariant::Secondary)
                        .on_click(cx.listener(|this, _, _, cx| {
                            let c = EffectConfig::Flip {
                                horizontal: false,
                                vertical: true,
                            };
                            if this
                                .state
                                .effects_gallery
                                .apply_one_shot_transform(&mut this.state.session, &c)
                            {
                                this.state.status_message = "Flipped Vertically".to_string();
                            }
                            cx.notify();
                        })),
                )
                .into_any_element(),

            EffectConfig::Scale {
                width,
                height,
                keep_aspect_ratio,
                dpi,
            } => {
                let cur_cw = self.state.session.bitmap.width().max(1) as f32;
                let cur_ch = self.state.session.bitmap.height().max(1) as f32;
                let aspect = cur_cw / cur_ch;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        NumericStepper::new("eff-scl-w", width as f32, 16.0, 7680.0)
                            .label("Width")
                            .step(20.0)
                            .unit("px")
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                let nw = v.round().max(16.0) as u32;
                                let nh = if keep_aspect_ratio {
                                    ((nw as f32) / aspect).round().max(16.0) as u32
                                } else {
                                    height
                                };
                                if let Some(e) = this.state.effects_gallery.entries.get_mut(idx) {
                                    e.config = EffectConfig::Scale {
                                        width: nw,
                                        height: nh,
                                        keep_aspect_ratio,
                                        dpi,
                                    };
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        NumericStepper::new("eff-scl-h", height as f32, 16.0, 4320.0)
                            .label("Height")
                            .step(20.0)
                            .unit("px")
                            .on_change(cx.listener(move |this, &v: &f32, _, cx| {
                                let nh = v.round().max(16.0) as u32;
                                let nw = if keep_aspect_ratio {
                                    ((nh as f32) * aspect).round().max(16.0) as u32
                                } else {
                                    width
                                };
                                if let Some(e) = this.state.effects_gallery.entries.get_mut(idx) {
                                    e.config = EffectConfig::Scale {
                                        width: nw,
                                        height: nh,
                                        keep_aspect_ratio,
                                        dpi,
                                    };
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("eff-scl-apply", format!("Apply Scale ({width}×{height})"))
                            .variant(ButtonVariant::Primary)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let c = EffectConfig::Scale {
                                    width,
                                    height,
                                    keep_aspect_ratio,
                                    dpi,
                                };
                                if this
                                    .state
                                    .effects_gallery
                                    .apply_one_shot_transform(&mut this.state.session, &c)
                                {
                                    this.state.pending_fit_canvas = true;
                                    this.state.status_message =
                                        format!("Scaled image to {width}×{height} px");
                                }
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }

            _ => div().into_any_element(),
        };

        div()
            .p(px(8.0))
            .rounded(theme.radius_sm)
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(controls)
            .into_any_element()
    }
}

/// Compute `(visible_pinned_tool_count, compact_labels)` for the Editor top toolbar
/// based on the current viewport width so tools collapse gracefully into `More ▾`
/// and buttons never clip off-screen on narrow/snapped windows.
pub fn compute_responsive_toolbar(
    viewport_w: f32,
    desired_pinned: usize,
    total_tools: usize,
) -> (usize, bool) {
    let max_pinned = if viewport_w >= 1260.0 {
        10
    } else if viewport_w >= 1060.0 {
        6
    } else if viewport_w >= 860.0 {
        3
    } else if viewport_w >= 680.0 {
        2
    } else {
        0
    };
    let compact = viewport_w < 1020.0;
    (desired_pinned.min(max_pinned).min(total_tools), compact)
}

/// Compute `(left, top)` in window logical coordinates for a dropdown popover anchored
/// beneath a toolbar button `(btn_left, btn_bottom, btn_width)` so menus always drop
/// cleanly directly under their trigger button without overflowing the window edges.
pub fn compute_anchored_popover_pos(
    btn_bounds: (f32, f32, f32),
    popover_w: f32,
    viewport_w: f32,
    align_right_edge: bool,
) -> (f32, f32) {
    let (btn_left, btn_bottom, btn_w) = btn_bounds;
    let raw_left = if align_right_edge {
        btn_left + btn_w - popover_w
    } else {
        btn_left
    };
    let max_left = (viewport_w - popover_w - 8.0).max(8.0);
    let left = raw_left.clamp(8.0, max_left);
    let top = (btn_bottom + 4.0).max(38.0);
    (left, top)
}

fn wrap_with_bounds_probe(
    cell: Rc<Cell<(f32, f32, f32)>>,
    child: impl IntoElement,
) -> impl IntoElement {
    div()
        .relative()
        .child(
            canvas(
                move |bounds, _window, _cx| {
                    let x: f32 = bounds.origin.x.into();
                    let y: f32 = bounds.origin.y.into();
                    let w: f32 = bounds.size.width.into();
                    let h: f32 = bounds.size.height.into();
                    cell.set((x, y + h, w));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .child(child)
}

impl Render for EditorShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        let viewport_w: f32 = window.viewport_size().width.into();
        let viewport_h: f32 = window.viewport_size().height.into();
        let dev_scale = window.scale_factor().max(0.5);
        self.state.viewport.device_scale = dev_scale;

        // Sync live canvas well dimensions before fitting & rasterizing so images never default to 960×640 -> 75%
        let (measured_cw, measured_ch) = self.canvas_size_cell.get();
        let side_w: f32 = if self.state.show_side_panel && viewport_w >= 680.0 {
            theme.panel_width.into()
        } else {
            0.0
        };
        let tray_h: f32 = if self.state.show_recent_tray { 84.0 } else { 0.0 };
        let est_cw = (viewport_w - side_w).max(160.0);
        let est_ch = (viewport_h - 46.0 - tray_h - 32.0).max(160.0);
        self.state.viewport.viewport_w = if measured_cw > 64.0 { measured_cw } else { est_cw };
        self.state.viewport.viewport_h = if measured_ch > 64.0 { measured_ch } else { est_ch };

        let can_undo = self.state.session.history.can_undo();
        let can_redo = self.state.session.history.can_redo();
        let (pinned_n, compact_bar) = compute_responsive_toolbar(
            viewport_w,
            self.state.pinned_tool_count,
            self.state.tools.len(),
        );

        // Primary pinned tools strip
        let mut tool_strip = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(2.0));
        for entry in &self.state.tools[..pinned_n] {
            let mode = entry.mode.clone();
            let active = self.state.active_tool == mode;
            tool_strip = tool_strip.child(
                IconButton::new(format!("ed-tool-{}", entry.label), entry.icon)
                    .active(active)
                    .tooltip(entry.label, entry.shortcut.map(SharedString::from))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.select_tool(mode.clone());
                        cx.notify();
                    })),
            );
        }

        // More overflow tools popover content (2 equal-width left-aligned columns)
        let overflow_tools = &self.state.tools[pinned_n..];
        let mid_idx = overflow_tools.len().div_ceil(2);
        let mut more_col_left = div().flex().flex_col().flex_1().gap(px(2.0));
        let mut more_col_right = div().flex().flex_col().flex_1().gap(px(2.0));
        for (idx, entry) in overflow_tools.iter().enumerate() {
            let mode = entry.mode.clone();
            let active = self.state.active_tool == mode;
            let btn = Button::new(format!("ed-more-{}", entry.label), entry.label)
                .icon(entry.icon)
                .variant(if active {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Ghost
                })
                .align_left()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.select_tool(mode.clone());
                    cx.notify();
                }));
            if idx < mid_idx {
                more_col_left = more_col_left.child(btn);
            } else {
                more_col_right = more_col_right.child(btn);
            }
        }
        let more_grid = div()
            .flex()
            .flex_row()
            .w_full()
            .gap(px(4.0))
            .child(more_col_left)
            .when(overflow_tools.len() > mid_idx, |el| el.child(more_col_right));

        let theme_icon = match theme.mode {
            ThemeMode::Light => SnagIcon::Moon,
            ThemeMode::Dark => SnagIcon::Sun,
        };

        let canvas_w = self.state.session.bitmap.width();
        let canvas_h = self.state.session.bitmap.height();
        let obj_count = self.state.session.page.capture_objects.len();
        if self.state.workspace_view == WorkspaceView::Editor {
            if self.state.pending_fit_canvas {
                self.state.viewport.fit_canvas(canvas_w, canvas_h);
                self.state.zoom_percent = self.state.viewport.zoom_percent();
                self.state.pending_fit_canvas = false;
            }
            let vp = self.state.viewport.clone();
            self.state.gpu_cache.sync_from_session(&mut self.state.session, &vp);
        }

        let create_probe = Rc::clone(&self.create_btn_bounds);
        let more_probe = Rc::clone(&self.more_btn_bounds);
        let share_probe = Rc::clone(&self.share_btn_bounds);
        let copy_copied = self.state.copy_feedback_active;

        // Top Bar (responsive: collapses pinned tools into More & wraps cleanly on narrow windows)
        let top_bar = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .min_h(theme.toolbar_height)
            .px(px(10.0))
            .py(px(4.0))
            .gap(px(6.0))
            .bg(theme.toolbar_bg)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        Button::new(
                            "btn-lib-toggle",
                            if self.state.workspace_view == WorkspaceView::Library {
                                "Editor"
                            } else {
                                "Library"
                            },
                        )
                        .icon(if self.state.workspace_view == WorkspaceView::Library {
                            SnagIcon::Editor
                        } else {
                            SnagIcon::Library
                        })
                        .variant(ButtonVariant::Secondary)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.workspace_view = match this.state.workspace_view {
                                WorkspaceView::Editor => WorkspaceView::Library,
                                _ => WorkspaceView::Editor,
                            };
                            if this.state.workspace_view == WorkspaceView::Library {
                                let lib = this.ensure_library_view(cx);
                                lib.update(cx, |v, cx| {
                                    v.sync_directory(&capture_storage_dir());
                                    v.refresh();
                                    cx.notify();
                                });
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        Button::new("ed-capture", "Capture")
                            .icon(SnagIcon::Capture)
                            .variant(ButtonVariant::Capture)
                            .tooltip(
                                "Open SnagSoup Capture Window",
                                Some("Configure All-in-One / Image / Video & click Capture".into()),
                            )
                            .on_click(cx.listener(|_this, _, _window, cx| {
                                crate::open_or_focus_capture_window(cx);
                            })),
                    )
                    .child(
                        Button::new("ed-save", "Save")
                            .variant(ButtonVariant::Secondary)
                            .tooltip("Save .snagx", Some("Save to Library & Captures folder".into()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                match this.state.quick_save() {
                                    Ok(_) => {
                                        if let Some(ref lib) = this.library_view {
                                            lib.update(cx, |v, cx| {
                                                v.sync_directory(&capture_storage_dir());
                                                v.refresh();
                                                cx.notify();
                                            });
                                        }
                                    }
                                    Err(err) => {
                                        this.state.status_message = err;
                                    }
                                }
                                cx.notify();
                            })),
                    )
                    .child(wrap_with_bounds_probe(
                        create_probe,
                        Button::new("ed-create", "+ Create")
                            .variant(if self.state.show_create_menu {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = !this.state.show_create_menu;
                                this.state.close_all_popovers();
                                this.state.show_create_menu = next;
                                cx.notify();
                            })),
                    ))
                    .child(
                        IconButton::new("ed-undo", SnagIcon::Undo)
                            .disabled(!can_undo)
                            .tooltip("Undo", Some("Ctrl+Z".into()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.state.perform_undo() {
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        IconButton::new("ed-redo", SnagIcon::Redo)
                            .disabled(!can_redo)
                            .tooltip("Redo", Some("Ctrl+Y / Ctrl+Shift+Z".into()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.state.perform_redo() {
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.0))
                    .child(tool_strip)
                    .child(wrap_with_bounds_probe(
                        more_probe,
                        Button::new("ed-more-btn", "More")
                            .icon(SnagIcon::ChevronDown)
                            .variant(if self.state.show_more_tools {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Ghost
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = !this.state.show_more_tools;
                                this.state.close_all_popovers();
                                this.state.show_more_tools = next;
                                cx.notify();
                            })),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        IconButton::new("ed-theme", theme_icon)
                            .tooltip("Toggle Light/Dark Theme", None)
                            .on_click(cx.listener(|_, _, _, cx| {
                                let next = cx
                                    .try_global::<SnagTheme>()
                                    .map(|t| t.mode.toggled())
                                    .unwrap_or(ThemeMode::Dark);
                                cx.set_global(SnagTheme::for_mode(next));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(
                            "ed-grab-text",
                            if self.state.grab_edit_text.is_running {
                                "Scanning..."
                            } else if compact_bar {
                                "OCR"
                            } else {
                                "Grab Text"
                            },
                        )
                            .disabled(self.state.grab_edit_text.is_running)
                            .variant(ButtonVariant::Secondary)
                            .tooltip("OCR Grab Text", Some("Extract text to Clipboard".into()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_grab_text_async(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(
                            "ed-copy-all",
                            if copy_copied {
                                "Copied"
                            } else if compact_bar {
                                "Copy"
                            } else {
                                "Copy All"
                            },
                        )
                        .icon(if copy_copied {
                            SnagIcon::Check
                        } else {
                            SnagIcon::Copy
                        })
                        .variant(if copy_copied {
                            ButtonVariant::Success
                        } else {
                            ButtonVariant::Primary
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            match this.state.trigger_copy_all_with_feedback() {
                                Ok(seq) => {
                                    cx.spawn(async move |weak, cx| {
                                        cx.background_executor()
                                            .timer(std::time::Duration::from_millis(2000))
                                            .await;
                                        let _ = weak.update(cx, |view, cx| {
                                            if view.state.clear_copy_feedback_if_seq(seq) {
                                                cx.notify();
                                            }
                                        });
                                    })
                                    .detach();
                                }
                                Err(err) => {
                                    this.state.status_message = err;
                                }
                            }
                            cx.notify();
                        })),
                    )
                    .child(wrap_with_bounds_probe(
                        share_probe,
                        Button::new("ed-share", "Share")
                            .icon(SnagIcon::Share)
                            .variant(if self.state.show_share_menu {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = !this.state.show_share_menu;
                                this.state.close_all_popovers();
                                this.state.show_share_menu = next;
                                cx.notify();
                            })),
                    )),
            );

        // Interactive 15 Image Effects Studio in Effects tab (ON/OFF toggles + inline parameters)
        let active_eff_count = self.state.effects_gallery.active_count();
        let selected_eff_idx = self.state.effects_gallery.selected_idx;
        let mut effects_list = div()
            .id("effects-studio-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(5.0))
            .pr(px(2.0))
            .overflow_y_scroll();

        for idx in 0..self.state.effects_gallery.entries.len() {
            let (lbl, is_toggleable, is_on) = {
                let e = &self.state.effects_gallery.entries[idx];
                (e.name, e.toggleable, e.enabled)
            };
            let is_selected = selected_eff_idx == Some(idx);

            let right_badge = if is_toggleable {
                div()
                    .id(SharedString::from(format!("eff-pill-{idx}")))
                    .flex_shrink_0()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(99.0))
                    .bg(if is_on {
                        theme.accent_primary
                    } else {
                        theme.panel_bg
                    })
                    .border_1()
                    .border_color(if is_on {
                        theme.accent_primary
                    } else {
                        theme.border_strong
                    })
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(if is_on {
                        gpui::Hsla::from(rgb(0xFFFFFF))
                    } else {
                        theme.text_muted
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let sel_hint = this.state.active_selection_rect;
                        if let Some(now_on) = this.state.effects_gallery.toggle_effect(
                            &mut this.state.session,
                            idx,
                            sel_hint,
                        ) {
                            this.state.applied_effects_count =
                                this.state.effects_gallery.active_count();
                            this.state.status_message = format!(
                                "{}: {}",
                                lbl,
                                if now_on { "ON" } else { "OFF" }
                            );
                        }
                        cx.notify();
                    }))
                    .child(if is_on { "ON" } else { "OFF" })
                    .into_any_element()
            } else {
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("▸")
                    .into_any_element()
            };

            let row_header = div()
                .id(SharedString::from(format!("eff-btn-{idx}")))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px(px(9.0))
                .py(px(6.0))
                .rounded(theme.radius_sm)
                .bg(if is_on {
                    theme.surface_active
                } else {
                    theme.surface_elevated
                })
                .border_1()
                .border_color(if is_on || is_selected {
                    theme.accent_primary
                } else {
                    theme.border
                })
                .cursor_pointer()
                .hover(|s| s.border_color(theme.accent_primary))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let sel_hint = this.state.active_selection_rect;
                    let cur_entry_on = this
                        .state
                        .effects_gallery
                        .entries
                        .get(idx)
                        .is_some_and(|e| e.enabled);
                    let cur_entry_toggle = this
                        .state
                        .effects_gallery
                        .entries
                        .get(idx)
                        .is_some_and(|e| e.toggleable);
                    let already_selected = this.state.effects_gallery.selected_idx == Some(idx);

                    if !cur_entry_toggle {
                        this.state.effects_gallery.selected_idx = if already_selected {
                            None
                        } else {
                            let _ = this.state.effects_gallery.toggle_effect(
                                &mut this.state.session,
                                idx,
                                sel_hint,
                            );
                            Some(idx)
                        };
                    } else if !cur_entry_on {
                        // 1st click: turn ON & expand parameter controls!
                        if this
                            .state
                            .effects_gallery
                            .toggle_effect(&mut this.state.session, idx, sel_hint)
                            == Some(true)
                        {
                            this.state.applied_effects_count =
                                this.state.effects_gallery.active_count();
                            this.state.status_message = format!("{lbl}: ON");
                        }
                    } else if already_selected {
                        // 2nd click on active row: turn OFF cleanly!
                        if this
                            .state
                            .effects_gallery
                            .toggle_effect(&mut this.state.session, idx, sel_hint)
                            == Some(false)
                        {
                            this.state.applied_effects_count =
                                this.state.effects_gallery.active_count();
                            this.state.status_message = format!("{lbl}: OFF");
                        }
                    } else {
                        // Active effect was collapsed while inspecting another row -> select & show its parameters
                        this.state.effects_gallery.selected_idx = Some(idx);
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .min_w_0()
                        .child(SnagIcon::Effects.render(
                            px(13.0),
                            if is_on {
                                theme.accent_primary
                            } else {
                                theme.text_secondary
                            },
                        ))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(if is_on {
                                    gpui::FontWeight::SEMIBOLD
                                } else {
                                    gpui::FontWeight::MEDIUM
                                })
                                .text_color(theme.text_primary)
                                .truncate()
                                .child(lbl),
                        ),
                )
                .child(right_badge);

            effects_list = effects_list.child(row_header);
            if is_selected {
                effects_list =
                    effects_list.child(self.render_effect_config_controls(idx, theme, cx));
            }
        }

        let effects_col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(8.0))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(format!("Active Effects: {active_eff_count}")),
                    )
                    .when(active_eff_count > 0, |hdr| {
                        hdr.child(
                            Button::new("eff-clear-all", "Reset All")
                                .variant(ButtonVariant::Ghost)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this
                                        .state
                                        .effects_gallery
                                        .clear_all_active_effects(&mut this.state.session)
                                    {
                                        this.state.applied_effects_count = 0;
                                        this.state.status_message =
                                            "Cleared all active effects".to_string();
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(effects_list);

        // Right collapsible Side Panel (Tool Properties / Effects)
        let side_panel = div()
            .flex()
            .flex_col()
            .h_full()
            .overflow_hidden()
            .w(theme.panel_width)
            .bg(theme.panel_bg)
            .border_l_1()
            .border_color(theme.border)
            .p(px(10.0))
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(4.0))
                    .child(
                        Button::new("tab-tool-props", "Tool Properties")
                            .variant(if self.state.side_panel_tab == SidePanelTab::ToolProperties {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Ghost
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.side_panel_tab = SidePanelTab::ToolProperties;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("tab-effects", "Effects")
                            .icon(SnagIcon::Effects)
                            .variant(if self.state.side_panel_tab == SidePanelTab::Effects {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Ghost
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.side_panel_tab = SidePanelTab::Effects;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.state.side_panel_tab == SidePanelTab::ToolProperties, |el| {
                let current_rgba = ColorRgba::rgb(
                    ((self.state.selected_color >> 16) & 0xFF) as u8,
                    ((self.state.selected_color >> 8) & 0xFF) as u8,
                    (self.state.selected_color & 0xFF) as u8,
                );
                let show_ghost = self.state.quickstyles.should_show_ghost_style(
                    &self.state.active_tool,
                    current_rgba,
                    self.state.stroke_width as f64,
                );
                let tool_styles: Vec<snagsoup_model::QuickStyle> = self
                    .state
                    .quickstyles
                    .styles_for_tool(&self.state.active_tool)
                    .into_iter()
                    .cloned()
                    .collect();
                let styles_count = tool_styles.len();
                let fav_count = self.state.quickstyles.favorites.quick_styles.len();
                let active_tool = self.state.active_tool.clone();

                // QuickStyles clickable visual grid
                let mut qs_grid = div().flex().flex_row().flex_wrap().gap(px(6.0));
                for (q_idx, qs) in tool_styles.iter().enumerate() {
                    let qs_clone = qs.clone();
                    let qs_fav = qs.clone();
                    let is_fav = self
                        .state
                        .quickstyles
                        .favorites
                        .quick_styles
                        .contains(&qs_clone);
                    let fg = qs
                        .foreground_color
                        .unwrap_or(ColorRgba::rgb(217, 43, 43));
                    let hex =
                        ((fg.r as u32) << 16) | ((fg.g as u32) << 8) | (fg.b as u32);
                    let sw_lbl = format!("{:.0}px", qs.stroke_width.unwrap_or(4.0));
                    qs_grid = qs_grid.child(
                        div()
                            .id(SharedString::from(format!("qs-chip-{q_idx}")))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .px(px(7.0))
                            .py(px(4.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.surface_elevated)
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(c) = qs_clone.foreground_color {
                                    this.state.selected_color = ((c.r as u32) << 16)
                                        | ((c.g as u32) << 8)
                                        | (c.b as u32);
                                }
                                if let Some(w) = qs_clone.stroke_width {
                                    this.state.stroke_width = w as f32;
                                }
                                if let Some(op) = qs_clone.opacity {
                                    this.state.opacity = op as f32;
                                }
                                this.state.status_message = "Applied QuickStyle".to_string();
                                cx.notify();
                            }))
                            .child(div().size(px(12.0)).rounded(px(6.0)).bg(rgb(hex)))
                            .child(div().text_xs().text_color(theme.text_primary).child(sw_lbl))
                            .child(
                                div()
                                    .id(SharedString::from(format!("qs-fav-{q_idx}")))
                                    .text_xs()
                                    .text_color(if is_fav {
                                        gpui::Hsla::from(rgb(0xF59E0B))
                                    } else {
                                        theme.text_muted
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let starred =
                                            this.state.quickstyles.toggle_favorite(qs_fav.clone());
                                        this.state.status_message = if starred {
                                            "Added QuickStyle to Favorites ★".to_string()
                                        } else {
                                            "Removed QuickStyle from Favorites".to_string()
                                        };
                                        cx.notify();
                                    }))
                                    .child(if is_fav { "★" } else { "☆" }),
                            ),
                    );
                }

                let mut props_col = div()
                    .id("ed-tool-props-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(8.0))
                    .overflow_y_scroll();

                // Contextual 1: Shape Library (`ToolMode::Shape`)
                if active_tool == ToolMode::Shape {
                    let mut shape_grid = div().flex().flex_row().flex_wrap().gap(px(4.0));
                    for &(kind, label) in EditorShellState::SHAPE_LIBRARY {
                        let is_sel = self.state.selected_shape_kind == kind;
                        let kind_s = kind.to_string();
                        shape_grid = shape_grid.child(
                            Button::new(format!("shape-kind-{kind}"), label)
                                .variant(if is_sel {
                                    ButtonVariant::Primary
                                } else {
                                    ButtonVariant::Secondary
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state.selected_shape_kind = kind_s.clone();
                                    this.state.status_message = format!("Shape: {label}");
                                    cx.notify();
                                })),
                        );
                    }
                    let filled = self.state.shape_filled;
                    let dashed = self.state.shape_dashed;
                    props_col = props_col
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.text_secondary)
                                .child("Shape Library"),
                        )
                        .child(shape_grid)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(4.0))
                                .child(
                                    Button::new("shape-outline", "□ Outline")
                                        .variant(if !filled {
                                            ButtonVariant::Primary
                                        } else {
                                            ButtonVariant::Secondary
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.shape_filled = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("shape-filled", "■ Filled")
                                        .variant(if filled {
                                            ButtonVariant::Primary
                                        } else {
                                            ButtonVariant::Secondary
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.shape_filled = true;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new(
                                        "shape-dash",
                                        if dashed { "╌ Dashed" } else { "— Solid" },
                                    )
                                    .variant(if dashed {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.shape_dashed = !this.state.shape_dashed;
                                        cx.notify();
                                    })),
                                ),
                        );
                }

                // Contextual 2: Stamp Library (`ToolMode::Stamp`)
                if active_tool == ToolMode::Stamp {
                    let mut cat_row = div().flex().flex_row().flex_wrap().gap(px(4.0));
                    for &cat in StampSearcher::CATEGORIES {
                        let active_cat = self.state.stamp_category_filter == cat;
                        let cat_s = cat.to_string();
                        cat_row = cat_row.child(
                            Button::new(format!("stamp-cat-{cat}"), cat)
                                .variant(if active_cat {
                                    ButtonVariant::Primary
                                } else {
                                    ButtonVariant::Ghost
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state.stamp_category_filter = cat_s.clone();
                                    cx.notify();
                                })),
                        );
                    }
                    let mut stamp_grid = div().flex().flex_row().flex_wrap().gap(px(5.0));
                    let filtered_stamps: Vec<_> = self
                        .state
                        .stamps
                        .search(&self.state.stamp_category_filter)
                        .into_iter()
                        .cloned()
                        .collect();
                    for st in filtered_stamps {
                        let is_sel = self.state.selected_stamp_id == st.id;
                        let st_id = st.id.clone();
                        let st_name = st.name.clone();
                        stamp_grid = stamp_grid.child(
                            div()
                                .id(SharedString::from(format!("stamp-item-{}", st.id)))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .w(px(116.0))
                                .px(px(6.0))
                                .py(px(5.0))
                                .rounded(theme.radius_sm)
                                .bg(if is_sel {
                                    gpui::Hsla::from(rgba(0x2563EB22))
                                } else {
                                    theme.surface_elevated
                                })
                                .border_1()
                                .border_color(if is_sel {
                                    gpui::Hsla::from(rgb(0x2563EB))
                                } else {
                                    theme.border
                                })
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state.selected_stamp_id = st_id.clone();
                                    this.state.status_message =
                                        format!("Selected Stamp: {st_name} — click or drag on canvas");
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(22.0))
                                        .rounded(px(11.0))
                                        .bg(rgb(st.accent_hex))
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .child(st.badge),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_primary)
                                        .child(st.name),
                                ),
                        );
                    }
                    props_col = props_col
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.text_secondary)
                                .child("Stamp Library"),
                        )
                        .child(cat_row)
                        .child(stamp_grid);
                }

                // Contextual 3: Text & Callout Controls (`ToolMode::Text` / `ToolMode::Callout` / active edit)
                if matches!(active_tool, ToolMode::Text | ToolMode::Callout)
                    || self.state.editing_text_obj.is_some()
                {
                    let is_editing = self.state.editing_text_obj.is_some();
                    let preview_txt = if self.state.inline_text.content.is_empty() {
                        "(Click canvas & type text...)".to_string()
                    } else {
                        self.state.inline_text.content.clone()
                    };
                    props_col = props_col
                        .child(
                            NumericStepper::new("ed-font-size", self.state.font_size, 8.0, 96.0)
                                .label("Font Size")
                                .step(2.0)
                                .unit("pt")
                                .on_change(cx.listener(|this, &v, _, cx| {
                                    this.state.font_size = v;
                                    this.state.sync_active_text_to_session();
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .p(px(6.0))
                                .rounded(theme.radius_sm)
                                .bg(theme.surface_elevated)
                                .border_1()
                                .border_color(if is_editing {
                                    gpui::Hsla::from(rgb(0x2563EB))
                                } else {
                                    theme.border
                                })
                                .text_xs()
                                .text_color(theme.text_primary)
                                .child(preview_txt),
                        )
                        .when(is_editing, |col| {
                            col.child(
                                Button::new("ed-finish-text", "✓ Finish Text (Esc)")
                                    .variant(ButtonVariant::Primary)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.finish_text_editing();
                                        cx.notify();
                                    })),
                            )
                        });
                }

                // Contextual 4: Pending Crop Confirmation (`ToolMode::Crop`)
                if let Some(crop_r) = self.state.pending_crop_rect {
                    props_col = props_col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .p(px(8.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.surface_elevated)
                            .border_1()
                            .border_color(rgb(0xF5A623))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child(format!(
                                        "Crop Area: {:.0} × {:.0} px",
                                        crop_r.width, crop_r.height
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(6.0))
                                    .child(
                                        Button::new("prop-crop-apply", "✓ Crop (Enter)")
                                            .variant(ButtonVariant::Capture)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                let _ = this.state.confirm_pending_crop();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("prop-crop-cancel", "✕ Cancel")
                                            .variant(ButtonVariant::Secondary)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.cancel_pending_crop();
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    );
                }

                // Contextual 5: Active Selection Actions (`ToolMode::Selection`)
                if let Some(sel_r) = self.state.active_selection_rect {
                    props_col = props_col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .p(px(8.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.surface_elevated)
                            .border_1()
                            .border_color(rgb(0x00A4E4))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_primary)
                                    .child(format!(
                                        "Selection: {:.0} × {:.0} px",
                                        sel_r.width, sel_r.height
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(6.0))
                                    .child(
                                        Button::new("prop-sel-copy", "Copy")
                                            .variant(ButtonVariant::Primary)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if let Err(e) =
                                                    this.state.copy_selected_region_to_clipboard()
                                                {
                                                    this.state.status_message = e;
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("prop-sel-del", "Clear (Del)")
                                            .variant(ButtonVariant::Secondary)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                let _ = this.state.delete_selected_region();
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    );
                }

                // Contextual 6: Magic Wand Controls (`ToolMode::MagicWand` or active Magic Wand mask)
                if active_tool == ToolMode::MagicWand || self.state.magic_wand.active_mask.is_some() {
                    let is_contig = self.state.magic_wand.mode == MagicWandMode::Contiguous;
                    let mw_tol = self.state.magic_wand.tolerance;
                    let mw_px = self.state.magic_wand.selected_pixel_count();
                    props_col = props_col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .p(px(8.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.surface_elevated)
                            .border_1()
                            .border_color(rgb(0x00A4E4))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child(if mw_px > 0 {
                                        format!("Magic Wand · {mw_px} px selected")
                                    } else {
                                        "Magic Wand · Click a color on canvas".to_string()
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(4.0))
                                    .child(
                                        Button::new("mw-mode-contig", "Contiguous")
                                            .variant(if is_contig {
                                                ButtonVariant::Primary
                                            } else {
                                                ButtonVariant::Secondary
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.magic_wand.mode =
                                                    MagicWandMode::Contiguous;
                                                if let Some(cnt) = this
                                                    .state
                                                    .magic_wand
                                                    .refresh_if_seeded(&this.state.session)
                                                {
                                                    this.state.status_message = format!(
                                                        "Magic Wand (Contiguous): {cnt} px selected"
                                                    );
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("mw-mode-global", "Global Color")
                                            .variant(if !is_contig {
                                                ButtonVariant::Primary
                                            } else {
                                                ButtonVariant::Secondary
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.magic_wand.mode = MagicWandMode::Global;
                                                if let Some(cnt) = this
                                                    .state
                                                    .magic_wand
                                                    .refresh_if_seeded(&this.state.session)
                                                {
                                                    this.state.status_message = format!(
                                                        "Magic Wand (Global): {cnt} px selected"
                                                    );
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                Slider::new("mw-tolerance", mw_tol, 1.0, 100.0)
                                    .label("Tolerance")
                                    .width(px(224.0))
                                    .on_change(cx.listener(|this, &v: &f32, _, cx| {
                                        this.state.magic_wand.tolerance = v.round();
                                        let _ = this
                                            .state
                                            .magic_wand
                                            .refresh_if_seeded(&this.state.session);
                                        cx.notify();
                                    })),
                            )
                            .when(mw_px > 0, |card| {
                                card.child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .flex_wrap()
                                        .gap(px(5.0))
                                        .child(
                                            Button::new("mw-del-btn", "Clear (Del)")
                                                .variant(ButtonVariant::Capture)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let n = this
                                                        .state
                                                        .magic_wand
                                                        .delete_selected_pixels(
                                                            &mut this.state.session,
                                                        );
                                                    this.state.status_message = format!(
                                                        "Erased {n} px to transparent (Ctrl+Z to undo)"
                                                    );
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("mw-fill-btn", "Fill Color")
                                                .variant(ButtonVariant::Primary)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let col = this.state.active_color_rgba();
                                                    let n = this
                                                        .state
                                                        .magic_wand
                                                        .fill_selected_pixels(
                                                            &mut this.state.session,
                                                            col,
                                                        );
                                                    this.state.status_message = format!(
                                                        "Filled {n} px with #{:06X}",
                                                        this.state.selected_color
                                                    );
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("mw-copy-btn", "Copy")
                                                .variant(ButtonVariant::Secondary)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let Some((w, h, data)) = this
                                                        .state
                                                        .magic_wand
                                                        .copy_selected_pixels_rgba(
                                                            &this.state.session,
                                                        )
                                                    {
                                                        let _ = snagsoup_win_platform::clipboard::write_image_to_clipboard(
                                                            &snagsoup_win_platform::clipboard::RgbaImage { width: w, height: h, data },
                                                        );
                                                        this.state.status_message = format!(
                                                            "Copied Magic Wand cutout ({w}×{h} px)"
                                                        );
                                                        cx.notify();
                                                    }
                                                })),
                                        )
                                        .child(
                                            Button::new("mw-desel-btn", "Deselect")
                                                .variant(ButtonVariant::Ghost)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.state.magic_wand.clear_selection();
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            }),
                    );
                }

                // Contextual 7: Smart Move Controls (`ToolMode::Move` / `ToolMode::SmartMove`)
                if matches!(active_tool, ToolMode::Move | ToolMode::SmartMove) {
                    let sm_on = self.state.smart_move.enabled;
                    let sm_cnt = self.state.smart_move.detected.len();
                    props_col = props_col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .p(px(8.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.surface_elevated)
                            .border_1()
                            .border_color(if sm_on {
                                gpui::Hsla::from(rgb(0x2563EB))
                            } else {
                                theme.border
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child("Smart Move (Auto-Detect UI)"),
                                    )
                                    .child(
                                        Button::new(
                                            "sm-toggle-btn",
                                            if sm_on { "ON" } else { "OFF" },
                                        )
                                        .variant(if sm_on {
                                            ButtonVariant::Primary
                                        } else {
                                            ButtonVariant::Secondary
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if this.state.smart_move.enabled {
                                                this.state.smart_move.disable();
                                                this.state.status_message =
                                                    "Smart Move OFF — moving annotations only"
                                                        .into();
                                            } else {
                                                let n = this
                                                    .state
                                                    .smart_move
                                                    .scan(&this.state.session);
                                                this.state.status_message = format!(
                                                    "Smart Move ON: detected {n} movable UI elements — drag any box!"
                                                );
                                            }
                                            cx.notify();
                                        })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(if sm_on {
                                        format!(
                                            "{sm_cnt} UI elements detected. Hover & drag any highlighted box to move it and auto-fill the background."
                                        )
                                    } else {
                                        "Turn ON to detect buttons, icons & text blocks inside flat screenshots so you can drag them anywhere."
                                            .to_string()
                                    }),
                            )
                            .when(sm_on, |card| {
                                card.child(
                                    Button::new("sm-rescan-btn", "↻ Re-scan Image")
                                        .variant(ButtonVariant::Secondary)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let n =
                                                this.state.smart_move.scan(&this.state.session);
                                            this.state.status_message =
                                                format!("Re-scanned: {n} UI elements detected");
                                            cx.notify();
                                        })),
                                )
                            }),
                    );
                }

                // Contextual 8: Simplify Controls (`ToolMode::Simplify`)
                if active_tool == ToolMode::Simplify {
                    let simp_on = self.state.simplify_ui.enabled;
                    let simp_detail = self.state.simplify_ui.detail;
                    let show_orig = self.state.simplify_ui.show_original;
                    let pal_idx = self.state.simplify_ui.active_palette_idx;
                    let palettes = SimplifyUiFloatyState::preset_palettes();

                    let mut pal_row = div().flex().flex_row().flex_wrap().gap(px(4.0));
                    for (p_i, pal) in palettes.iter().enumerate() {
                        pal_row = pal_row.child(
                            Button::new(format!("simp-pal-{p_i}"), pal.name.clone())
                                .variant(if pal_idx == p_i {
                                    ButtonVariant::Primary
                                } else {
                                    ButtonVariant::Secondary
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state
                                        .simplify_ui
                                        .select_palette(p_i, &mut this.state.session);
                                    cx.notify();
                                })),
                        );
                    }

                    props_col = props_col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .p(px(8.0))
                            .rounded(theme.radius_sm)
                            .bg(theme.surface_elevated)
                            .border_1()
                            .border_color(if simp_on {
                                gpui::Hsla::from(rgb(0x2563EB))
                            } else {
                                theme.border
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child("Auto-Simplify (Wireframe)"),
                                    )
                                    .child(
                                        Button::new(
                                            "simp-auto-toggle",
                                            if simp_on { "ON" } else { "Auto Simplify" },
                                        )
                                        .variant(if simp_on {
                                            ButtonVariant::Primary
                                        } else {
                                            ButtonVariant::Capture
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if this.state.simplify_ui.enabled {
                                                let n = this
                                                    .state
                                                    .simplify_ui
                                                    .clear_auto_simplify(&mut this.state.session);
                                                this.state.status_message = format!(
                                                    "Removed {n} Auto-Simplify shapes"
                                                );
                                            } else {
                                                let n = this
                                                    .state
                                                    .simplify_ui
                                                    .run_auto_simplify(&mut this.state.session);
                                                this.state.status_message = format!(
                                                    "Auto-Simplify generated {n} wireframe blocks"
                                                );
                                            }
                                            cx.notify();
                                        })),
                                    ),
                            )
                            .child(
                                Slider::new("simp-detail", simp_detail as f32, 1.0, 10.0)
                                    .label("Detail")
                                    .width(px(224.0))
                                    .on_change(cx.listener(|this, &v: &f32, _, cx| {
                                        let next_d = v.round().clamp(1.0, 10.0) as u8;
                                        if this.state.simplify_ui.detail != next_d {
                                            this.state.simplify_ui.detail = next_d;
                                            if this.state.simplify_ui.enabled {
                                                let _ = this
                                                    .state
                                                    .simplify_ui
                                                    .run_auto_simplify(&mut this.state.session);
                                            }
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(pal_row)
                            .when(simp_on, |card| {
                                card.child(
                                    Button::new(
                                        "simp-show-orig",
                                        if show_orig {
                                            "👁 Showing Original (Click to Hide)"
                                        } else {
                                            "👁 Show Original"
                                        },
                                    )
                                    .variant(if show_orig {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let next = !this.state.simplify_ui.show_original;
                                        this.state
                                            .simplify_ui
                                            .set_show_original(next, &mut this.state.session);
                                        cx.notify();
                                    })),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(
                                        "Tip: Drag on the canvas anytime to draw manual cover blocks.",
                                    ),
                            ),
                    );
                }

                props_col = props_col
                    .child(
                        NumericStepper::new("ed-stroke-width", self.state.stroke_width, 1.0, 64.0)
                            .label("Width")
                            .step(1.0)
                            .unit("px")
                            .on_change(cx.listener(|this, &v, _, cx| {
                                this.state.stroke_width = v;
                                cx.notify();
                            })),
                    )
                    .child(
                        Slider::new("ed-opacity", self.state.opacity, 0.0, 100.0)
                            .label("Opacity")
                            .width(px(244.0))
                            .on_change(cx.listener(|this, &v, _, cx| {
                                this.state.opacity = v;
                                cx.notify();
                            })),
                    )
                    .child(
                        ColorSwatchPicker::new("ed-color", self.state.selected_color)
                            .on_select(cx.listener(|this, &rgb_hex, _, cx| {
                                this.state.selected_color = rgb_hex;
                                if this.state.editing_text_obj.is_some() {
                                    this.state.inline_text.style.color = Some(ColorRgba::rgb(
                                        ((rgb_hex >> 16) & 0xFF) as u8,
                                        ((rgb_hex >> 8) & 0xFF) as u8,
                                        (rgb_hex & 0xFF) as u8,
                                    ));
                                    this.state.sync_active_text_to_session();
                                }
                                cx.notify();
                            }))
                            .on_eyedropper(cx.listener(|this, _, _, cx| {
                                this.state.select_tool(ToolMode::Eyedropper);
                                this.state.status_message =
                                    "Eyedropper active — click canvas to pick color".to_string();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .pt(px(6.0))
                            .border_t_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(format!("QuickStyles ({styles_count}) · ★ {fav_count}")),
                            )
                            .when(show_ghost, |row| {
                                row.child(
                                    Button::new("save-ghost-style", "+ Save Style")
                                        .variant(ButtonVariant::Secondary)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let c = ColorRgba::rgb(
                                                ((this.state.selected_color >> 16) & 0xFF) as u8,
                                                ((this.state.selected_color >> 8) & 0xFF) as u8,
                                                (this.state.selected_color & 0xFF) as u8,
                                            );
                                            this.state.quickstyles.save_ghost_style(
                                                this.state.active_tool.clone(),
                                                c,
                                                this.state.stroke_width as f64,
                                                this.state.opacity as f64,
                                            );
                                            this.state.status_message = format!(
                                                "Saved QuickStyle for {}",
                                                this.state.active_tool.as_str()
                                            );
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .when(styles_count > 0, |col| col.child(qs_grid));

                el.child(props_col)
            })
            .when(self.state.side_panel_tab == SidePanelTab::Effects, |el| {
                el.child(effects_col)
            });

        // Bottom Recent Captures Tray (Snagit-style visual image/video thumbnails)
        let active_norm_id = self
            .state
            .active_path
            .as_deref()
            .map(normalize_capture_id);
        let thumb_well_bg = if theme.mode == ThemeMode::Dark {
            gpui::Hsla::from(rgb(0x111827))
        } else {
            gpui::Hsla::from(rgb(0xE5E7EB))
        };

        let mut tray_strip = div()
            .id("recent-captures-tray")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(84.0))
            .px(px(10.0))
            .overflow_x_scroll()
            .bg(theme.panel_bg)
            .border_t_1()
            .border_color(theme.border);

        for (idx, item) in self.state.recent_captures.iter().enumerate() {
            let path_opt = item.path.clone();
            let is_active = match (&active_norm_id, &item.path) {
                (Some(act), Some(_)) => *act == item.id,
                (None, None) => idx == 0,
                _ => false,
            };
            let is_vid = item.is_video;

            let card_body = if let Some(ref thumb) = item.thumbnail {
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .rounded(px(3.0))
                    .overflow_hidden()
                    .bg(thumb_well_bg)
                    .child(
                        img(Arc::clone(thumb))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .when(is_vid, |el| {
                        el.child(
                            div()
                                .absolute()
                                .bottom(px(3.0))
                                .right(px(3.0))
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
                                .bg(rgba(0x000000BF))
                                .text_xs()
                                .text_color(rgb(0xFFFFFF))
                                .child("MP4"),
                        )
                    })
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .size_full()
                    .p(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_primary)
                            .truncate()
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(format!("{}×{}", item.dimensions.0, item.dimensions.1)),
                    )
                    .into_any_element()
            };

            let mut card = div()
                .id(SharedString::from(format!("tray-item-{idx}")))
                .flex_shrink_0()
                .w(px(108.0))
                .h(px(68.0))
                .p(px(3.0))
                .rounded(theme.radius_sm)
                .bg(theme.surface_elevated)
                .cursor_pointer()
                .hover(|s| s.border_color(theme.accent_primary));

            if is_active {
                card = card.border_2().border_color(theme.accent_primary);
            } else {
                card = card.border_1().border_color(theme.border);
            }

            tray_strip = tray_strip.child(
                card.on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(ref p) = path_opt {
                        let is_mp4 = p
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.eq_ignore_ascii_case("mp4"))
                            .unwrap_or(false);
                        let res = if is_mp4 {
                            this.state.open_video_file(p)
                        } else {
                            this.state.open_snagx(p)
                        };
                        if let Err(err) = res {
                            this.state.status_message = err;
                        }
                        cx.notify();
                    }
                }))
                .child(card_body),
            );
        }

        // Bottom Status & Zoom Bar
        let status_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(32.0))
            .px(px(10.0))
            .bg(theme.toolbar_bg)
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child(format!("Canvas: {canvas_w} × {canvas_h} px"))
                    .child(format!("Objects: {obj_count}"))
                    .child(format!("Effects: {}", self.state.applied_effects_count))
                    .child(self.state.status_message.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        Button::new("toggle-tray", if self.state.show_recent_tray { "Hide Tray" } else { "Show Tray" })
                            .variant(ButtonVariant::Ghost)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_recent_tray = !this.state.show_recent_tray;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("toggle-side", if self.state.show_side_panel { "Hide Panel" } else { "Show Panel" })
                            .variant(ButtonVariant::Ghost)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.show_side_panel = !this.state.show_side_panel;
                                cx.notify();
                            })),
                    )
                    .child(
                        Slider::new("zoom-slider", self.state.zoom_percent, 10.0, 800.0)
                            .width(px(110.0))
                            .on_change(cx.listener(|this, &z, _, cx| {
                                this.state.set_zoom(z);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("zoom-fit", format!("{:.0}%", self.state.zoom_percent))
                            .variant(ButtonVariant::Ghost)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.set_zoom(100.0);
                                cx.notify();
                            })),
                    ),
            );

        let focus_handle = self
            .focus_handle
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        if self.state.workspace_view == WorkspaceView::Editor
            && !focus_handle.contains_focused(window, cx)
        {
            focus_handle.focus(window, cx);
        }
        let fh_for_canvas = focus_handle.clone();
        let fh_for_paint = focus_handle.clone();
        let entity_for_paint = cx.entity();
        let origin_for_prepaint = Rc::clone(&self.canvas_origin);
        let size_for_prepaint = Rc::clone(&self.canvas_size_cell);
        let any_popover_open =
            self.state.show_create_menu || self.state.show_share_menu || self.state.show_more_tools;
        let create_btn_rect = self.create_btn_bounds.get();
        let more_btn_rect = self.more_btn_bounds.get();
        let share_btn_rect = self.share_btn_bounds.get();

        // Build live interactive canvas overlays (drag marquee, selection, pending crop, inline text)
        let active_region_drag = match &self.state.active_drag {
            Some(CanvasDragOp::RegionTool { start, current }) => Some((
                self.state.active_tool.clone(),
                RectF::from_points(*start, *current),
            )),
            _ => None,
        };
        let editing_text_box = self.state.editing_text_obj.and_then(|idx| {
            let obj = self.state.session.page.capture_objects.get(idx)?;
            let r = RectF::from_points(
                obj.start_point.unwrap_or(PointF::new(0.0, 0.0)),
                obj.end_point.unwrap_or(PointF::new(160.0, 64.0)),
            );
            Some((r, self.state.inline_text.content.clone()))
        });
        let smart_move_boxes = if matches!(
            self.state.active_tool,
            ToolMode::Move | ToolMode::SmartMove
        ) && self.state.smart_move.enabled
        {
            self.state
                .smart_move
                .detected
                .iter()
                .enumerate()
                .map(|(i, el)| {
                    let r = RectF::new(
                        el.bounds.x as f64,
                        el.bounds.y as f64,
                        el.bounds.width as f64,
                        el.bounds.height as f64,
                    );
                    (r, self.state.smart_move.hovered_idx == Some(i))
                })
                .collect()
        } else {
            Vec::new()
        };
        let overlays = CanvasInteractiveOverlays {
            active_region_drag,
            active_selection_rect: self.state.active_selection_rect,
            pending_crop_rect: self.state.pending_crop_rect,
            editing_text_box,
            magic_wand_overlay: self.state.magic_wand.mask_overlay.clone(),
            smart_move_boxes,
        };
        let pending_crop_for_bar = self.state.pending_crop_rect;

        if self.state.workspace_view != WorkspaceView::Editor || self.state.video_player.path.is_some() {
            if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                snagsoup_win_platform::cursor::sync_hwnd_canvas_cursor(hwnd, None, None);
            }
        }
        let custom_tool_hcursor = self
            .state
            .active_custom_tool_cursor_mode()
            .and_then(|mode| cached_tool_hcursor(&mode, dev_scale));

        div()
            .id("editor-root-shell")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.app_bg)
            .text_color(theme.text_primary)
            .track_focus(&focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                let key = ev.keystroke.key.as_str();
                let ctrl = ev.keystroke.modifiers.control;

                if this.state.editing_text_obj.is_some() {
                    let shift = ev.keystroke.modifiers.shift;
                    let key_lower = key.to_ascii_lowercase();
                    match key_lower.as_str() {
                        "escape" => {
                            this.state.finish_text_editing();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "enter" => {
                            this.state.inline_text.insert_text("\n");
                            this.state.sync_active_text_to_session();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "backspace" => {
                            this.state.inline_text.backspace();
                            this.state.sync_active_text_to_session();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "delete" => {
                            this.state.inline_text.delete_forward();
                            this.state.sync_active_text_to_session();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "left" => {
                            this.state.inline_text.move_left();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "right" => {
                            this.state.inline_text.move_right();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "home" => {
                            this.state.inline_text.move_home();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "end" => {
                            this.state.inline_text.move_end();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "a" if ctrl => {
                            this.state.inline_text.select_all();
                            cx.stop_propagation();
                            cx.notify();
                        }
                        "z" if ctrl && shift => {
                            if this.state.perform_redo() {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }
                        "z" if ctrl => {
                            if this.state.perform_undo() {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }
                        "y" if ctrl => {
                            if this.state.perform_redo() {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }
                        _ => {}
                    }
                    return;
                }

                if this.state.effects_gallery.editing_caption_text {
                    if let Some(cap_idx) = this
                        .state
                        .effects_gallery
                        .entries
                        .iter()
                        .position(|e| e.id == "caption")
                    {
                        match key {
                            "escape" | "enter" => {
                                this.state.effects_gallery.editing_caption_text = false;
                                cx.stop_propagation();
                                cx.notify();
                            }
                            "backspace" => {
                                if let Some(EffectConfig::Caption {
                                    mut text,
                                    position,
                                    font_family,
                                    font_size,
                                    text_color,
                                    background_color,
                                }) = this
                                    .state
                                    .effects_gallery
                                    .entries
                                    .get(cap_idx)
                                    .map(|e| e.config.clone())
                                {
                                    text.pop();
                                    this.state.effects_gallery.update_effect_config(
                                        &mut this.state.session,
                                        cap_idx,
                                        EffectConfig::Caption {
                                            text,
                                            position,
                                            font_family,
                                            font_size,
                                            text_color,
                                            background_color,
                                        },
                                        false,
                                    );
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }
                            _ if !ctrl => {
                                let typed = if key == "space" {
                                    Some(" ".to_string())
                                } else {
                                    ev.keystroke
                                        .key_char
                                        .clone()
                                        .or_else(|| (key.chars().count() == 1).then(|| key.to_string()))
                                };
                                if let Some(ch_str) = typed {
                                    if let Some(EffectConfig::Caption {
                                        mut text,
                                        position,
                                        font_family,
                                        font_size,
                                        text_color,
                                        background_color,
                                    }) = this
                                        .state
                                        .effects_gallery
                                        .entries
                                        .get(cap_idx)
                                        .map(|e| e.config.clone())
                                    {
                                        text.push_str(&ch_str);
                                        this.state.effects_gallery.update_effect_config(
                                            &mut this.state.session,
                                            cap_idx,
                                            EffectConfig::Caption {
                                                text,
                                                position,
                                                font_family,
                                                font_size,
                                                text_color,
                                                background_color,
                                            },
                                            false,
                                        );
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }
                            _ => {}
                        }
                    } else {
                        this.state.effects_gallery.editing_caption_text = false;
                    }
                    return;
                }

                let shift = ev.keystroke.modifiers.shift;
                let key_lower = key.to_ascii_lowercase();
                match key_lower.as_str() {
                    "enter" if this.state.pending_crop_rect.is_some() => {
                        let _ = this.state.confirm_pending_crop();
                        cx.notify();
                    }
                    "escape" => {
                        if this.state.grab_edit_text.modal_open {
                            this.state.grab_edit_text.cancel_running_job();
                            this.state.grab_edit_text.modal_open = false;
                            this.state.status_message = "Closed Grab Text".to_string();
                        } else if this.state.pending_crop_rect.is_some() {
                            this.state.cancel_pending_crop();
                        } else if this.state.magic_wand.active_mask.is_some() {
                            this.state.magic_wand.clear_selection();
                            this.state.status_message = "Cleared Magic Wand selection".to_string();
                        } else {
                            this.state.active_selection_rect = None;
                            this.state.close_all_popovers();
                        }
                        cx.notify();
                    }
                    "backspace" | "delete" => {
                        if this.state.magic_wand.active_mask.is_some() {
                            let n = this
                                .state
                                .magic_wand
                                .delete_selected_pixels(&mut this.state.session);
                            this.state.status_message =
                                format!("Erased {n} px to transparent (Ctrl+Z to undo)");
                            cx.notify();
                        } else if this.state.active_selection_rect.is_some() {
                            let _ = this.state.delete_selected_region();
                            cx.notify();
                        } else if !this.state.gizmos_g1.selected_indices.is_empty() {
                            this.state.gizmos_g1.delete_selected(&mut this.state.session);
                            this.state.status_message = "Deleted selected object(s)".to_string();
                            cx.notify();
                        }
                    }
                    "c" if ctrl => {
                        if this.state.magic_wand.active_mask.is_some() {
                            if let Some((w, h, data)) = this
                                .state
                                .magic_wand
                                .copy_selected_pixels_rgba(&this.state.session)
                            {
                                let _ = snagsoup_win_platform::clipboard::write_image_to_clipboard(
                                    &snagsoup_win_platform::clipboard::RgbaImage {
                                        width: w,
                                        height: h,
                                        data,
                                    },
                                );
                                this.state.status_message =
                                    format!("Copied Magic Wand cutout ({w}×{h} px)");
                                cx.notify();
                            }
                        } else if this.state.active_selection_rect.is_some() {
                            if let Err(e) = this.state.copy_selected_region_to_clipboard() {
                                this.state.status_message = e;
                            }
                            cx.notify();
                        }
                    }
                    "z" if ctrl && shift => {
                        if this.state.perform_redo() {
                            cx.notify();
                        }
                    }
                    "z" if ctrl => {
                        if this.state.perform_undo() {
                            cx.notify();
                        }
                    }
                    "y" if ctrl => {
                        if this.state.perform_redo() {
                            cx.notify();
                        }
                    }
                    _ if !ctrl && !ev.keystroke.modifiers.alt && !this.state.grab_edit_text.modal_open => {
                        if let Some(found) = this
                            .state
                            .tools
                            .iter()
                            .find(|t| t.shortcut.is_some_and(|sc| sc.eq_ignore_ascii_case(&key_lower)))
                            .map(|t| t.mode.clone())
                        {
                            this.state.select_tool(found);
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .child(top_bar)
            // Center Workspace + Right Side Panel
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .child(match self.state.workspace_view {
                        WorkspaceView::Editor if self.state.video_player.path.is_some() => {
                            render_video_stage(&self.state.video_player, theme).into_any_element()
                        }
                        WorkspaceView::Editor => {
                            let canvas_cursor = self.state.canvas_cursor_style();
                            let spotlight_gizmo_opt =
                                if self.state.side_panel_tab == SidePanelTab::Effects {
                                    self.state.effects_gallery.active_spotlight_rect()
                                } else {
                                    None
                                };
                            let brush_ring_opt = if spotlight_gizmo_opt.is_none()
                                && matches!(
                                    self.state.active_tool,
                                    ToolMode::Pen | ToolMode::Highlighter | ToolMode::Eraser
                                )
                            {
                                self.state.hover_local_pos.map(|(lx, ly)| {
                                    let diam = (self.state.stroke_width
                                        * self.state.viewport.effective_logical_zoom())
                                    .clamp(6.0, 220.0);
                                    (lx, ly, diam, self.state.active_tool == ToolMode::Eraser)
                                })
                            } else {
                                None
                            };

                            div()
                                .id("editor-interactive-canvas")
                                .relative()
                                .flex()
                                .flex_1()
                                .size_full()
                                .overflow_hidden()
                                .cursor(canvas_cursor)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                        this.state.close_all_popovers();
                                        fh_for_canvas.focus(window, cx);
                                        let (ox, oy) = this.canvas_origin.get();
                                        let sx: f32 = ev.position.x.into();
                                        let sy: f32 = ev.position.y.into();
                                        let local_x = (sx - ox).max(0.0);
                                        let local_y = (sy - oy).max(0.0);
                                        this.state.pointer_down(
                                            local_x,
                                            local_y,
                                            ev.modifiers.shift,
                                            ev.click_count,
                                        );
                                        cx.notify();
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    |this, ev: &MouseMoveEvent, _window, cx| {
                                        let (ox, oy) = this.canvas_origin.get();
                                        let sx: f32 = ev.position.x.into();
                                        let sy: f32 = ev.position.y.into();
                                        let local_x = (sx - ox).max(0.0);
                                        let local_y = (sy - oy).max(0.0);
                                        if this.state.pointer_move(local_x, local_y) {
                                            cx.notify();
                                        }
                                    },
                                ))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                                        this.state.pointer_up();
                                        cx.notify();
                                    }),
                                )
                            .on_scroll(cx.listener(
                                |this, ev: &ScrollEvent, _window, cx| {
                                    let delta_px = match ev.delta {
                                        ScrollDelta::Lines(pt) => (pt.x * 24.0, pt.y * 24.0),
                                        ScrollDelta::Pixels(pt) => (pt.x.into(), pt.y.into()),
                                    };
                                    if ev.modifiers.control {
                                        let factor = if delta_px.1 > 0.0 { 1.12 } else { 0.89 };
                                        let (ox, oy) = this.canvas_origin.get();
                                        let sx: f32 = ev.position.x.into();
                                        let sy: f32 = ev.position.y.into();
                                        let next_zoom = this.state.viewport.zoom * factor;
                                        this.state.viewport.zoom_anchored_at(
                                            next_zoom,
                                            (sx - ox).max(0.0),
                                            (sy - oy).max(0.0),
                                        );
                                        this.state.zoom_percent =
                                            this.state.viewport.zoom_percent();
                                    } else {
                                        this.state.viewport.pan_by(delta_px.0, delta_px.1);
                                    }
                                    cx.notify();
                                },
                            ))
                            .child(render_canvas_surface(
                                &self.state.viewport,
                                &self.state.gpu_cache,
                                canvas_w,
                                canvas_h,
                                &overlays,
                                theme,
                            ))
                            // Measure exact canvas screen origin + size & bind Windows IME / text input handler
                            .child(
                                canvas(
                                    move |bounds, _window, _cx| {
                                        origin_for_prepaint
                                            .set((bounds.origin.x.into(), bounds.origin.y.into()));
                                        size_for_prepaint
                                            .set((bounds.size.width.into(), bounds.size.height.into()));
                                    },
                                    move |bounds, _, window, cx| {
                                        if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                                            let ox: f32 = bounds.origin.x.into();
                                            let oy: f32 = bounds.origin.y.into();
                                            let cw: f32 = bounds.size.width.into();
                                            let ch: f32 = bounds.size.height.into();
                                            let rect_phys = (cw > 1.0 && ch > 1.0).then(|| {
                                                (
                                                    (ox * dev_scale).round() as i32,
                                                    (oy * dev_scale).round() as i32,
                                                    ((ox + cw) * dev_scale).round() as i32,
                                                    ((oy + ch) * dev_scale).round() as i32,
                                                )
                                            });
                                            snagsoup_win_platform::cursor::sync_hwnd_canvas_cursor(
                                                hwnd,
                                                rect_phys,
                                                custom_tool_hcursor,
                                            );
                                        }
                                        window.handle_input(
                                            &fh_for_paint,
                                            ElementInputHandler::new(
                                                bounds,
                                                entity_for_paint.clone(),
                                            ),
                                            cx,
                                        );
                                    },
                                )
                                .absolute()
                                .inset_0(),
                            )
                            // Live stroke-width ring indicator for Pen / Highlighter / Eraser
                            .when_some(brush_ring_opt, |canvas_el, (bx, by, diam, is_eraser)| {
                                let r = diam * 0.5;
                                let ring_color = if is_eraser {
                                    gpui::Hsla::from(rgb(0xEF4444))
                                } else {
                                    let c = self.state.selected_color;
                                    gpui::Hsla::from(rgb(c))
                                };
                                canvas_el.child(
                                    div()
                                        .absolute()
                                        .left(px(bx - r))
                                        .top(px(by - r))
                                        .size(px(diam))
                                        .rounded(px(r))
                                        .border_1()
                                        .border_color(ring_color),
                                )
                            })
                            // Interactive 8-Handle Spotlight & Magnify Gizmo Overlay on Canvas
                            .when_some(spotlight_gizmo_opt, |canvas_el, (_eff_idx, spot_r)| {
                                let eff_z = self.state.viewport.effective_logical_zoom();
                                let sx = self.state.viewport.pan_x + (spot_r.x as f32) * eff_z;
                                let sy = self.state.viewport.pan_y + (spot_r.y as f32) * eff_z;
                                let sw = ((spot_r.width as f32) * eff_z).max(4.0);
                                let sh = ((spot_r.height as f32) * eff_z).max(4.0);
                                let hs = 9.0f32;
                                let hh = hs * 0.5;
                                let handle_pts: &[(f32, f32)] = &[
                                    (sx, sy),
                                    (sx + sw * 0.5, sy),
                                    (sx + sw, sy),
                                    (sx + sw, sy + sh * 0.5),
                                    (sx + sw, sy + sh),
                                    (sx + sw * 0.5, sy + sh),
                                    (sx, sy + sh),
                                    (sx, sy + sh * 0.5),
                                ];

                                let mut layer = canvas_el
                                    .child(
                                        div()
                                            .absolute()
                                            .left(px(sx))
                                            .top(px(sy))
                                            .w(px(sw))
                                            .h(px(sh))
                                            .border_2()
                                            .border_color(rgb(0x0EA5E9)),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .left(px(sx))
                                            .top(px((sy - 24.0).max(4.0)))
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(4.0))
                                            .bg(rgba(0x0EA5E9EB))
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xFFFFFF))
                                            .child(format!(
                                                "Spotlight {:.0}×{:.0} px",
                                                spot_r.width, spot_r.height
                                            )),
                                    );

                                for &(hx, hy) in handle_pts {
                                    layer = layer.child(
                                        div()
                                            .absolute()
                                            .left(px(hx - hh))
                                            .top(px(hy - hh))
                                            .size(px(hs))
                                            .rounded(px(2.0))
                                            .bg(rgb(0xFFFFFF))
                                            .border_2()
                                            .border_color(rgb(0x0284C7)),
                                    );
                                }
                                layer
                            })
                            // Floating Snagit Crop Confirmation Bar (`✓ Crop` / `✕ Cancel`) right below pending crop box!
                            .when_some(pending_crop_for_bar, |canvas_el, crop_r| {
                                let eff_z = self.state.viewport.effective_logical_zoom();
                                let bar_x = (self.state.viewport.pan_x
                                    + (crop_r.x as f32 + crop_r.width as f32 * 0.5) * eff_z
                                    - 92.0)
                                    .max(8.0);
                                let bar_y = (self.state.viewport.pan_y
                                    + (crop_r.y as f32 + crop_r.height as f32) * eff_z
                                    + 10.0)
                                    .max(8.0);
                                canvas_el.child(
                                    div()
                                        .id("floating-crop-confirm-bar")
                                        .occlude()
                                        .absolute()
                                        .left(px(bar_x))
                                        .top(px(bar_y))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .p(px(6.0))
                                        .rounded(theme.radius_md)
                                        .bg(theme.surface_elevated)
                                        .border_1()
                                        .border_color(rgb(0xF5A623))
                                        .shadow_lg()
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_mouse_up(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .child(
                                            Button::new("canvas-crop-confirm", "✓ Crop")
                                                .variant(ButtonVariant::Capture)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    let _ = this.state.confirm_pending_crop();
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("canvas-crop-cancel", "✕ Cancel")
                                                .variant(ButtonVariant::Secondary)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.state.cancel_pending_crop();
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            })
                            .into_any_element()
                        }
                        WorkspaceView::Library => {
                            self.ensure_library_view(cx).into_any_element()
                        }
                        WorkspaceView::Template => {
                            self.ensure_template_view(cx).into_any_element()
                        }
                        WorkspaceView::Combine => {
                            self.ensure_combine_view(cx).into_any_element()
                        }
                        WorkspaceView::VideoFromImages => {
                            self.ensure_vfi_view(cx).into_any_element()
                        }
                        WorkspaceView::Xliff => {
                            self.ensure_xliff_view(cx).into_any_element()
                        }
                    })
                    .when(self.state.show_side_panel && viewport_w >= 680.0, |el| {
                        el.child(side_panel)
                    }),
            )
            .when(self.state.video_player.path.is_some(), |el| {
                let is_playing = self.state.video_player.playing;
                let has_sel = self.state.video_player.has_selection_range();
                let time_label = format!(
                    "{} / {}",
                    format_timestamp_ms(self.state.video_player.playhead_ms),
                    format_timestamp_ms(self.state.video_player.duration_ms())
                );
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .bg(theme.toolbar_bg)
                        .border_t_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .px(px(12.0))
                                .pt(px(8.0))
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            Button::new(
                                                "vid-play-pause",
                                                if is_playing { "⏸ Pause" } else { "▶ Play" },
                                            )
                                            .variant(ButtonVariant::Primary)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.video_player.playing =
                                                    !this.state.video_player.playing;
                                                if this.state.video_player.playing {
                                                    let fps = this
                                                        .state
                                                        .video_player
                                                        .metadata
                                                        .map_or(30.0, |m| m.fps.clamp(5.0, 60.0));
                                                    let step_ms =
                                                        (1000.0 / fps).round().max(16.0) as u64;
                                                    cx.spawn(async move |weak, cx| {
                                                        loop {
                                                            cx.background_executor()
                                                                .timer(std::time::Duration::from_millis(
                                                                    step_ms,
                                                                ))
                                                                .await;
                                                            let keep_going = weak
                                                                .update(cx, |view, cx| {
                                                                    if !view.state.video_player.playing {
                                                                        return false;
                                                                    }
                                                                    let has_more = view
                                                                        .state
                                                                        .video_player
                                                                        .advance_playback_frame();
                                                                    cx.notify();
                                                                    has_more
                                                                })
                                                                .unwrap_or(false);
                                                            if !keep_going {
                                                                break;
                                                            }
                                                        }
                                                    })
                                                    .detach();
                                                }
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            Button::new("vid-step-prev", "◀ Frame")
                                                .variant(ButtonVariant::Secondary)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.state.video_player.playing = false;
                                                    this.state.video_player.step_frame(-1);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("vid-step-next", "Frame ▶")
                                                .variant(ButtonVariant::Secondary)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.state.video_player.playing = false;
                                                    this.state.video_player.step_frame(1);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("vid-set-in", "[ In")
                                                .variant(ButtonVariant::Ghost)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let cur = this.state.video_player.playhead_ms;
                                                    let out = this
                                                        .state
                                                        .video_player
                                                        .out_point_ms
                                                        .max(cur);
                                                    this.state
                                                        .video_player
                                                        .set_selection_range(cur, out);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("vid-set-out", "Out ]")
                                                .variant(ButtonVariant::Ghost)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let cur = this.state.video_player.playhead_ms;
                                                    let in_ms = this
                                                        .state
                                                        .video_player
                                                        .in_point_ms
                                                        .min(cur);
                                                    this.state
                                                        .video_player
                                                        .set_selection_range(in_ms, cur);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.text_secondary)
                                                .child(time_label),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .when(has_sel, |row| {
                                            row.child(
                                                Button::new("vid-cut-btn", "✂ Cut")
                                                    .variant(ButtonVariant::Capture)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        let ts = SystemTime::now()
                                                            .duration_since(UNIX_EPOCH)
                                                            .map(|d| d.as_secs())
                                                            .unwrap_or(0);
                                                        let out = capture_storage_dir()
                                                            .join(format!("VID-trim-{ts}.mp4"));
                                                        match this
                                                            .state
                                                            .video_player
                                                            .apply_cut_selection(&out)
                                                        {
                                                            Ok(_) => {
                                                                let _ = this
                                                                    .state
                                                                    .open_video_file(&out);
                                                                this.state.status_message = format!(
                                                                    "Saved trimmed video {}",
                                                                    out.display()
                                                                );
                                                            }
                                                            Err(err) => {
                                                                this.state.status_message = err;
                                                            }
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                        })
                                        .child(
                                            Button::new("vid-save-png", "📷 Save PNG Frame")
                                                .variant(ButtonVariant::Secondary)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    match this
                                                        .state
                                                        .video_player
                                                        .capture_current_frame_as_bitmap()
                                                    {
                                                        Ok(bmp) => {
                                                            this.state
                                                                .load_captured_bitmap(bmp, None);
                                                        }
                                                        Err(err) => {
                                                            this.state.status_message = err;
                                                        }
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("vid-export-gif", "🎞 Export GIF")
                                                .variant(ButtonVariant::Secondary)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let ts = SystemTime::now()
                                                        .duration_since(UNIX_EPOCH)
                                                        .map(|d| d.as_secs())
                                                        .unwrap_or(0);
                                                    let dir = capture_storage_dir();
                                                    let _ = std::fs::create_dir_all(&dir);
                                                    let out = dir.join(format!("GIF-{ts}.gif"));
                                                    match this
                                                        .state
                                                        .video_player
                                                        .export_animated_gif(&out)
                                                    {
                                                        Ok(()) => {
                                                            this.state.status_message = format!(
                                                                "Exported GIF {}",
                                                                out.display()
                                                            );
                                                        }
                                                        Err(err) => {
                                                            this.state.status_message = err;
                                                        }
                                                    }
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                        .child(render_video_timeline(&self.state.video_player, theme)),
                )
            })
            .when(self.state.show_recent_tray, |el| el.child(tray_strip))
            .child(status_bar)
            // Click-outside transparent backdrop whenever any dropdown menu is open
            .when(any_popover_open, |el| {
                el.child(
                    div()
                        .id("popover-dismiss-backdrop")
                        .occlude()
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.state.close_all_popovers();
                                cx.notify();
                            }),
                        ),
                )
            })
            // Topmost Floating Popovers (+ Create, Share, More Tools) anchored directly under their trigger buttons!
            .when(self.state.show_create_menu, |el| {
                let (create_left, create_top) =
                    compute_anchored_popover_pos(create_btn_rect, 224.0, viewport_w, false);
                el.child(
                    div()
                        .id("popover-create-layer")
                        .occlude()
                        .absolute()
                        .top(px(create_top))
                        .left(px(create_left))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Popover::new(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        Button::new("cr-new", "New Image")
                                            .variant(ButtonVariant::Ghost)
                                            .align_left()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.create_new_blank_canvas(1280, 720);
                                                this.state.show_create_menu = false;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("cr-tmpl", "Image from Template")
                                            .variant(ButtonVariant::Ghost)
                                            .align_left()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.workspace_view = WorkspaceView::Template;
                                                this.state.show_create_menu = false;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("cr-vid", "Video from Images")
                                            .variant(ButtonVariant::Ghost)
                                            .align_left()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.workspace_view =
                                                    WorkspaceView::VideoFromImages;
                                                this.state.show_create_menu = false;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("cr-comb", "Combine Images")
                                            .variant(ButtonVariant::Ghost)
                                            .align_left()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.workspace_view = WorkspaceView::Combine;
                                                this.state.show_create_menu = false;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("cr-xliff", "Translate Callouts (XLIFF)")
                                            .variant(ButtonVariant::Ghost)
                                            .align_left()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.state.workspace_view = WorkspaceView::Xliff;
                                                this.state.show_create_menu = false;
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .title("Create")
                            .width(px(224.0)),
                        ),
                )
            })
            .when(self.state.show_share_menu, |el| {
                let (share_left, share_top) =
                    compute_anchored_popover_pos(share_btn_rect, 248.0, viewport_w, true);
                let share_menu_col = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        Button::new("share-png", "Save PNG File")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) = this.state.export_flattened_as("png") {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("share-jpg", "Save JPG File")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) = this.state.export_flattened_as("jpg") {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("share-pdf", "Export PDF Document")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) = this.state.export_flattened_as("pdf") {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("share-svg", "Export Vector SVG")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) = this.state.export_flattened_as("svg") {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("share-clip", "Copy to Clipboard (PNG + DIB)")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) = this.state.copy_all_to_clipboard() {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .my(px(2.0))
                            .h(px(1.0))
                            .w_full()
                            .bg(theme.border),
                    )
                    .child(
                        Button::new("share-word", "Microsoft Word")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) =
                                    this.state.share_flattened_to(ShareTargetKind::OfficeWord)
                                {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("share-ppt", "Microsoft PowerPoint")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) = this
                                    .state
                                    .share_flattened_to(ShareTargetKind::OfficePowerPoint)
                                {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("share-excel", "Microsoft Excel")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) =
                                    this.state.share_flattened_to(ShareTargetKind::OfficeExcel)
                                {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("share-email", "Email (MAPI)")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) =
                                    this.state.share_flattened_to(ShareTargetKind::EmailMapi)
                                {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("share-print", "System Printer")
                            .variant(ButtonVariant::Ghost)
                            .align_left()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Err(err) = this.state.share_flattened_to(
                                    ShareTargetKind::SystemPrinter("Default".into()),
                                ) {
                                    this.state.status_message = err;
                                }
                                this.state.show_share_menu = false;
                                cx.notify();
                            })),
                    );
                el.child(
                    div()
                        .id("popover-share-layer")
                        .occlude()
                        .absolute()
                        .top(px(share_top))
                        .left(px(share_left))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Popover::new(share_menu_col)
                                .title("Share & Export")
                                .width(px(248.0)),
                        ),
                )
            })
            .when(self.state.show_more_tools, |el| {
                let (more_left, more_top) =
                    compute_anchored_popover_pos(more_btn_rect, 284.0, viewport_w, false);
                el.child(
                    div()
                        .id("popover-more-layer")
                        .occlude()
                        .absolute()
                        .top(px(more_top))
                        .left(px(more_left))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Popover::new(more_grid)
                                .title("More Tools")
                                .width(px(284.0)),
                        ),
                )
            })
            // Snagit-Style `Grab Text` Floating Modal Dialog (with live OCR Progress Bar & Results)
            .when(self.state.grab_edit_text.modal_open, |el| {
                let is_running = self.state.grab_edit_text.is_running;
                let progress = self.state.grab_edit_text.progress.clamp(0.0, 1.0);
                let pct = (progress * 100.0).round() as u32;
                let stage_lbl = if self.state.grab_edit_text.progress_stage.is_empty() {
                    "Recognizing text...".to_string()
                } else {
                    self.state.grab_edit_text.progress_stage.clone()
                };
                let raw_txt = self.state.grab_edit_text.grabbed_text.trim().to_string();
                let scope_lbl = if self.state.grab_edit_text.scope_label.is_empty() {
                    "Canvas".to_string()
                } else {
                    self.state.grab_edit_text.scope_label.clone()
                };
                let char_cnt = raw_txt.chars().count();
                let line_cnt = if raw_txt.is_empty() {
                    0
                } else {
                    raw_txt.lines().count()
                };
                let copied_now = self.state.grab_edit_text.copied_feedback;
                let modal_w = if is_running {
                    (viewport_w - 48.0).clamp(340.0, 460.0)
                } else {
                    (viewport_w - 48.0).clamp(360.0, 640.0)
                };
                let modal_h = if is_running {
                    216.0
                } else {
                    (viewport_h - 80.0).clamp(280.0, 440.0)
                };

                let body_el = if is_running {
                    let track_w = (modal_w - 48.0).max(160.0);
                    let fill_w = (track_w * progress).clamp(6.0, track_w);
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .justify_center()
                        .gap(px(12.0))
                        .px(px(24.0))
                        .py(px(16.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .text_color(theme.text_primary)
                                        .truncate()
                                        .child(stage_lbl),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(theme.accent_primary)
                                        .child(format!("{pct}%")),
                                ),
                        )
                        .child(
                            div()
                                .w(px(track_w))
                                .h(px(10.0))
                                .rounded(px(5.0))
                                .bg(theme.border)
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(px(fill_w))
                                        .rounded(px(5.0))
                                        .bg(theme.accent_primary),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child("Running offline neural OCR locally on device..."),
                        )
                        .into_any_element()
                } else {
                    let mut text_lines_col = div()
                        .id("grab-text-scroll-body")
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .w_full()
                        .h_full()
                        .px(px(14.0))
                        .py(px(12.0))
                        .overflow_y_scroll();

                    if raw_txt.is_empty() {
                        text_lines_col = text_lines_col.child(
                            div()
                                .w_full()
                                .text_sm()
                                .text_color(theme.text_muted)
                                .child("No readable text was detected in this region."),
                        );
                    } else {
                        for line_str in raw_txt.lines() {
                            let display_line = line_str.replace('\t', "    ");
                            text_lines_col = text_lines_col.child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(theme.text_primary)
                                    .child(if display_line.is_empty() {
                                        " ".to_string()
                                    } else {
                                        display_line
                                    }),
                            );
                        }
                    }

                    let scroll_shell = div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .rounded(theme.radius_sm)
                        .bg(theme.input_bg)
                        .border_1()
                        .border_color(theme.border_strong)
                        .overflow_hidden()
                        .child(text_lines_col);

                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .p(px(14.0))
                        .overflow_hidden()
                        .child(scroll_shell)
                        .into_any_element()
                };

                let header_badge = if is_running {
                    format!("{scope_lbl} · Scanning...")
                } else {
                    format!("{scope_lbl} · {line_cnt} lines · {char_cnt} chars")
                };

                let txt_for_copy = raw_txt.clone();
                let footer_actions = if is_running {
                    div()
                        .flex()
                        .flex_row()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            Button::new("grab-modal-cancel-btn", "Cancel")
                                .variant(ButtonVariant::Secondary)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.grab_edit_text.cancel_running_job();
                                    this.state.grab_edit_text.modal_open = false;
                                    this.state.status_message = "OCR cancelled".to_string();
                                    cx.notify();
                                })),
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_row()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            Button::new(
                                "grab-modal-copy-btn",
                                if copied_now { "Copied" } else { "Copy All" },
                            )
                            .icon(if copied_now {
                                SnagIcon::Check
                            } else {
                                SnagIcon::Copy
                            })
                            .disabled(raw_txt.is_empty())
                            .variant(if copied_now {
                                ButtonVariant::Success
                            } else {
                                ButtonVariant::Primary
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !txt_for_copy.is_empty() {
                                    let _ = snagsoup_win_platform::clipboard::write_text_to_clipboard(
                                        &txt_for_copy,
                                    );
                                    this.state.grab_edit_text.copied_feedback = true;
                                    cx.notify();
                                }
                            })),
                        )
                        .child(
                            Button::new("grab-modal-done-btn", "Close")
                                .variant(ButtonVariant::Secondary)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.grab_edit_text.modal_open = false;
                                    cx.notify();
                                })),
                        )
                        .into_any_element()
                };

                el.child(
                    div()
                        .id("grab-text-modal-backdrop")
                        .occlude()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(px(20.0))
                        .bg(rgba(0x00000066))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if !is_running {
                                    this.state.grab_edit_text.modal_open = false;
                                    cx.notify();
                                }
                            }),
                        )
                        .child(
                            div()
                                .id("grab-text-modal-card")
                                .flex()
                                .flex_col()
                                .w(px(modal_w))
                                .h(px(modal_h))
                                .rounded(theme.radius_md)
                                .bg(theme.surface_elevated)
                                .border_1()
                                .border_color(theme.border_strong)
                                .shadow_lg()
                                .overflow_hidden()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                // Header
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .flex_shrink_0()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(8.0))
                                        .h(px(44.0))
                                        .px(px(16.0))
                                        .bg(theme.toolbar_bg)
                                        .border_b_1()
                                        .border_color(theme.border)
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .flex_1()
                                                .min_w_0()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .text_sm()
                                                        .font_weight(gpui::FontWeight::BOLD)
                                                        .text_color(theme.text_primary)
                                                        .child("Grab Text"),
                                                )
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .px(px(7.0))
                                                        .py(px(2.0))
                                                        .rounded(theme.radius_sm)
                                                        .bg(theme.panel_bg)
                                                        .border_1()
                                                        .border_color(theme.border)
                                                        .text_xs()
                                                        .text_color(theme.text_secondary)
                                                        .truncate()
                                                        .child(header_badge),
                                                ),
                                        )
                                        .child(
                                            IconButton::new("grab-modal-close-x", SnagIcon::Close)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.state.grab_edit_text.cancel_running_job();
                                                    this.state.grab_edit_text.modal_open = false;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                // Progress bar or scrollable wrapped text body
                                .child(body_el)
                                // Balanced non-overflowing bottom footer
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .flex_shrink_0()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(12.0))
                                        .h(px(50.0))
                                        .px(px(16.0))
                                        .bg(theme.panel_bg)
                                        .border_t_1()
                                        .border_color(theme.border)
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_xs()
                                                .text_color(theme.text_muted)
                                                .truncate()
                                                .child(if is_running {
                                                    "Press Esc or Cancel to stop OCR"
                                                } else {
                                                    "Tip: Select an area (M) first to grab a region"
                                                }),
                                        )
                                        .child(footer_actions),
                                ),
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_shell_has_21_tools_and_snagx_roundtrip() {
        let mut st = EditorShellState::default();
        assert_eq!(st.tools.len(), 21);
        assert_eq!(
            st.active_tool,
            ToolMode::Selection,
            "Selection must be the default active tool when opening Editor"
        );
        assert_eq!(st.canvas_cursor_style(), CursorStyle::Crosshair);

        st.select_tool(ToolMode::Move);
        assert_eq!(st.canvas_cursor_style(), CursorStyle::OpenHand);
        st.select_tool(ToolMode::Text);
        assert_eq!(st.canvas_cursor_style(), CursorStyle::IBeam);
        st.select_tool(ToolMode::Stamp);
        assert_eq!(st.canvas_cursor_style(), CursorStyle::PointingHand);
        st.select_tool(ToolMode::Arrow);
        assert_eq!(st.canvas_cursor_style(), CursorStyle::Crosshair);
        st.select_tool(ToolMode::Selection);

        assert!(st.reorder_tool(0, 2));
        assert_eq!(st.tools[2].mode, ToolMode::Move);

        st.set_zoom(1200.0);
        assert_eq!(st.zoom_percent, 800.0);
        st.set_zoom(2.0);
        assert_eq!(st.zoom_percent, 10.0);

        // Save and re-open .snagx in temp dir
        let tmp = std::env::temp_dir().join("snagsoup_w3_05_test.snagx");
        st.save_snagx(&tmp).expect("save_snagx should succeed");

        let mut st2 = EditorShellState::default();
        st2.open_snagx(&tmp).expect("open_snagx should succeed");
        assert_eq!(st2.session.bitmap.width(), 1280);
        assert_eq!(st2.session.bitmap.height(), 720);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn all_editor_buttons_and_share_exports_work() {
        let mut st = EditorShellState::default();

        // 1. + Create -> New Image button
        st.create_new_blank_canvas(640, 480);
        assert_eq!(
            (st.session.bitmap.width(), st.session.bitmap.height()),
            (640, 480)
        );
        assert!(st.active_path.is_none());

        // 2. Paint a known pixel & test Eyedropper button + Undo/Redo buttons
        st.session.bitmap.mutate_region(PixelRect::new(10, 10, 4, 4), |_, _, px| {
            *px = [0x12, 0xAB, 0xEF, 0xFF];
        });
        assert_eq!(st.pick_canvas_color(11, 11), Some(0x12ABEF));
        assert_eq!(st.selected_color, 0x12ABEF);

        let col = st.selected_color_rgba();
        let idx = st.gizmos_g1.create_object(
            &mut st.session,
            ToolMode::Arrow,
            PointF::new(20.0, 20.0),
            PointF::new(120.0, 90.0),
            col,
            col,
            4.0,
            100.0,
        );
        assert_eq!(idx, 0);
        assert!(st.session.undo());
        assert_eq!(st.session.page.capture_objects.len(), 0);
        assert!(st.session.redo());
        assert_eq!(st.session.page.capture_objects.len(), 1);

        // 3. Effects panel buttons
        let first_eff = st.effects_gallery.entries[0].config.clone();
        st.apply_effect(&first_eff, st.effects_gallery.entries[0].name);
        assert_eq!(st.applied_effects_count, 1);

        // 4. Share & Export menu buttons (PNG, JPG, PDF, SVG)
        for ext in ["png", "jpg", "pdf", "svg"] {
            let exported = st
                .export_flattened_as(ext)
                .unwrap_or_else(|e| panic!("export {ext} failed: {e}"));
            assert!(exported.exists(), "exported {ext} file should exist");
            let meta = std::fs::metadata(&exported).unwrap();
            assert!(meta.len() > 32, "exported {ext} file should be non-empty");
            let _ = std::fs::remove_file(&exported);
        }

        // 5. Quick Save button -> MUST appear immediately in LibraryDb & LibraryOrganizerView!
        let saved = st.quick_save().expect("quick_save should succeed");
        assert!(saved.exists());
        let saved_str = saved.display().to_string();
        let db = LibraryDb::open_file(&library_db_path()).expect("open library.db");
        let mut lib_view = LibraryOrganizerView::new(db);
        assert!(
            lib_view.items.iter().any(|i| i.file_path == saved_str),
            "saved .snagx must be visible immediately in LibraryOrganizerView"
        );
        let _ = lib_view.db.remove_by_path(&saved_str);
        let _ = std::fs::remove_file(&saved);
    }

    #[test]
    fn responsive_toolbar_collapses_cleanly_on_narrow_windows() {
        assert_eq!(compute_responsive_toolbar(1440.0, 10, 21), (10, false));
        assert_eq!(compute_responsive_toolbar(1120.0, 10, 21), (6, false));
        assert_eq!(compute_responsive_toolbar(900.0, 10, 21), (3, true));
        assert_eq!(compute_responsive_toolbar(720.0, 10, 21), (2, true));
        assert_eq!(compute_responsive_toolbar(560.0, 10, 21), (0, true));
    }

    #[test]
    fn text_callout_typing_shape_stamp_libraries_and_staged_crop() {
        let mut st = EditorShellState::default();

        // 1. Text & Callout inline typing + Vietnamese UTF-8 backspace/sync
        let callout = CaptureObject {
            tool_mode: Some(ToolMode::Callout),
            start_point: Some(PointF::new(40.0, 40.0)),
            end_point: Some(PointF::new(260.0, 120.0)),
            tail_points: Some(vec![PointF::new(90.0, 155.0)]),
            foreground_color: Some(ColorRgba::rgb(24, 24, 27)),
            background_color: Some(ColorRgba::rgb(250, 204, 21)),
            font_size: Some(20.0),
            ..CaptureObject::default()
        };
        let idx = st.session.add_object(callout);
        st.start_editing_text_object(idx);
        assert_eq!(st.editing_text_obj, Some(idx));
        st.inline_text.insert_text("Xin chào SnagSoupX");
        assert!(st.inline_text.backspace());
        st.sync_active_text_to_session();
        st.finish_text_editing();
        assert!(st.editing_text_obj.is_none());

        // Reloading the object recovers exact typed Vietnamese string
        st.start_editing_text_object(idx);
        assert_eq!(st.inline_text.content, "Xin chào SnagSoup");
        st.finish_text_editing();

        // 2. Shape Library & Stamp Library render cleanly through SceneCompositor
        for &(shape_kind, _) in EditorShellState::SHAPE_LIBRARY {
            let obj = CaptureObject {
                tool_mode: Some(ToolMode::Shape),
                tool_shape: Some(shape_kind.to_string()),
                start_point: Some(PointF::new(10.0, 10.0)),
                end_point: Some(PointF::new(90.0, 80.0)),
                foreground_color: Some(ColorRgba::rgb(217, 43, 43)),
                background_color: Some(ColorRgba::TRANSPARENT),
                stroke_width: Some(4.0),
                ..CaptureObject::default()
            };
            st.session.add_object(obj);
        }
        for stamp in &st.stamps.stamps {
            let mut extra = std::collections::BTreeMap::new();
            extra.insert(
                "StampId".to_string(),
                serde_json::Value::String(stamp.id.clone()),
            );
            let obj = CaptureObject {
                tool_mode: Some(ToolMode::Stamp),
                tool_shape: Some(stamp.id.clone()),
                start_point: Some(PointF::new(100.0, 100.0)),
                end_point: Some(PointF::new(156.0, 156.0)),
                extra,
                ..CaptureObject::default()
            };
            st.session.add_object(obj);
        }
        let comp = SceneCompositor::new(&st.session.page, Some(&st.session.bitmap));
        let (fw, fh, rgba) = comp.flatten_to_rgba().expect("flatten with shapes & stamps");
        assert_eq!((fw, fh), (1280, 720));
        assert_eq!(rgba.len(), 1280 * 720 * 4);

        // 3. Staged Crop requires explicit confirmation (`confirm_pending_crop`)
        st.select_tool(ToolMode::Crop);
        assert!(st.pending_crop_rect.is_some(), "entering Crop initializes preview box");
        st.pending_crop_rect = Some(RectF::new(100.0, 80.0, 500.0, 320.0));
        // Canvas must STILL be 1280x720 before confirming!
        assert_eq!(
            (st.session.bitmap.width(), st.session.bitmap.height()),
            (1280, 720)
        );
        assert!(st.confirm_pending_crop());
        assert!(st.pending_crop_rect.is_none());
        assert_eq!(
            (st.session.bitmap.width(), st.session.bitmap.height()),
            (500, 320)
        );

        // 4. Selection marquee + delete_selected_region
        st.active_selection_rect = Some(RectF::new(10.0, 10.0, 80.0, 60.0));
        assert!(st.delete_selected_region());
        assert!(st.active_selection_rect.is_none());
    }

    #[test]
    fn crisp_hidpi_canvas_button_anchored_menus_and_copy_feedback() {
        let mut st = EditorShellState::default();

        // 1. HiDPI 125% (`device_scale = 1.25`) with a `1214x652` capture inside a `1020x620` logical canvas well:
        //    `avail_phys` = `(984 * 1.25, 584 * 1.25)` = `(1230, 730)` >= `(1214, 652)`
        //    -> `fit_canvas` MUST keep exact `100%` (`1.0`) 1:1 physical-pixel crispness instead of shrinking to `75%`!
        st.viewport.device_scale = 1.25;
        st.viewport.viewport_w = 1020.0;
        st.viewport.viewport_h = 620.0;
        let mut bmp = TiledBitmap::new_filled(1214, 652, ColorRgba::WHITE);
        bmp.mutate_region(PixelRect::new(10, 10, 1, 20), |_, _, px| {
            *px = [17, 24, 39, 255]; // 1px vertical crisp font stem
        });
        st.session = DocumentSession::new(
            SnagxPage {
                capture_canvas_width: Some(1214.0),
                capture_canvas_height: Some(652.0),
                dpi: Some(96.0),
                capture_background_color: Some(ColorRgba::WHITE),
                ..SnagxPage::default()
            },
            bmp,
        );
        st.viewport.fit_canvas(1214, 652);
        assert!(
            (st.viewport.zoom - 1.0).abs() < 1e-6,
            "1214x652 capture on 125% display must open at exact 100% 1:1 physical pixels"
        );
        // Verify `pan_x * 1.25` and `pan_y * 1.25` land on exact integer physical pixels
        assert!(((st.viewport.pan_x * 1.25) - (st.viewport.pan_x * 1.25).round()).abs() < 1e-4);
        assert!(((st.viewport.pan_y * 1.25) - (st.viewport.pan_y * 1.25).round()).abs() < 1e-4);

        // Verify `CanvasGpuCache` rebuilds when zoom changes (`75%` -> `100%`) even if `vis` rect is identical
        st.set_zoom(75.0);
        let vp75 = st.viewport.clone();
        st.gpu_cache.sync_from_session(&mut st.session, &vp75);
        assert_eq!(st.gpu_cache.cached_image_px, (911, 489));
        let rebuilds_75 = st.gpu_cache.dirty_rebuilds;

        st.set_zoom(100.0);
        let vp100 = st.viewport.clone();
        st.gpu_cache.sync_from_session(&mut st.session, &vp100);
        assert_eq!(st.gpu_cache.cached_image_px, (1214, 652));
        assert_eq!(st.gpu_cache.dirty_rebuilds, rebuilds_75 + 1);

        // 2. Button-anchored dropdown menu positions (`+ Create`, `More`, `Share`)
        let (cr_x, cr_y) = compute_anchored_popover_pos((386.0, 36.0, 82.0), 224.0, 1280.0, false);
        assert_eq!((cr_x, cr_y), (386.0, 40.0));
        let (mr_x, mr_y) = compute_anchored_popover_pos((668.0, 36.0, 68.0), 284.0, 1280.0, false);
        assert_eq!((mr_x, mr_y), (668.0, 40.0));
        let (sh_x, sh_y) = compute_anchored_popover_pos((1192.0, 36.0, 76.0), 248.0, 1280.0, true);
        assert_eq!((sh_x, sh_y), (1192.0 + 76.0 - 248.0, 40.0));

        // 3. `Copy All` -> `"Copied"` temporary state + monotonic sequence token
        assert!(!st.copy_feedback_active);
        let seq1 = st.activate_copy_feedback();
        assert!(st.copy_feedback_active);
        let seq2 = st.activate_copy_feedback();
        assert!(seq2 > seq1);
        // Stale timer from click 1 must NOT revert state early!
        assert!(!st.clear_copy_feedback_if_seq(seq1));
        assert!(st.copy_feedback_active);
        // Timer from latest click 2 cleanly reverts back to `Copy All`
        assert!(st.clear_copy_feedback_if_seq(seq2));
        assert!(!st.copy_feedback_active);
    }

    #[test]
    fn recent_captures_tray_populates_thumbnails_and_never_duplicates_on_click() {
        let tmp_dir = std::env::temp_dir().join("snagsoup_recent_tray_dedup_test");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");

        let file_a = tmp_dir.join("Capture_A.snagx");
        let file_b = tmp_dir.join("Capture_B.snagx");

        let mut writer = EditorShellState::default();
        writer.create_new_blank_canvas(770, 561);
        writer
            .session
            .bitmap
            .mutate_region(PixelRect::new(16, 16, 64, 64), |_, _, px| {
                *px = [5, 150, 105, 255];
            });
        writer.save_snagx(&file_a).expect("save A");

        writer.create_new_blank_canvas(640, 360);
        writer.save_snagx(&file_b).expect("save B");

        // Fresh Editor state loading from directory
        let mut st = EditorShellState::default();
        st.load_recent_captures_from_dir(&tmp_dir, 16);
        assert_eq!(
            st.recent_captures.len(),
            2,
            "directory scan loads exact 2 files and removes Untitled placeholder"
        );
        assert!(
            st.recent_captures.iter().all(|c| c.thumbnail.is_some()),
            "every loaded .snagx item must have a decoded RenderImage thumbnail"
        );

        // Clicking / re-opening the same file repeatedly (including mixed case/slashes) MUST NOT duplicate!
        for _ in 0..4 {
            st.open_snagx(&file_a).expect("open A");
        }
        let alt_case = PathBuf::from(
            file_a
                .display()
                .to_string()
                .replace('\\', "/")
                .to_uppercase(),
        );
        st.open_snagx(&alt_case).expect("open A uppercase");

        assert_eq!(
            st.recent_captures.len(),
            2,
            "clicking recent tray items must NEVER insert duplicate entries"
        );
        assert!(st.recent_captures[0].thumbnail.is_some());
        assert!(st.recent_captures[1].thumbnail.is_some());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn single_history_entry_per_tool_gesture_and_step_counter_sync() {
        let mut st = EditorShellState::default();
        st.create_new_blank_canvas(600, 400);
        st.viewport.zoom = 1.0;
        st.viewport.pan_x = 0.0;
        st.viewport.pan_y = 0.0;
        st.viewport.device_scale = 1.0;
        assert_eq!(st.session.history.undo_len(), 0);

        // 1. Step tool: mouse_down + 20 mouse_move frames + mouse_up MUST record EXACTLY 1 history step!
        st.select_tool(ToolMode::Step);
        st.pointer_down(100.0, 100.0, false, 1);
        for i in 1..=20 {
            st.pointer_move(100.0 + (i as f32) * 0.4, 100.0 + (i as f32) * 0.3); // slight click jitter (< 12px)
        }
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 1, "1 Step placement must be 1 history step");
        assert_eq!(st.session.page.capture_objects.len(), 1);
        assert_eq!(st.session.page.capture_objects[0].step_value.as_deref(), Some("1"));
        assert_eq!(st.gizmos_g1.step_counter.next_index, 2);

        // Place Step "2" with an intentional drag resize (25 mouse_move frames)
        st.pointer_down(220.0, 120.0, false, 1);
        for i in 1..=25 {
            st.pointer_move(220.0 + i as f32, 120.0);
        }
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 2, "2nd Step drag must add only 1 history step");
        assert_eq!(st.session.page.capture_objects[1].step_value.as_deref(), Some("2"));
        assert_eq!(st.gizmos_g1.step_counter.next_index, 3);

        // Undo Step "2" -> single Ctrl+Z removes Step "2" AND rewinds Step counter back to 2!
        assert!(st.perform_undo());
        assert_eq!(st.session.history.undo_len(), 1);
        assert_eq!(st.session.page.capture_objects.len(), 1);
        assert_eq!(st.gizmos_g1.step_counter.next_index, 2);

        // Redo Step "2" -> restores Step "2" AND advances Step counter back to 3!
        assert!(st.perform_redo());
        assert_eq!(st.session.history.undo_len(), 2);
        assert_eq!(st.session.page.capture_objects.len(), 2);
        assert_eq!(st.gizmos_g1.step_counter.next_index, 3);

        // 2. Shape, Arrow, and Pen freehand strokes: 20+ mouse_move frames each -> 1 history step each!
        st.select_tool(ToolMode::Shape);
        st.selected_shape_kind = "RoundedRectangle".to_string();
        st.shape_dashed = true;
        st.pointer_down(40.0, 200.0, false, 1);
        for i in 1..=20 {
            st.pointer_move(40.0 + (i as f32) * 4.0, 200.0 + (i as f32) * 3.0);
        }
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 3, "Shape drag must be 1 history step");

        st.select_tool(ToolMode::Arrow);
        st.pointer_down(300.0, 200.0, false, 1);
        for i in 1..=20 {
            st.pointer_move(300.0 + (i as f32) * 5.0, 200.0 + (i as f32) * 2.0);
        }
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 4, "Arrow drag must be 1 history step");

        st.select_tool(ToolMode::Pen);
        st.pointer_down(50.0, 300.0, false, 1);
        for i in 1..=30 {
            st.pointer_move(50.0 + (i as f32) * 3.0, 300.0 + ((i % 5) as f32));
        }
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 5, "30-point Pen stroke must be 1 history step");

        // 3. Move tool: click without moving -> 0 history steps; 15-frame drag -> 1 history step!
        st.select_tool(ToolMode::Move);
        st.pointer_down(100.0, 100.0, false, 1); // select Step "1"
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 5, "Click-to-select without drag must add 0 steps");

        st.pointer_down(100.0, 100.0, false, 1);
        for i in 1..=15 {
            st.pointer_move(100.0 + (i as f32) * 2.0, 100.0 + (i as f32) * 2.0);
        }
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 6, "Dragging object across 15 frames must add 1 step");

        // 4. Text tool: place new Text + type 10 keystrokes + finish -> 1 single AddObject step!
        st.select_tool(ToolMode::Text);
        st.pointer_down(350.0, 40.0, false, 1);
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 7);
        for ch in "Hello Step".chars() {
            let mut buf = [0u8; 4];
            st.inline_text.insert_text(ch.encode_utf8(&mut buf));
            st.sync_active_text_to_session();
        }
        st.finish_text_editing();
        assert_eq!(st.session.history.undo_len(), 7, "Typing into new Text box must coalesce into 1 step");

        // Re-edit existing Text box + type 3 chars + finish -> 1 single UpdateObject step!
        let txt_idx = st.session.page.capture_objects.len() - 1;
        st.start_editing_text_object(txt_idx);
        for ch in " 1!".chars() {
            let mut buf = [0u8; 4];
            st.inline_text.insert_text(ch.encode_utf8(&mut buf));
            st.sync_active_text_to_session();
        }
        st.finish_text_editing();
        assert_eq!(st.session.history.undo_len(), 8, "Re-editing existing Text box must add 1 step");

        // 5. Eraser multi-point stroke -> 1 RasterSnapshot step
        st.select_tool(ToolMode::Eraser);
        st.pointer_down(10.0, 10.0, false, 1);
        for i in 1..=15 {
            st.pointer_move(10.0 + (i as f32) * 3.0, 10.0);
        }
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 9, "Eraser stroke must add 1 step");

        // 6. Auto-Simplify + Show Original + Clear -> 1 step for ON, 0 for Show Original, 1 for OFF
        st.session.bitmap.mutate_region(PixelRect::new(420, 260, 120, 44), |_, _, px| {
            *px = [37, 99, 235, 255];
        });
        let simp_n = st.simplify_ui.run_auto_simplify(&mut st.session);
        assert!(simp_n >= 1);
        assert_eq!(st.session.history.undo_len(), 10, "Auto-Simplify generating N shapes must add 1 step");
        st.simplify_ui.set_show_original(true, &mut st.session);
        st.simplify_ui.set_show_original(false, &mut st.session);
        assert_eq!(st.session.history.undo_len(), 10, "Show Original preview must add 0 history steps");
        st.simplify_ui.clear_auto_simplify(&mut st.session);
        assert_eq!(st.session.history.undo_len(), 11, "Clearing Auto-Simplify must add 1 step");

        // 7. Smart Move one-gesture lift + drag -> 1 atomic Batch step!
        st.select_tool(ToolMode::Move);
        let det_n = st.smart_move.scan(&st.session);
        assert!(det_n >= 1);
        st.pointer_down(450.0, 280.0, false, 1);
        for i in 1..=12 {
            st.pointer_move(450.0 + (i as f32) * 4.0, 280.0 + (i as f32) * 2.0);
        }
        st.pointer_up();
        assert_eq!(st.session.history.undo_len(), 12, "Smart Move lift + drag must be 1 atomic step");

        // Single Undo of Smart Move restores original blue button pixel AND removes lifted Image object!
        let objs_before_undo = st.session.page.capture_objects.len();
        assert!(st.perform_undo());
        assert_eq!(st.session.page.capture_objects.len(), objs_before_undo - 1);
        assert_eq!(st.session.bitmap.get_pixel(450, 280), Some([37, 99, 235, 255]));
    }

    #[test]
    fn custom_tool_cursor_resolves_per_tool_and_yields_to_transient_modes() {
        let mut st = EditorShellState::default();
        assert_eq!(st.active_custom_tool_cursor_mode(), Some(ToolMode::Selection));

        st.select_tool(ToolMode::Step);
        assert_eq!(st.active_custom_tool_cursor_mode(), Some(ToolMode::Step));

        st.select_tool(ToolMode::Arrow);
        assert_eq!(st.active_custom_tool_cursor_mode(), Some(ToolMode::Arrow));

        // Move uses native Win32 4-way IDC_SIZEALL cursor
        st.select_tool(ToolMode::Move);
        assert_eq!(st.active_custom_tool_cursor_mode(), None);

        // Text shows Crosshair + Text icon before clicking, then yields to IBeam while typing!
        st.select_tool(ToolMode::Text);
        assert_eq!(st.active_custom_tool_cursor_mode(), Some(ToolMode::Text));
        st.pointer_down(120.0, 100.0, false, 1);
        st.pointer_up();
        assert!(st.editing_text_obj.is_some());
        assert_eq!(
            st.active_custom_tool_cursor_mode(),
            None,
            "actively typing inside Text box must yield to IBeam"
        );
        st.finish_text_editing();
        assert_eq!(st.active_custom_tool_cursor_mode(), Some(ToolMode::Text));

        // Open popovers, modal dialogs, and Space-panning yield to standard cursors
        st.show_more_tools = true;
        assert_eq!(st.active_custom_tool_cursor_mode(), None);
        st.show_more_tools = false;

        st.viewport.space_panning = true;
        assert_eq!(st.active_custom_tool_cursor_mode(), None);
        st.viewport.space_panning = false;
        assert_eq!(st.active_custom_tool_cursor_mode(), Some(ToolMode::Text));
    }
}
