//! Editor Shell module (`W3-05`..`W3-12` Editor Window, Interactive Canvas,
//! Vector Gizmos G1/G2, Raster UI Tools, QuickStyles/Themes, 15 Effects Gallery,
//! Dialogs, Smart CV/OCR UI, Video Player & GIF Dialog).

pub mod canvas_element;
pub mod dialogs;
pub mod effects_panel;
pub mod gif_dialog;
pub mod gizmos_g1;
pub mod gizmos_g2;
pub mod quickstyles;
pub mod raster_ui;
pub mod shell;
pub mod smart_cv_ui;
pub mod video_player;

pub use canvas_element::{
    AlignmentGuide, CanvasGpuCache, CanvasResizeHandle, CanvasViewportState, render_canvas_surface,
};
pub use dialogs::{
    CaptureDetailsDialogState, ResizeCanvasDialogState, ResizeImageDialogState,
    RotateCustomDialogState, SizeUnit,
};
pub use effects_panel::{
    EffectGalleryEntry, EffectPreviewThumbnail, EffectsGalleryState, default_15_effects,
};
pub use gif_dialog::{CreateGifDialogState, GifPresetChoice};
pub use gizmos_g1::{GizmoControllerG1, StepCounter, StepSequence, VectorHandle, object_bounds};
pub use gizmos_g2::{CalloutTailController, InlineTextEditor, StampEntry, StampSearcher};
pub use quickstyles::QuickStyleManager;
pub use raster_ui::{
    CropFloatyState, CutOutPreviewState, RasterSelectionKind, RasterToolController,
};
pub use shell::{
    EditorShellState, EditorShellView, RecentCaptureItem, SidePanelTab, ToolToolbarEntry,
    WorkspaceView, default_all_21_tools,
};
pub use smart_cv_ui::{
    GrabEditTextUiState, MagicWandUiState, SimplifyUiFloatyState, SmartMoveUiState,
};
pub use video_player::{VideoPlayerState, format_timestamp_ms, render_video_timeline};
