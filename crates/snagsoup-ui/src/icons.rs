//! Embedded vector SVG icon system for SnagSoup.
//!
//! All UI icons are embedded as lightweight static SVG strings served directly
//! via [`SnagAssets`](crate::SnagAssets), keeping startup zero-I/O and single-binary.

use gpui::{Hsla, Pixels, Styled, Svg, svg};

/// All standard SnagSoup vector UI icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnagIcon {
    Capture,
    Editor,
    Library,
    Arrow,
    Callout,
    Text,
    Shape,
    Step,
    Stamp,
    Pen,
    Highlighter,
    Blur,
    Magnify,
    Crop,
    Eraser,
    Fill,
    Selection,
    Move,
    Undo,
    Redo,
    Copy,
    Share,
    Effects,
    ZoomIn,
    ZoomOut,
    Sun,
    Moon,
    Plus,
    Minus,
    Close,
    ChevronDown,
    Pipette,
    Video,
    Settings,
    CameraCapture,
    VideoCapture,
    PanoramicCapture,
    RedoCycle,
    CloseBold,
    MicOn,
    MicOff,
    SpeakerOn,
    SpeakerOff,
    RecordDot,
    Check,
    ChevronRight,
}

impl SnagIcon {
    /// Virtual asset path recognized by [`SnagAssets`](crate::SnagAssets).
    pub const fn asset_path(self) -> &'static str {
        match self {
            Self::Capture => "icons/capture.svg",
            Self::Editor => "icons/editor.svg",
            Self::Library => "icons/library.svg",
            Self::Arrow => "icons/arrow.svg",
            Self::Callout => "icons/callout.svg",
            Self::Text => "icons/text.svg",
            Self::Shape => "icons/shape.svg",
            Self::Step => "icons/step.svg",
            Self::Stamp => "icons/stamp.svg",
            Self::Pen => "icons/pen.svg",
            Self::Highlighter => "icons/highlighter.svg",
            Self::Blur => "icons/blur.svg",
            Self::Magnify => "icons/magnify.svg",
            Self::Crop => "icons/crop.svg",
            Self::Eraser => "icons/eraser.svg",
            Self::Fill => "icons/fill.svg",
            Self::Selection => "icons/selection.svg",
            Self::Move => "icons/move.svg",
            Self::Undo => "icons/undo.svg",
            Self::Redo => "icons/redo.svg",
            Self::Copy => "icons/copy.svg",
            Self::Share => "icons/share.svg",
            Self::Effects => "icons/effects.svg",
            Self::ZoomIn => "icons/zoom_in.svg",
            Self::ZoomOut => "icons/zoom_out.svg",
            Self::Sun => "icons/sun.svg",
            Self::Moon => "icons/moon.svg",
            Self::Plus => "icons/plus.svg",
            Self::Minus => "icons/minus.svg",
            Self::Close => "icons/close.svg",
            Self::ChevronDown => "icons/chevron_down.svg",
            Self::Pipette => "icons/pipette.svg",
            Self::Video => "icons/video.svg",
            Self::Settings => "icons/settings.svg",
            Self::CameraCapture => "icons/camera_capture.svg",
            Self::VideoCapture => "icons/video_capture.svg",
            Self::PanoramicCapture => "icons/panoramic_capture.svg",
            Self::RedoCycle => "icons/redo_cycle.svg",
            Self::CloseBold => "icons/close_bold.svg",
            Self::MicOn => "icons/mic_on.svg",
            Self::MicOff => "icons/mic_off.svg",
            Self::SpeakerOn => "icons/speaker_on.svg",
            Self::SpeakerOff => "icons/speaker_off.svg",
            Self::RecordDot => "icons/record_dot.svg",
            Self::Check => "icons/check.svg",
            Self::ChevronRight => "icons/chevron_right.svg",
        }
    }

    /// Embedded SVG markup for this icon (24x24 viewBox, `currentColor` stroke/fill).
    pub const fn svg_source(self) -> &'static str {
        match self {
            Self::Capture => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="7"/><circle cx="12" cy="12" r="3" fill="currentColor"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3"/></svg>"#,
            Self::Editor => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><path d="M8 16l3-3 2 2 4-5"/></svg>"#,
            Self::Library => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></svg>"#,
            Self::Arrow => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 19L19 5M10 5h9v9"/></svg>"#,
            Self::Callout => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"/></svg>"#,
            Self::Text => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7V4h16v3M9 20h6M12 4v16"/></svg>"#,
            Self::Shape => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="10" height="10" rx="1.5"/><circle cx="15.5" cy="15.5" r="5.5"/></svg>"#,
            Self::Step => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 8v8M10 10l2-2"/></svg>"#,
            Self::Stamp => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 20h14M6 16h12v2H6zM10 16V9a2 2 0 1 1 4 0v7"/></svg>"#,
            Self::Pen => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>"#,
            Self::Highlighter => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 11l-6 6v3h9l3-3M22 12l-4.6 4.6a2 2 0 0 1-2.8 0l-5.2-5.2a2 2 0 0 1 0-2.8L14 4"/></svg>"#,
            Self::Blur => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2.69l5.66 5.66a8 8 0 1 1-11.31 0z"/></svg>"#,
            Self::Magnify => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.35-4.35M11 8v6M8 11h6"/></svg>"#,
            Self::Crop => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 2v14a2 2 0 0 0 2 2h14M18 22V8a2 2 0 0 0-2-2H2"/></svg>"#,
            Self::Eraser => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20H7L3 16a2 2 0 0 1 0-2.83l9.9-9.9a2 2 0 0 1 2.83 0l5.44 5.44a2 2 0 0 1 0 2.83L14 19"/></svg>"#,
            Self::Fill => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 11l-8-8-8.6 8.6a2 2 0 0 0 0 2.8l5.2 5.2c.8.8 2 .8 2.8 0L19 11zM5 2l5 5M2 13h15"/><path d="M20 15s2 2.2 2 3.5a2 2 0 0 1-4 0c0-1.3 2-3.5 2-3.5z"/></svg>"#,
            Self::Selection => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" stroke-dasharray="3 3"><rect x="4" y="4" width="16" height="16" rx="1"/></svg>"#,
            Self::Move => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 9l-3 3 3 3M9 5l3-3 3 3M15 19l-3 3-3-3M19 9l3 3-3 3M2 12h20M12 2v20"/></svg>"#,
            Self::Undo => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 14L4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/></svg>"#,
            Self::Redo => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 14l5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/></svg>"#,
            Self::Copy => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>"#,
            Self::Share => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 12v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8M16 6l-4-4-4 4M12 2v13"/></svg>"#,
            Self::Effects => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9L12 3z"/></svg>"#,
            Self::ZoomIn => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="M21 21l-4.35-4.35M11 8v6M8 11h6"/></svg>"#,
            Self::ZoomOut => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="M21 21l-4.35-4.35M8 11h6"/></svg>"#,
            Self::Sun => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/></svg>"#,
            Self::Moon => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>"#,
            Self::Plus => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14M5 12h14"/></svg>"#,
            Self::Minus => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/></svg>"#,
            Self::Close => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6L6 18M6 6l12 12"/></svg>"#,
            Self::ChevronDown => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>"#,
            Self::Pipette => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 22l1-1h3l9-9M13 6l5 5M14.5 4.5l5 5a2.12 2.12 0 0 0 0-3l-2-2a2.12 2.12 0 0 0-3 0z"/></svg>"#,
            Self::Video => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="6" width="14" height="12" rx="2"/><path d="M22 8l-6 4 6 4V8z"/></svg>"#,
            Self::Settings => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"#,
            Self::CameraCapture => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 4h-5L7.5 6.5H4a2 2 0 0 0-2 2v9.5a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8.5a2 2 0 0 0-2-2h-3.5L14.5 4z"/><circle cx="12" cy="13" r="4"/></svg>"#,
            Self::VideoCapture => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><rect x="2" y="5.5" width="13.5" height="13" rx="2.5"/><path d="M17 10.2l5-3.2v10l-5-3.2v-3.6z"/></svg>"#,
            Self::PanoramicCapture => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="7.5" y="8.5" width="9" height="7" rx="1.5"/><circle cx="12" cy="12" r="1.8"/><path d="M12 2v4M10 4l2-2 2 2M12 22v-4M10 20l2 2 2-2M2 12h3.5M4 10l-2 2 2 2M22 12h-3.5M20 10l2 2-2 2"/></svg>"#,
            Self::RedoCycle => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/></svg>"#,
            Self::CloseBold => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6L6 18M6 6l12 12"/></svg>"#,
            Self::MicOn => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="2" width="6" height="11" rx="3"/><path d="M19 10v2a7 7 0 0 1-14 0v-2M12 19v3M8 22h8"/></svg>"#,
            Self::MicOff => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><path d="M2 2l20 20M9 9v3a3 3 0 0 0 5.12 2.12M15 9.34V5a3 3 0 0 0-5.68-1.33"/><path d="M17 16.95A7 7 0 0 1 5 12v-2m14 0v2c0 .76-.12 1.5-.35 2.18M12 19v3M8 22h8"/></svg>"#,
            Self::SpeakerOn => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14"/></svg>"#,
            Self::SpeakerOff => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><line x1="23" y1="9" x2="17" y2="15"/><line x1="17" y1="9" x2="23" y2="15"/></svg>"#,
            Self::RecordDot => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="5" fill="currentColor"/></svg>"#,
            Self::Check => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>"#,
            Self::ChevronRight => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>"#,
        }
    }

    /// All icons registered in [`SnagAssets`](crate::SnagAssets).
    pub const ALL: &'static [SnagIcon] = &[
        Self::Capture,
        Self::Editor,
        Self::Library,
        Self::Arrow,
        Self::Callout,
        Self::Text,
        Self::Shape,
        Self::Step,
        Self::Stamp,
        Self::Pen,
        Self::Highlighter,
        Self::Blur,
        Self::Magnify,
        Self::Crop,
        Self::Eraser,
        Self::Fill,
        Self::Selection,
        Self::Move,
        Self::Undo,
        Self::Redo,
        Self::Copy,
        Self::Share,
        Self::Effects,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::Sun,
        Self::Moon,
        Self::Plus,
        Self::Minus,
        Self::Close,
        Self::ChevronDown,
        Self::Pipette,
        Self::Video,
        Self::Settings,
        Self::CameraCapture,
        Self::VideoCapture,
        Self::PanoramicCapture,
        Self::RedoCycle,
        Self::CloseBold,
        Self::MicOn,
        Self::MicOff,
        Self::SpeakerOn,
        Self::SpeakerOff,
        Self::RecordDot,
        Self::Check,
        Self::ChevronRight,
    ];

    /// Lookup embedded SVG bytes by asset path.
    pub fn lookup_bytes(path: &str) -> Option<&'static [u8]> {
        Self::ALL
            .iter()
            .find(|icon| icon.asset_path() == path)
            .map(|icon| icon.svg_source().as_bytes())
    }

    /// Build a GPUI `Svg` element with the requested size and tint color.
    pub fn render(self, size: Pixels, color: Hsla) -> Svg {
        svg()
            .path(self.asset_path())
            .size(size)
            .text_color(color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_icons_have_valid_svg_and_unique_paths() {
        let mut seen = std::collections::HashSet::new();
        for &icon in SnagIcon::ALL {
            let path = icon.asset_path();
            assert!(seen.insert(path), "duplicate icon path: {path}");
            let bytes = SnagIcon::lookup_bytes(path).expect("icon lookup must succeed");
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.starts_with("<svg ") && text.ends_with("</svg>"));
        }
    }
}
