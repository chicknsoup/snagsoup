use crate::geometry::ColorRgba;
use serde::{Deserialize, Serialize};

/// All 8 Snagit selection modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionMode {
    AllInOne,
    Region,
    Window,
    FullScreen,
    ScrollingWindow,
    Panoramic,
    FixedRegion,
    Freehand,
}

impl Default for SelectionMode {
    fn default() -> Self {
        SelectionMode::AllInOne
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureKind {
    AllInOne,
    Image,
    Video,
}

impl Default for CaptureKind {
    fn default() -> Self {
        CaptureKind::AllInOne
    }
}

/// Delayed / timed capture configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TimedCaptureConfig {
    pub enabled: bool,
    pub delay_seconds: u32,
    pub show_countdown: bool,
    pub repeat_interval_seconds: Option<u32>,
}

/// Video capture settings (MP4 + WASAPI + Webcam PiP).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoCaptureConfig {
    pub fps: u32,
    pub capture_cursor: bool,
    pub record_microphone: bool,
    pub record_system_audio: bool,
    pub record_webcam: bool,
    pub webcam_pip_corner: String,
    pub webcam_pip_scale: f32,
}

impl Default for VideoCaptureConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            capture_cursor: true,
            record_microphone: true,
            record_system_audio: true,
            record_webcam: false,
            webcam_pip_corner: "BottomRight".to_string(),
            webcam_pip_scale: 0.25,
        }
    }
}

/// Capture profile / preset matching `Presets4.xml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturePreset {
    pub id: String,
    pub name: String,
    pub kind: CaptureKind,
    pub selection_mode: SelectionMode,
    pub hotkey: Option<String>,
    pub capture_cursor: bool,
    pub copy_to_clipboard: bool,
    pub preview_in_editor: bool,
    pub timed_capture: TimedCaptureConfig,
    pub video_config: Option<VideoCaptureConfig>,
    pub auto_effects: Vec<EffectConfig>,
    pub share_destination: Option<String>,
}

impl Default for CapturePreset {
    fn default() -> Self {
        Self {
            id: "default-all-in-one".to_string(),
            name: "All-in-One".to_string(),
            kind: CaptureKind::AllInOne,
            selection_mode: SelectionMode::AllInOne,
            hotkey: Some("PrintScreen".to_string()),
            capture_cursor: false,
            copy_to_clipboard: false,
            preview_in_editor: true,
            timed_capture: TimedCaptureConfig::default(),
            video_config: None,
            auto_effects: Vec::new(),
            share_destination: None,
        }
    }
}

/// Animated GIF export preset matching `GifExportOptions.xml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GifExportPreset {
    pub name: String,
    pub fps: u32,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub skip_similar_frames: bool,
    pub similarity_threshold: f32,
    pub dithering_factor: f32,
    pub insert_transition_frames: bool,
    pub loop_forever: bool,
}

impl GifExportPreset {
    pub fn screen_video() -> Self {
        Self {
            name: "ScreenVideo".to_string(),
            fps: 10,
            max_width: Some(1280),
            max_height: None,
            skip_similar_frames: true,
            similarity_threshold: 0.0005,
            dithering_factor: 0.5,
            insert_transition_frames: true,
            loop_forever: true,
        }
    }

    pub fn high_motion_video() -> Self {
        Self {
            name: "HighMotionVideo".to_string(),
            fps: 15,
            max_width: Some(960),
            max_height: None,
            skip_similar_frames: false,
            similarity_threshold: 0.0,
            dithering_factor: 0.75,
            insert_transition_frames: false,
            loop_forever: true,
        }
    }

    pub fn reduced_file_size() -> Self {
        Self {
            name: "ReducedFileSize".to_string(),
            fps: 6,
            max_width: Some(640),
            max_height: None,
            skip_similar_frames: true,
            similarity_threshold: 0.002,
            dithering_factor: 0.25,
            insert_transition_frames: false,
            loop_forever: true,
        }
    }
}

/// All 15 Snagit global image effects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EffectConfig {
    Border {
        width: u32,
        color: ColorRgba,
        inside: bool,
    },
    Shadow {
        offset_x: i32,
        offset_y: i32,
        blur_radius: u32,
        opacity: f32,
        color: ColorRgba,
    },
    Edges {
        style: String,
        size: u32,
        edges_mask: u8,
        seed: u64,
    },
    PageCurl {
        corner: String,
        radius: f32,
        background_color: ColorRgba,
    },
    Perspective {
        angle_x: f32,
        angle_y: f32,
        reflection: bool,
        reflection_opacity: f32,
    },
    Watermark {
        image_path: String,
        anchor: String,
        scale: f32,
        opacity: f32,
        emboss: bool,
    },
    Caption {
        text: String,
        position: String,
        font_family: String,
        font_size: f32,
        text_color: ColorRgba,
        background_color: ColorRgba,
    },
    Filters {
        kind: String,
        intensity: f32,
    },
    ColorAdjustment {
        brightness: f32,
        contrast: f32,
        hue: f32,
        saturation: f32,
        gamma: f32,
    },
    ColorReplacement {
        source: ColorRgba,
        target: ColorRgba,
        tolerance: f32,
        preserve_luminance: bool,
    },
    SpotlightAndMagnify {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        dim_opacity: f32,
        blur_radius: u32,
        magnify_scale: f32,
    },
    Trim {
        tolerance: u8,
        auto_trim: bool,
    },
    Rotate {
        angle_degrees: f32,
        background_color: ColorRgba,
    },
    Flip {
        horizontal: bool,
        vertical: bool,
    },
    Scale {
        width: u32,
        height: u32,
        keep_aspect_ratio: bool,
        dpi: u32,
    },
}
