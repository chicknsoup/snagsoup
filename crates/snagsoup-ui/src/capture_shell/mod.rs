//! Capture Shell module (`W3-02`, `W3-03`, `W3-04`, `W3-13`).
//!
//! Houses the 3-tab Capture Window, OneClick top-edge floating widget,
//! interactive Screen Selection Overlay (Crosshair, 11x11 Magnifier Loupe, Guidelines),
//! and future Wave 4 HUDs.

pub mod capture_window;
pub mod oneclick;
pub mod overlay;
pub mod recording_flow;

pub use capture_window::{
    AboutDialogView, CapturePreferencesView, CaptureWindowState, CaptureWindowView, PreferencesTab,
    selection_mode_label,
};
pub use oneclick::OneClickWidget;
pub use overlay::{
    GuidelinePreset, MagnifierSample, OverlayPhase, ScrollArrowsHint, SelectionOverlayState,
    SelectionOverlayView, VideoRecordRequest,
};
pub use recording_flow::{LiveRecordingHudView, open_live_recording_hud};
