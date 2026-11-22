//! Snagit 2022 Capture App Window (`All-in-One`, `Image`, `Video`), Presets Drawer,
//! Solid Blue Integrated Header (`File`, `Edit`, `View`, `Help > About`),
//! Dedicated `AboutDialogView`, and Redesigned 3-Tab `CapturePreferencesView`
//! (`General`, `Capture`, `Hotkeys` with Win32 hardware key polling).

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowControlArea, div,
    prelude::FluentBuilder, px, rgb, rgba, size,
};
use snagsoup_model::{
    CaptureKind, CapturePreset, SelectionMode, TimedCaptureConfig, VideoCaptureConfig,
};
use snagsoup_win_platform::hotkey::{
    RecordedKeyAction, begin_hotkey_recording, end_hotkey_recording, poll_hotkey_recording_input,
};

use crate::icons::SnagIcon;
use crate::theme::{SnagTheme, ThemeMode};
use crate::widgets::{Button, ButtonVariant, IconButton, Popover};

/// Compact window dimensions when Presets drawer is closed/open.
pub const CAPTURE_WIN_WIDTH: f32 = 520.0;
pub const CAPTURE_WIN_COMPACT_HEIGHT: f32 = 268.0;
pub const CAPTURE_WIN_EXPANDED_HEIGHT: f32 = 416.0;

/// Standalone Preferences & About dialog dimensions.
pub const PREFS_DIALOG_WIDTH: f32 = 480.0;
pub const PREFS_DIALOG_HEIGHT: f32 = 436.0;
pub const ABOUT_DIALOG_WIDTH: f32 = 390.0;
pub const ABOUT_DIALOG_HEIGHT: f32 = 236.0;

/// All selectable capture modes exposed in the Capture Window.
pub const SELECTION_MODES: &[SelectionMode] = &[
    SelectionMode::AllInOne,
    SelectionMode::Region,
    SelectionMode::Window,
    SelectionMode::FullScreen,
    SelectionMode::ScrollingWindow,
    SelectionMode::Panoramic,
    SelectionMode::FixedRegion,
    SelectionMode::Freehand,
];

/// Share destinations available in Capture Window dropdown.
pub const SHARE_DESTINATIONS: &[&str] = &[
    "None",
    "File (Auto-save)",
    "Clipboard",
    "Word",
    "PowerPoint",
    "Excel",
    "Email",
];

/// Format human-readable label for a [`SelectionMode`].
pub const fn selection_mode_label(mode: SelectionMode) -> &'static str {
    match mode {
        SelectionMode::AllInOne => "All-in-One",
        SelectionMode::Region => "Region",
        SelectionMode::Window => "Window",
        SelectionMode::FullScreen => "Fullscreen",
        SelectionMode::ScrollingWindow => "Scrolling Window",
        SelectionMode::Panoramic => "Panoramic",
        SelectionMode::FixedRegion => "Fixed Region",
        SelectionMode::Freehand => "Freehand",
    }
}

/// Active dropdown menu on the solid blue top bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTopMenu {
    File,
    Edit,
    View,
    Help,
}

/// Active tab inside the Snagit-style `Capture Preferences` dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreferencesTab {
    #[default]
    General,
    Capture,
    Hotkeys,
}

/// Which hotkey slot is currently listening for a live keyboard shortcut press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySlot {
    GlobalCapture,
    RepeatLastCapture,
    VideoRecordPause,
    VideoStop,
    ShowHideApp,
}

/// Persistent Snagit-style Capture Preferences (`%LOCALAPPDATA%\SnagSoup\preferences.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePreferences {
    // General
    pub run_when_windows_starts: bool,
    pub show_oneclick_widget: bool,
    pub close_to_tray: bool,
    pub show_notification_tips: bool,
    pub dark_theme: bool,

    // Capture
    pub hide_snagsoup_before_capturing: bool,
    pub show_magnifier: bool,
    pub show_all_in_one_toolbar: bool,
    pub auto_copy_to_clipboard: bool,
    pub default_video_fps: u32,
    pub video_quality: String,

    // Hotkeys
    pub global_capture_hotkey: String,
    pub repeat_last_capture_hotkey: String,
    pub video_record_pause_hotkey: String,
    pub video_stop_hotkey: String,
    pub show_hide_hotkey: String,
}

impl Default for CapturePreferences {
    fn default() -> Self {
        Self {
            run_when_windows_starts: false,
            show_oneclick_widget: false,
            close_to_tray: false,
            show_notification_tips: true,
            dark_theme: false,
            hide_snagsoup_before_capturing: true,
            show_magnifier: true,
            show_all_in_one_toolbar: true,
            auto_copy_to_clipboard: false,
            default_video_fps: 60,
            video_quality: "High".to_string(),
            global_capture_hotkey: "Print Screen".to_string(),
            repeat_last_capture_hotkey: "Ctrl+Shift+R".to_string(),
            video_record_pause_hotkey: "Shift+F9".to_string(),
            video_stop_hotkey: "Shift+F10".to_string(),
            show_hide_hotkey: "Ctrl+Shift+X".to_string(),
        }
    }
}

pub fn preferences_file_path() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("SnagSoup")
        .join("preferences.json")
}

impl CapturePreferences {
    pub fn from_json_value(val: &serde_json::Value) -> Self {
        let mut out = Self::default();
        if let Some(b) = val.get("show_oneclick_widget").and_then(|v| v.as_bool()) {
            out.show_oneclick_widget = b;
        }
        if let Some(b) = val.get("close_to_tray").and_then(|v| v.as_bool()) {
            out.close_to_tray = b;
        }
        if let Some(b) = val.get("show_notification_tips").and_then(|v| v.as_bool()) {
            out.show_notification_tips = b;
        }
        if let Some(b) = val.get("dark_theme").and_then(|v| v.as_bool()) {
            out.dark_theme = b;
        }
        if let Some(b) = val
            .get("hide_snagsoup_before_capturing")
            .and_then(|v| v.as_bool())
        {
            out.hide_snagsoup_before_capturing = b;
        }
        if let Some(b) = val.get("show_magnifier").and_then(|v| v.as_bool()) {
            out.show_magnifier = b;
        }
        if let Some(b) = val.get("show_all_in_one_toolbar").and_then(|v| v.as_bool()) {
            out.show_all_in_one_toolbar = b;
        }
        if let Some(b) = val.get("auto_copy_to_clipboard").and_then(|v| v.as_bool()) {
            out.auto_copy_to_clipboard = b;
        }
        if let Some(n) = val.get("default_video_fps").and_then(|v| v.as_u64()) {
            out.default_video_fps = n as u32;
        }
        if let Some(s) = val.get("video_quality").and_then(|v| v.as_str()) {
            out.video_quality = s.to_string();
        }
        if let Some(s) = val.get("global_capture_hotkey").and_then(|v| v.as_str()) {
            out.global_capture_hotkey = s.to_string();
        }
        if let Some(s) = val
            .get("repeat_last_capture_hotkey")
            .and_then(|v| v.as_str())
        {
            out.repeat_last_capture_hotkey = s.to_string();
        }
        if let Some(s) = val
            .get("video_record_pause_hotkey")
            .and_then(|v| v.as_str())
        {
            out.video_record_pause_hotkey = s.to_string();
        }
        if let Some(s) = val.get("video_stop_hotkey").and_then(|v| v.as_str()) {
            out.video_stop_hotkey = s.to_string();
        }
        if let Some(s) = val.get("show_hide_hotkey").and_then(|v| v.as_str()) {
            out.show_hide_hotkey = s.to_string();
        }
        out
    }

    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "run_when_windows_starts": self.run_when_windows_starts,
            "show_oneclick_widget": self.show_oneclick_widget,
            "close_to_tray": self.close_to_tray,
            "show_notification_tips": self.show_notification_tips,
            "dark_theme": self.dark_theme,
            "hide_snagsoup_before_capturing": self.hide_snagsoup_before_capturing,
            "show_magnifier": self.show_magnifier,
            "show_all_in_one_toolbar": self.show_all_in_one_toolbar,
            "auto_copy_to_clipboard": self.auto_copy_to_clipboard,
            "default_video_fps": self.default_video_fps,
            "video_quality": self.video_quality,
            "global_capture_hotkey": self.global_capture_hotkey,
            "repeat_last_capture_hotkey": self.repeat_last_capture_hotkey,
            "video_record_pause_hotkey": self.video_record_pause_hotkey,
            "video_stop_hotkey": self.video_stop_hotkey,
            "show_hide_hotkey": self.show_hide_hotkey,
        })
    }

    /// Load saved preferences from `%LOCALAPPDATA%\SnagSoup\preferences.json`, syncing
    /// `run_when_windows_starts` with the live `HKCU\...\Run` Windows Registry entry.
    pub fn load_or_default() -> Self {
        let path = preferences_file_path();
        let mut prefs = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| Self::from_json_value(&v))
            .unwrap_or_default();
        prefs.run_when_windows_starts =
            snagsoup_win_platform::startup::is_run_at_windows_startup_enabled();
        prefs
    }

    /// Persist preferences to `%LOCALAPPDATA%\SnagSoup\preferences.json`.
    pub fn save_to_disk(&self) {
        let path = preferences_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.to_json_value()) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Reset all hotkeys back to Snagit 2022 factory defaults.
    pub fn restore_default_hotkeys(&mut self) {
        let def = Self::default();
        self.global_capture_hotkey = def.global_capture_hotkey;
        self.repeat_last_capture_hotkey = def.repeat_last_capture_hotkey;
        self.video_record_pause_hotkey = def.video_record_pause_hotkey;
        self.video_stop_hotkey = def.video_stop_hotkey;
        self.show_hide_hotkey = def.show_hide_hotkey;
    }

    pub fn set_slot(&mut self, slot: HotkeySlot, value: String) {
        match slot {
            HotkeySlot::GlobalCapture => self.global_capture_hotkey = value,
            HotkeySlot::RepeatLastCapture => self.repeat_last_capture_hotkey = value,
            HotkeySlot::VideoRecordPause => self.video_record_pause_hotkey = value,
            HotkeySlot::VideoStop => self.video_stop_hotkey = value,
            HotkeySlot::ShowHideApp => self.show_hide_hotkey = value,
        }
    }
}

/// Format a GPUI [`KeyDownEvent`] into a human-readable Win32 hotkey string (fallback helper).
pub fn format_keystroke_as_hotkey(ev: &KeyDownEvent) -> Option<String> {
    let key = ev.keystroke.key.trim();
    let lower = key.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "control" | "shift" | "alt" | "platform" | "super" | "win" | ""
    ) {
        return None;
    }
    if matches!(lower.as_str(), "backspace" | "delete") && !ev.keystroke.modifiers.control {
        return Some("None".to_string());
    }

    let mut parts: Vec<String> = Vec::new();
    if ev.keystroke.modifiers.control {
        parts.push("Ctrl".to_string());
    }
    if ev.keystroke.modifiers.alt {
        parts.push("Alt".to_string());
    }
    if ev.keystroke.modifiers.shift {
        parts.push("Shift".to_string());
    }

    let main_label = match lower.as_str() {
        "printscreen" | "snapshot" => "Print Screen".to_string(),
        "space" => "Space".to_string(),
        s if s.starts_with('f') && s.len() <= 3 && s[1..].chars().all(|c| c.is_ascii_digit()) => {
            s.to_ascii_uppercase()
        }
        s if s.len() == 1 => s.to_ascii_uppercase(),
        other => {
            let mut ch = other.chars();
            match ch.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), ch.as_str()),
                None => return None,
            }
        }
    };
    parts.push(main_label);
    Some(parts.join("+"))
}

/// Render a smooth Snagit-style `44×22 px` capsule toggle row.
fn render_snagit_pill_toggle(
    id: &'static str,
    label: &'static str,
    checked: bool,
    is_dark: bool,
    theme: SnagTheme,
    on_toggle: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let track_bg = if checked {
        rgb(0x2B7BB9)
    } else if is_dark {
        rgb(0x4B5563)
    } else {
        rgb(0x8D9AA6)
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .py(px(3.5))
        .cursor_pointer()
        .on_click(on_toggle)
        .child(
            div()
                .text_sm()
                .text_color(theme.text_primary)
                .child(label),
        )
        .child(
            div()
                .w(px(44.0))
                .h(px(22.0))
                .rounded(px(11.0))
                .p(px(2.0))
                .bg(track_bg)
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .when(!checked, |el| el.justify_start())
                .child(
                    div()
                        .size(px(18.0))
                        .rounded(px(9.0))
                        .bg(rgb(0xFFFFFF))
                        .shadow_sm(),
                ),
        )
}

/// Pure state model for the Capture Window (easily unit-testable without a display).
#[derive(Debug, Clone)]
pub struct CaptureWindowState {
    pub active: CapturePreset,
    pub presets: Vec<CapturePreset>,
    pub presets_drawer_open: bool,
    pub selected_preset_idx: Option<usize>,
    pub open_menu: Option<CaptureTopMenu>,
    pub prefs: CapturePreferences,
}

impl Default for CaptureWindowState {
    fn default() -> Self {
        let prefs = CapturePreferences::load_or_default();
        let mut default_all = CapturePreset::default();
        default_all.hotkey = if prefs.global_capture_hotkey.eq_ignore_ascii_case("none") {
            None
        } else {
            Some(prefs.global_capture_hotkey.clone())
        };
        default_all.copy_to_clipboard = prefs.auto_copy_to_clipboard;
        default_all.preview_in_editor = true;
        default_all.capture_cursor = true;
        default_all.timed_capture = TimedCaptureConfig {
            enabled: false,
            delay_seconds: 5,
            ..TimedCaptureConfig::default()
        };

        let default_img = CapturePreset {
            id: "preset-image-region".to_string(),
            name: "Region to Clipboard".to_string(),
            kind: CaptureKind::Image,
            selection_mode: SelectionMode::Region,
            hotkey: Some("Ctrl+Shift+C".to_string()),
            capture_cursor: true,
            copy_to_clipboard: true,
            preview_in_editor: true,
            timed_capture: TimedCaptureConfig {
                enabled: false,
                delay_seconds: 5,
                ..TimedCaptureConfig::default()
            },
            video_config: None,
            auto_effects: Vec::new(),
            share_destination: Some("Clipboard".to_string()),
        };
        let default_vid = CapturePreset {
            id: "preset-video-1080p".to_string(),
            name: "Screen Video (60fps)".to_string(),
            kind: CaptureKind::Video,
            selection_mode: SelectionMode::Region,
            hotkey: Some("Ctrl+Shift+V".to_string()),
            capture_cursor: true,
            copy_to_clipboard: false,
            preview_in_editor: true,
            timed_capture: TimedCaptureConfig {
                enabled: false,
                delay_seconds: 5,
                ..TimedCaptureConfig::default()
            },
            video_config: Some(VideoCaptureConfig {
                fps: prefs.default_video_fps,
                ..VideoCaptureConfig::default()
            }),
            auto_effects: Vec::new(),
            share_destination: Some("File (Auto-save)".to_string()),
        };

        Self {
            active: default_all.clone(),
            presets: vec![default_all, default_img, default_vid],
            presets_drawer_open: false,
            selected_preset_idx: Some(0),
            open_menu: None,
            prefs,
        }
    }
}

impl CaptureWindowState {
    /// Switch between All-in-One, Image, and Video tabs, synchronizing `CapturePreset`.
    pub fn set_tab(&mut self, kind: CaptureKind) {
        self.active.kind = kind;
        match kind {
            CaptureKind::AllInOne => {
                self.active.selection_mode = SelectionMode::AllInOne;
            }
            CaptureKind::Image => {
                if self.active.selection_mode == SelectionMode::AllInOne {
                    self.active.selection_mode = SelectionMode::Region;
                }
            }
            CaptureKind::Video => {
                if self.active.selection_mode == SelectionMode::AllInOne {
                    self.active.selection_mode = SelectionMode::Region;
                }
                if self.active.video_config.is_none() {
                    self.active.video_config = Some(VideoCaptureConfig {
                        fps: self.prefs.default_video_fps,
                        ..VideoCaptureConfig::default()
                    });
                }
            }
        }
    }

    /// Reset the current tab's toggles to Snagit defaults (`↺` button in top-right of pane).
    pub fn reset_current_tab_defaults(&mut self) {
        let cur_kind = self.active.kind;
        self.active.preview_in_editor = true;
        self.active.copy_to_clipboard = false;
        self.active.capture_cursor = true;
        self.active.timed_capture = TimedCaptureConfig {
            enabled: false,
            delay_seconds: 5,
            ..TimedCaptureConfig::default()
        };
        self.active.share_destination = None;
        if cur_kind == CaptureKind::Video {
            self.active.video_config = Some(VideoCaptureConfig {
                fps: self.prefs.default_video_fps,
                ..VideoCaptureConfig::default()
            });
        }
        self.set_tab(cur_kind);
    }

    /// Sync updated `CapturePreferences` onto `CaptureWindowState`.
    pub fn apply_updated_preferences(&mut self, new_prefs: CapturePreferences) {
        self.active.hotkey = if new_prefs.global_capture_hotkey.eq_ignore_ascii_case("none") {
            None
        } else {
            Some(new_prefs.global_capture_hotkey.clone())
        };
        self.active.copy_to_clipboard = new_prefs.auto_copy_to_clipboard;
        if let Some(ref mut vc) = self.active.video_config {
            vc.fps = new_prefs.default_video_fps;
        }
        self.prefs = new_prefs;
    }

    /// Desired window height in logical pixels for the Capture App Window.
    pub fn target_window_height(&self) -> f32 {
        if self.presets_drawer_open {
            CAPTURE_WIN_EXPANDED_HEIGHT
        } else {
            CAPTURE_WIN_COMPACT_HEIGHT
        }
    }

    /// Cycle the active [`SelectionMode`] to the next option.
    pub fn cycle_selection_mode(&mut self) {
        let modes = &[
            SelectionMode::Region,
            SelectionMode::Window,
            SelectionMode::FullScreen,
            SelectionMode::ScrollingWindow,
            SelectionMode::Panoramic,
        ];
        let idx = modes
            .iter()
            .position(|&m| m == self.active.selection_mode)
            .unwrap_or(0);
        self.active.selection_mode = modes[(idx + 1) % modes.len()];
    }

    /// Cycle the Share destination.
    pub fn cycle_share_destination(&mut self) {
        let cur = self
            .active
            .share_destination
            .as_deref()
            .unwrap_or("None");
        let idx = SHARE_DESTINATIONS
            .iter()
            .position(|&s| s == cur)
            .unwrap_or(0);
        let next = SHARE_DESTINATIONS[(idx + 1) % SHARE_DESTINATIONS.len()];
        self.active.share_destination = if next == "None" {
            None
        } else {
            Some(next.to_string())
        };
    }

    /// Assign a hotkey string to `slot`, syncing `active.hotkey` if `GlobalCapture`.
    pub fn set_hotkey_slot(&mut self, slot: HotkeySlot, value: String) {
        self.prefs.set_slot(slot, value.clone());
        if slot == HotkeySlot::GlobalCapture {
            self.active.hotkey = if value.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(value)
            };
        }
        self.prefs.save_to_disk();
    }

    /// Save the current active settings as a new preset in the drawer.
    pub fn save_current_as_preset(&mut self, name: impl Into<String>) {
        let mut preset = self.active.clone();
        let next_n = self.presets.len() + 1;
        preset.id = format!("preset-custom-{next_n}");
        preset.name = name.into();
        self.presets.push(preset);
        self.selected_preset_idx = Some(self.presets.len() - 1);
    }

    /// Apply a preset from the drawer by index.
    pub fn apply_preset(&mut self, index: usize) -> bool {
        if let Some(preset) = self.presets.get(index).cloned() {
            self.active = preset;
            self.selected_preset_idx = Some(index);
            true
        } else {
            false
        }
    }

    /// Remove a preset from the drawer by index.
    pub fn delete_preset(&mut self, index: usize) -> bool {
        if index < self.presets.len() {
            self.presets.remove(index);
            self.selected_preset_idx = None;
            true
        } else {
            false
        }
    }

    /// Serialize presets list to JSON for persistence.
    pub fn presets_to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.presets)
    }

    /// Load presets list from JSON string.
    pub fn load_presets_from_json(&mut self, json: &str) -> Result<usize, serde_json::Error> {
        let loaded: Vec<CapturePreset> = serde_json::from_str(json)?;
        let count = loaded.len();
        self.presets = loaded;
        self.selected_preset_idx = if count > 0 { Some(0) } else { None };
        Ok(count)
    }
}

// ============================================================================
// Standalone Clean `CapturePreferencesView` Dialog (`General | Capture | Hotkeys` + Bottom OK/Cancel)
// ============================================================================

pub struct CapturePreferencesView {
    pub tab: PreferencesTab,
    pub draft: CapturePreferences,
    pub recording_slot: Option<HotkeySlot>,
    recording_seq: u64,
    on_saved: Option<Rc<dyn Fn(&CapturePreferences, &mut App) + 'static>>,
}

impl CapturePreferencesView {
    pub fn new(initial_tab: PreferencesTab) -> Self {
        Self {
            tab: initial_tab,
            draft: CapturePreferences::load_or_default(),
            recording_slot: None,
            recording_seq: 0,
            on_saved: None,
        }
    }

    pub fn with_on_saved(
        mut self,
        handler: impl Fn(&CapturePreferences, &mut App) + 'static,
    ) -> Self {
        self.on_saved = Some(Rc::new(handler));
        self
    }

    fn start_win32_recording(&mut self, slot: HotkeySlot, cx: &mut Context<Self>) {
        if self.recording_slot == Some(slot) {
            self.cancel_recording();
            return;
        }
        self.recording_seq = self.recording_seq.wrapping_add(1);
        let seq = self.recording_seq;
        self.recording_slot = Some(slot);
        begin_hotkey_recording();

        cx.spawn(async move |weak, cx| {
            // Brief 90ms debounce so the mouse click / residual key state clears cleanly
            cx.background_executor()
                .timer(Duration::from_millis(90))
                .await;
            begin_hotkey_recording();

            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                let action_opt = poll_hotkey_recording_input();
                let keep_polling = weak
                    .update(cx, |view, cx| {
                        if view.recording_slot != Some(slot) || view.recording_seq != seq {
                            return false;
                        }
                        if let Some(action) = action_opt {
                            match action {
                                RecordedKeyAction::Cancel => {
                                    view.recording_slot = None;
                                    end_hotkey_recording();
                                }
                                RecordedKeyAction::ClearToNone => {
                                    view.draft.set_slot(slot, "None".to_string());
                                    view.recording_slot = None;
                                    end_hotkey_recording();
                                }
                                RecordedKeyAction::Shortcut(shortcut) => {
                                    view.draft.set_slot(slot, shortcut);
                                    view.recording_slot = None;
                                    end_hotkey_recording();
                                }
                            }
                            cx.notify();
                            return false;
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_polling {
                    end_hotkey_recording();
                    break;
                }
            }
        })
        .detach();
    }

    fn cancel_recording(&mut self) {
        if self.recording_slot.is_some() {
            self.recording_slot = None;
            end_hotkey_recording();
        }
    }

    fn section_card(
        title: &'static str,
        theme: SnagTheme,
        content: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .rounded(theme.radius_md)
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .pb(px(4.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_secondary)
                    .child(title),
            )
            .child(content)
    }

    fn render_hotkey_row(
        &self,
        id: &'static str,
        label: &'static str,
        slot: HotkeySlot,
        current_val: &str,
        theme: SnagTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_rec = self.recording_slot == Some(slot);
        let has_key = !current_val.eq_ignore_ascii_case("none") && !current_val.is_empty();
        let display_txt = if is_rec {
            "Press shortcut... (Esc cancel)".to_string()
        } else if has_key {
            current_val.to_string()
        } else {
            "None".to_string()
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .py(px(4.0))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_primary)
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .id(id)
                            .flex()
                            .items_center()
                            .justify_center()
                            .min_w(px(164.0))
                            .h(px(28.0))
                            .px(px(10.0))
                            .rounded(theme.radius_sm)
                            .bg(if is_rec {
                                gpui::Hsla::from(rgba(0x2563EB20))
                            } else {
                                theme.input_bg
                            })
                            .border_1()
                            .border_color(if is_rec {
                                gpui::Hsla::from(rgb(0x2563EB))
                            } else {
                                theme.border_strong
                            })
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if is_rec {
                                gpui::Hsla::from(rgb(0x2563EB))
                            } else if has_key {
                                theme.text_primary
                            } else {
                                theme.text_muted
                            })
                            .cursor_pointer()
                            .hover(|s| s.border_color(rgb(0x2563EB)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.start_win32_recording(slot, cx);
                                cx.notify();
                            }))
                            .child(display_txt),
                    )
                    .child(
                        IconButton::new(format!("{id}-clear"), SnagIcon::Close)
                            .size(px(26.0))
                            .disabled(!has_key && !is_rec)
                            .tooltip("Clear Shortcut", Some("Set to None".into()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cancel_recording();
                                this.draft.set_slot(slot, "None".to_string());
                                cx.notify();
                            })),
                    ),
            )
    }
}

impl Render for CapturePreferencesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);
        let is_dark = self.draft.dark_theme;
        let cur_tab = self.tab;

        // Clean modern top underline tab bar (General | Capture | Hotkeys)
        let tab_items: &[(PreferencesTab, &str)] = &[
            (PreferencesTab::General, "General"),
            (PreferencesTab::Capture, "Capture"),
            (PreferencesTab::Hotkeys, "Hotkeys"),
        ];
        let mut top_tabs = div()
            .flex()
            .flex_row()
            .px(px(16.0))
            .pt(px(6.0))
            .gap(px(18.0))
            .bg(theme.toolbar_bg)
            .border_b_1()
            .border_color(theme.border);

        for &(t_kind, t_label) in tab_items {
            let active = cur_tab == t_kind;
            top_tabs = top_tabs.child(
                div()
                    .id(SharedString::from(format!("prefs-tab-{t_label}")))
                    .pb(px(9.0))
                    .pt(px(6.0))
                    .px(px(4.0))
                    .border_b_2()
                    .border_color(if active {
                        theme.accent_primary
                    } else {
                        gpui::transparent_black()
                    })
                    .text_sm()
                    .font_weight(if active {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::MEDIUM
                    })
                    .text_color(if active {
                        theme.accent_primary
                    } else {
                        theme.text_secondary
                    })
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text_primary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cancel_recording();
                        this.tab = t_kind;
                        cx.notify();
                    }))
                    .child(t_label),
            );
        }

        // Spacious grouped body content
        let body_content = match cur_tab {
            PreferencesTab::General => div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(Self::section_card(
                    "System & Startup",
                    theme,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(render_snagit_pill_toggle(
                            "pref-start-win",
                            "Run SnagSoup when Windows starts",
                            self.draft.run_when_windows_starts,
                            is_dark,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.draft.run_when_windows_starts =
                                    !this.draft.run_when_windows_starts;
                                cx.notify();
                            }),
                        ))
                        .child(render_snagit_pill_toggle(
                            "pref-close-tray",
                            "Keep running in System Tray on close",
                            self.draft.close_to_tray,
                            is_dark,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.draft.close_to_tray = !this.draft.close_to_tray;
                                cx.notify();
                            }),
                        ))
                        .child(render_snagit_pill_toggle(
                            "pref-oneclick",
                            "Show OneClick floating capture bar",
                            self.draft.show_oneclick_widget,
                            is_dark,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.draft.show_oneclick_widget =
                                    !this.draft.show_oneclick_widget;
                                cx.notify();
                            }),
                        )),
                ))
                .child(Self::section_card(
                    "Appearance & Feedback",
                    theme,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(render_snagit_pill_toggle(
                            "pref-dark-theme",
                            "Dark Studio UI Theme",
                            self.draft.dark_theme,
                            is_dark,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.draft.dark_theme = !this.draft.dark_theme;
                                let next_mode = if this.draft.dark_theme {
                                    ThemeMode::Dark
                                } else {
                                    ThemeMode::Light
                                };
                                cx.set_global(SnagTheme::for_mode(next_mode));
                                cx.notify();
                            }),
                        ))
                        .child(render_snagit_pill_toggle(
                            "pref-tips",
                            "Show notification tips & HUD hints",
                            self.draft.show_notification_tips,
                            is_dark,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.draft.show_notification_tips =
                                    !this.draft.show_notification_tips;
                                cx.notify();
                            }),
                        )),
                ))
                .into_any_element(),

            PreferencesTab::Capture => {
                let fps_lbl = format!("{} fps", self.draft.default_video_fps);
                let qual_lbl = format!("Quality: {}", self.draft.video_quality);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(Self::section_card(
                        "Capture Behavior",
                        theme,
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(render_snagit_pill_toggle(
                                "pref-hide-before",
                                "Hide SnagSoup before capturing",
                                self.draft.hide_snagsoup_before_capturing,
                                is_dark,
                                theme,
                                cx.listener(|this, _, _, cx| {
                                    this.draft.hide_snagsoup_before_capturing =
                                        !this.draft.hide_snagsoup_before_capturing;
                                    cx.notify();
                                }),
                            ))
                            .child(render_snagit_pill_toggle(
                                "pref-magnifier",
                                "Show pixel magnifier during selection",
                                self.draft.show_magnifier,
                                is_dark,
                                theme,
                                cx.listener(|this, _, _, cx| {
                                    this.draft.show_magnifier = !this.draft.show_magnifier;
                                    cx.notify();
                                }),
                            ))
                            .child(render_snagit_pill_toggle(
                                "pref-aio-bar",
                                "Show All-in-One toolbar after selection",
                                self.draft.show_all_in_one_toolbar,
                                is_dark,
                                theme,
                                cx.listener(|this, _, _, cx| {
                                    this.draft.show_all_in_one_toolbar =
                                        !this.draft.show_all_in_one_toolbar;
                                    cx.notify();
                                }),
                            ))
                            .child(render_snagit_pill_toggle(
                                "pref-auto-clip",
                                "Copy captures to Clipboard automatically",
                                self.draft.auto_copy_to_clipboard,
                                is_dark,
                                theme,
                                cx.listener(|this, _, _, cx| {
                                    this.draft.auto_copy_to_clipboard =
                                        !this.draft.auto_copy_to_clipboard;
                                    cx.notify();
                                }),
                            )),
                    ))
                    .child(Self::section_card(
                        "Video Recording Defaults",
                        theme,
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .pt(px(2.0))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.text_primary)
                                    .child("Default Encoder Settings"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(6.0))
                                    .child(
                                        Button::new("pref-vid-fps", fps_lbl)
                                            .variant(ButtonVariant::Secondary)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.draft.default_video_fps =
                                                    if this.draft.default_video_fps == 60 {
                                                        30
                                                    } else {
                                                        60
                                                    };
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("pref-vid-qual", qual_lbl)
                                            .variant(ButtonVariant::Secondary)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.draft.video_quality =
                                                    match this.draft.video_quality.as_str() {
                                                        "High" => "Medium".to_string(),
                                                        "Medium" => "Low".to_string(),
                                                        _ => "High".to_string(),
                                                    };
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ))
                    .into_any_element()
            }

            PreferencesTab::Hotkeys => {
                let g_hk = self.draft.global_capture_hotkey.clone();
                let r_hk = self.draft.repeat_last_capture_hotkey.clone();
                let vp_hk = self.draft.video_record_pause_hotkey.clone();
                let vs_hk = self.draft.video_stop_hotkey.clone();
                let sh_hk = self.draft.show_hide_hotkey.clone();

                Self::section_card(
                    "Global Keyboard Shortcuts (Click box to record)",
                    theme,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(self.render_hotkey_row(
                            "hk-global",
                            "Global Capture",
                            HotkeySlot::GlobalCapture,
                            &g_hk,
                            theme,
                            cx,
                        ))
                        .child(self.render_hotkey_row(
                            "hk-repeat",
                            "Repeat Last Capture",
                            HotkeySlot::RepeatLastCapture,
                            &r_hk,
                            theme,
                            cx,
                        ))
                        .child(self.render_hotkey_row(
                            "hk-vid-rec",
                            "Video Record / Pause",
                            HotkeySlot::VideoRecordPause,
                            &vp_hk,
                            theme,
                            cx,
                        ))
                        .child(self.render_hotkey_row(
                            "hk-vid-stop",
                            "Video Stop",
                            HotkeySlot::VideoStop,
                            &vs_hk,
                            theme,
                            cx,
                        ))
                        .child(self.render_hotkey_row(
                            "hk-show-hide",
                            "Show / Hide SnagSoup",
                            HotkeySlot::ShowHideApp,
                            &sh_hk,
                            theme,
                            cx,
                        )),
                )
                .into_any_element()
            }
        };

        // Standard Desktop Bottom Footer (`Restore Defaults` on left in Hotkeys, `Cancel` + `OK` on right)
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(52.0))
            .px(px(16.0))
            .bg(theme.panel_bg)
            .border_t_1()
            .border_color(theme.border)
            .child(if cur_tab == PreferencesTab::Hotkeys {
                Button::new("hk-restore-defaults", "Restore Defaults")
                    .variant(ButtonVariant::Secondary)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cancel_recording();
                        this.draft.restore_default_hotkeys();
                        cx.notify();
                    }))
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        Button::new("prefs-cancel-btn", "Cancel")
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_recording();
                                // Restore persisted theme if user toggled Dark preview and hit Cancel
                                let saved = CapturePreferences::load_or_default();
                                cx.set_global(SnagTheme::for_mode(if saved.dark_theme {
                                    ThemeMode::Dark
                                } else {
                                    ThemeMode::Light
                                }));
                                window.remove_window();
                            })),
                    )
                    .child(
                        Button::new("prefs-ok-btn", "   OK   ")
                            .variant(ButtonVariant::Primary)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_recording();
                                let _ = snagsoup_win_platform::startup::set_run_at_windows_startup(
                                    this.draft.run_when_windows_starts,
                                );
                                this.draft.save_to_disk();
                                cx.set_global(SnagTheme::for_mode(if this.draft.dark_theme {
                                    ThemeMode::Dark
                                } else {
                                    ThemeMode::Light
                                }));
                                if let Some(ref cb) = this.on_saved {
                                    cb(&this.draft, cx);
                                }
                                window.remove_window();
                            })),
                    ),
            );

        div()
            .id("snagsoup-preferences-window")
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.app_bg)
            .text_color(theme.text_primary)
            .child(top_tabs)
            .child(
                div()
                    .id("prefs-scroll-body")
                    .flex_1()
                    .p(px(16.0))
                    .overflow_y_scroll()
                    .child(body_content),
            )
            .child(footer)
    }
}

// ============================================================================
// Standalone `AboutDialogView` (`Help > About`)
// ============================================================================

pub struct AboutDialogView;

impl AboutDialogView {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AboutDialogView {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for AboutDialogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);

        div()
            .id("snagsoup-about-dialog")
            .flex()
            .flex_col()
            .justify_between()
            .size_full()
            .bg(theme.app_bg)
            .text_color(theme.text_primary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(16.0))
                    .p(px(20.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(54.0))
                            .rounded(px(14.0))
                            .bg(rgb(0x059669))
                            .border_2()
                            .border_color(rgb(0x34D399))
                            .shadow_md()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child("S"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .flex_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.text_primary)
                                    .child(crate::APP_TITLE),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child("High-Performance Multi-Monitor Capture & Editor"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .pt(px(2.0))
                                    .child("Engine: Native Rust + GPUI DirectX 11 · 100% Local-First"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(format!("Version {}", crate::APP_VERSION_FULL)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .h(px(48.0))
                    .px(px(16.0))
                    .bg(theme.panel_bg)
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("about-ok-btn", "   OK   ")
                            .variant(ButtonVariant::Primary)
                            .on_click(|_, window, _| {
                                window.remove_window();
                            }),
                    ),
            )
    }
}

// ============================================================================
// Main Snagit-Style `CaptureWindowView`
// ============================================================================

pub struct CaptureWindowView {
    pub state: CaptureWindowState,
    on_capture: Option<Rc<dyn Fn(&CapturePreset, &mut Window, &mut App) + 'static>>,
    on_open_editor: Option<Rc<dyn Fn(&mut Window, &mut App) + 'static>>,
}

impl Default for CaptureWindowView {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureWindowView {
    pub fn new() -> Self {
        Self {
            state: CaptureWindowState::default(),
            on_capture: None,
            on_open_editor: None,
        }
    }

    pub fn with_on_capture(
        mut self,
        handler: impl Fn(&CapturePreset, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_capture = Some(Rc::new(handler));
        self
    }

    pub fn with_on_open_editor(
        mut self,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_editor = Some(Rc::new(handler));
        self
    }

    fn sync_window_height(&self, window: &mut Window) {
        let cur_w: f32 = window.viewport_size().width.into();
        let target_h = self.state.target_window_height();
        window.resize(size(px(cur_w.max(CAPTURE_WIN_WIDTH)), px(target_h)));
    }
}

impl Render for CaptureWindowView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx
            .try_global::<SnagTheme>()
            .copied()
            .unwrap_or_else(SnagTheme::light);
        let is_dark = theme.mode == ThemeMode::Dark;

        let header_blue = rgb(0x3A82C4);
        let body_surface = if is_dark {
            theme.app_bg
        } else {
            gpui::Hsla::from(rgb(0xEEF2F6))
        };
        let inactive_tab_bg = if is_dark {
            gpui::Hsla::from(rgb(0x161B22))
        } else {
            gpui::Hsla::from(rgb(0xCFD7DF))
        };
        let footer_bg = if is_dark {
            theme.panel_bg
        } else {
            gpui::Hsla::from(rgb(0xE3EAF0))
        };

        // 1. Solid Blue Integrated Top Bar (`[Emerald S] File Edit View Help ... v1.0.0.0 [─] [✕]`)
        let open_menu = self.state.open_menu;
        let menu_items: &[(CaptureTopMenu, &str)] = &[
            (CaptureTopMenu::File, "File"),
            (CaptureTopMenu::Edit, "Edit"),
            (CaptureTopMenu::View, "View"),
            (CaptureTopMenu::Help, "Help"),
        ];

        let mut menu_strip = div().flex().flex_row().items_center().gap(px(2.0));
        for &(m_kind, m_label) in menu_items {
            let is_open = open_menu == Some(m_kind);
            menu_strip = menu_strip.child(
                div()
                    .id(SharedString::from(format!("cap-top-menu-{m_label}")))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(if is_open {
                        gpui::Hsla::from(rgba(0xFFFFFF2E))
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(|s| s.bg(rgba(0xFFFFFF22)))
                    .text_xs()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(0xFFFFFF))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.open_menu = if this.state.open_menu == Some(m_kind) {
                            None
                        } else {
                            Some(m_kind)
                        };
                        cx.notify();
                    }))
                    .child(m_label),
            );
        }

        let brand_badge = div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(20.0))
            .rounded(px(5.0))
            .bg(rgb(0x059669))
            .border_1()
            .border_color(rgba(0xFFFFFF55))
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(0xFFFFFF))
            .child("S");

        let top_blue_header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(36.0))
            .pl(px(10.0))
            .bg(header_blue)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(brand_badge)
                    .child(menu_strip),
            )
            .child(
                div()
                    .id("cap-header-drag-zone")
                    .flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .justify_end()
                    .pr(px(8.0))
                    .window_control_area(WindowControlArea::Drag)
                    .text_xs()
                    .text_color(rgba(0xFFFFFFBF))
                    .child(crate::APP_TITLE),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h_full()
                    .child(
                        div()
                            .id("cap-win-theme")
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(36.0))
                            .h_full()
                            .hover(|s| s.bg(rgba(0xFFFFFF24)))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next_mode = cx
                                    .try_global::<SnagTheme>()
                                    .map(|t| t.mode.toggled())
                                    .unwrap_or(ThemeMode::Dark);
                                this.state.prefs.dark_theme = next_mode == ThemeMode::Dark;
                                this.state.prefs.save_to_disk();
                                cx.set_global(SnagTheme::for_mode(next_mode));
                                cx.notify();
                            }))
                            .child(
                                (if is_dark {
                                    SnagIcon::Sun
                                } else {
                                    SnagIcon::Moon
                                })
                                .render(px(14.0), rgb(0xFFFFFF).into()),
                            ),
                    )
                    .child(
                        div()
                            .id("cap-win-minimize")
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(38.0))
                            .h_full()
                            .hover(|s| s.bg(rgba(0xFFFFFF24)))
                            .cursor_pointer()
                            .on_click(|_, window, _| {
                                window.minimize_window();
                            })
                            .child(SnagIcon::Minus.render(px(14.0), rgb(0xFFFFFF).into())),
                    )
                    .child(
                        div()
                            .id("cap-win-close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(38.0))
                            .h_full()
                            .hover(|s| s.bg(rgb(0xE81123)))
                            .cursor_pointer()
                            .on_click(|_, window, cx| {
                                crate::handle_capture_window_close_action(window, cx);
                            })
                            .child(SnagIcon::Close.render(px(14.0), rgb(0xFFFFFF).into())),
                    ),
            );

        // 2. Left Vertical 3-Tab Switcher (`All-in-One`, `Image`, `Video`)
        let kind = self.state.active.kind;
        let tabs: &[(CaptureKind, SnagIcon, &str)] = &[
            (CaptureKind::AllInOne, SnagIcon::RecordDot, "All-in-One"),
            (CaptureKind::Image, SnagIcon::CameraCapture, "Image"),
            (CaptureKind::Video, SnagIcon::VideoCapture, "Video"),
        ];

        let mut tab_col = div().flex().flex_col().w(px(86.0)).bg(inactive_tab_bg);

        for &(tab_kind, icon, title) in tabs {
            let active = kind == tab_kind;
            let icon_color = if active {
                gpui::Hsla::from(rgb(0x2B7BB9))
            } else if is_dark {
                theme.text_secondary
            } else {
                gpui::Hsla::from(rgb(0x475569))
            };
            tab_col = tab_col.child(
                div()
                    .id(SharedString::from(format!("cap-tab-{title}")))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(4.0))
                    .px(px(6.0))
                    .py(px(8.0))
                    .bg(if active {
                        body_surface
                    } else {
                        inactive_tab_bg
                    })
                    .when(!active, |el| {
                        el.border_b_1()
                            .border_r_1()
                            .border_color(theme.border)
                            .hover(|s| s.opacity(0.9))
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.open_menu = None;
                        this.state.set_tab(tab_kind);
                        cx.notify();
                    }))
                    .child(icon.render(px(20.0), icon_color))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(if active {
                                gpui::FontWeight::SEMIBOLD
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .text_color(theme.text_primary)
                            .child(title),
                    ),
            );
        }

        // 3. Center Settings Column
        let delay_5s_on = self.state.active.timed_capture.enabled;
        let share_label = self
            .state
            .active
            .share_destination
            .clone()
            .unwrap_or_else(|| "None".to_string());
        let video_cfg = self
            .state
            .active
            .video_config
            .clone()
            .unwrap_or_default();

        let toggles_stack = match kind {
            CaptureKind::AllInOne => div()
                .flex()
                .flex_col()
                .justify_center()
                .flex_1()
                .gap(px(6.0))
                .child(render_snagit_pill_toggle(
                    "toggle-preview",
                    "Preview in Editor",
                    self.state.active.preview_in_editor,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.preview_in_editor = !this.state.active.preview_in_editor;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "toggle-clipboard",
                    "Copy to Clipboard",
                    self.state.active.copy_to_clipboard,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.copy_to_clipboard = !this.state.active.copy_to_clipboard;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "toggle-cursor",
                    "Capture Cursor",
                    self.state.active.capture_cursor,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.capture_cursor = !this.state.active.capture_cursor;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "toggle-delay-5s",
                    "5 Second Delay",
                    delay_5s_on,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.timed_capture.enabled =
                            !this.state.active.timed_capture.enabled;
                        this.state.active.timed_capture.delay_seconds = 5;
                        cx.notify();
                    }),
                ))
                .into_any_element(),

            CaptureKind::Image => div()
                .flex()
                .flex_col()
                .justify_center()
                .flex_1()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(6.0))
                        .pb(px(2.0))
                        .child(
                            Button::new(
                                "sel-mode-btn",
                                format!(
                                    "Mode: {}",
                                    selection_mode_label(self.state.active.selection_mode)
                                ),
                            )
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.cycle_selection_mode();
                                cx.notify();
                            })),
                        )
                        .child(
                            Button::new("share-dest-btn", format!("Share: {share_label}"))
                                .variant(ButtonVariant::Secondary)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.cycle_share_destination();
                                    cx.notify();
                                })),
                        ),
                )
                .child(render_snagit_pill_toggle(
                    "img-toggle-preview",
                    "Preview in Editor",
                    self.state.active.preview_in_editor,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.preview_in_editor = !this.state.active.preview_in_editor;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "img-toggle-clip",
                    "Copy to Clipboard",
                    self.state.active.copy_to_clipboard,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.copy_to_clipboard = !this.state.active.copy_to_clipboard;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "img-toggle-cursor",
                    "Capture Cursor",
                    self.state.active.capture_cursor,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.capture_cursor = !this.state.active.capture_cursor;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "img-toggle-delay",
                    "5 Second Delay",
                    delay_5s_on,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.timed_capture.enabled =
                            !this.state.active.timed_capture.enabled;
                        this.state.active.timed_capture.delay_seconds = 5;
                        cx.notify();
                    }),
                ))
                .into_any_element(),

            CaptureKind::Video => div()
                .flex()
                .flex_col()
                .justify_center()
                .flex_1()
                .gap(px(4.0))
                .child(render_snagit_pill_toggle(
                    "vid-toggle-preview",
                    "Preview in Editor",
                    self.state.active.preview_in_editor,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.preview_in_editor = !this.state.active.preview_in_editor;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "vid-toggle-cursor",
                    "Capture Cursor",
                    self.state.active.capture_cursor,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.state.active.capture_cursor = !this.state.active.capture_cursor;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "vid-toggle-mic",
                    "Record Microphone",
                    video_cfg.record_microphone,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        let vc = this
                            .state
                            .active
                            .video_config
                            .get_or_insert_with(VideoCaptureConfig::default);
                        vc.record_microphone = !vc.record_microphone;
                        cx.notify();
                    }),
                ))
                .child(render_snagit_pill_toggle(
                    "vid-toggle-sys",
                    "Record System Audio",
                    video_cfg.record_system_audio,
                    is_dark,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        let vc = this
                            .state
                            .active
                            .video_config
                            .get_or_insert_with(VideoCaptureConfig::default);
                        vc.record_system_audio = !vc.record_system_audio;
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        };

        let center_pane = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .pl(px(6.0))
            .pr(px(12.0))
            .py(px(10.0))
            .gap(px(8.0))
            .child(
                div()
                    .id("cap-cycle-tab-chevron")
                    .p(px(4.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let next = match this.state.active.kind {
                            CaptureKind::AllInOne => CaptureKind::Image,
                            CaptureKind::Image => CaptureKind::Video,
                            CaptureKind::Video => CaptureKind::AllInOne,
                        };
                        this.state.set_tab(next);
                        cx.notify();
                    }))
                    .child(SnagIcon::ChevronRight.render(px(18.0), theme.text_secondary)),
            )
            .child(toggles_stack);

        // 4. Right Column: Reset `↺` + Large Circular Red `Capture` Button + Hotkey Badge
        let on_capture = self.on_capture.clone();
        let on_capture_menu = self.on_capture.clone();
        let on_open_editor = self.on_open_editor.clone();
        let on_open_editor_menu = self.on_open_editor.clone();
        let active_preset_snapshot = self.state.active.clone();
        let active_preset_for_menu = self.state.active.clone();
        let hotkey_label = self
            .state
            .active
            .hotkey
            .clone()
            .unwrap_or_else(|| "Add Hotkey".to_string());

        let right_capture_col = div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .w(px(148.0))
            .py(px(10.0))
            .pr(px(12.0))
            .gap(px(8.0))
            .child(
                div()
                    .absolute()
                    .top(px(6.0))
                    .right(px(8.0))
                    .child(
                        IconButton::new("cap-reset-tab", SnagIcon::RedoCycle)
                            .size(px(26.0))
                            .tooltip("Reset Settings", Some("Restore defaults for current tab".into()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.reset_current_tab_defaults();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("big-red-capture")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(94.0))
                    .rounded(px(47.0))
                    .bg(rgb(0xDC2F18))
                    .border_2()
                    .border_color(rgb(0xA91E0D))
                    .shadow_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xC62611)))
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .on_click(move |_, window, cx| {
                        if let Some(cb) = &on_capture {
                            cb(&active_preset_snapshot, window, cx);
                        }
                    })
                    .child("Capture"),
            )
            .child(
                div()
                    .id("cap-hotkey-label")
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .hover(|s| s.bg(theme.surface_hover))
                    .text_xs()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.open_menu = None;
                        crate::open_or_focus_preferences_window(PreferencesTab::Hotkeys, cx);
                        cx.notify();
                    }))
                    .child(hotkey_label),
            );

        // 5. Bottom Footer Strip (`⚙ Presets  +▾` on left, `[☑ Open Editor]` on right)
        let footer_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(38.0))
            .px(px(12.0))
            .bg(footer_bg)
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .id("toggle-presets-drawer")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(theme.radius_sm)
                            .hover(|s| s.bg(theme.surface_hover))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.state.open_menu = None;
                                this.state.presets_drawer_open = !this.state.presets_drawer_open;
                                this.sync_window_height(window);
                                cx.notify();
                            }))
                            .child(SnagIcon::Settings.render(px(15.0), theme.text_secondary))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text_primary)
                                    .child("Presets"),
                            ),
                    )
                    .child(
                        div()
                            .id("add-preset")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(1.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(theme.radius_sm)
                            .hover(|s| s.bg(theme.surface_hover))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                let label = format!(
                                    "{} Preset {}",
                                    selection_mode_label(this.state.active.selection_mode),
                                    this.state.presets.len() + 1
                                );
                                this.state.save_current_as_preset(label);
                                this.state.presets_drawer_open = true;
                                this.sync_window_height(window);
                                cx.notify();
                            }))
                            .child(SnagIcon::Plus.render(px(13.0), theme.text_primary))
                            .child(SnagIcon::ChevronDown.render(px(11.0), theme.text_secondary)),
                    ),
            )
            .child(
                div()
                    .id("cap-open-editor")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(theme.radius_sm)
                    .hover(|s| s.bg(theme.surface_hover))
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        if let Some(cb) = &on_open_editor {
                            cb(window, cx);
                        } else {
                            crate::open_or_focus_editor_window(None, cx);
                        }
                    })
                    .child(SnagIcon::Editor.render(px(15.0), theme.text_secondary))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child("Open Editor"),
                    ),
            );

        // Collapsible Presets Drawer
        let mut preset_rows = div()
            .id("cap-presets-scroll")
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(8.0))
            .h(px(144.0))
            .overflow_y_scroll()
            .bg(theme.panel_bg)
            .border_t_1()
            .border_color(theme.border);

        for (idx, preset) in self.state.presets.iter().enumerate() {
            let selected = self.state.selected_preset_idx == Some(idx);
            let name = preset.name.clone();
            let hk = preset.hotkey.clone().unwrap_or_else(|| "—".to_string());
            preset_rows = preset_rows.child(
                div()
                    .id(SharedString::from(format!("preset-row-{idx}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(theme.radius_sm)
                    .cursor_pointer()
                    .bg(if selected {
                        theme.surface_active
                    } else {
                        theme.surface_elevated
                    })
                    .border_1()
                    .border_color(if selected {
                        theme.accent_primary
                    } else {
                        theme.border
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.apply_preset(idx);
                        cx.notify();
                    }))
                    .child(div().text_xs().text_color(theme.text_primary).child(name))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().text_xs().text_color(theme.text_muted).child(hk))
                            .child(
                                IconButton::new(format!("del-preset-{idx}"), SnagIcon::Close)
                                    .size(px(20.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.delete_preset(idx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }

        div()
            .id("capture-app-root")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(body_surface)
            .border_1()
            .border_color(theme.border_strong)
            .child(top_blue_header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.state.open_menu.is_some() {
                                this.state.open_menu = None;
                                cx.notify();
                            }
                        }),
                    )
                    .child(tab_col)
                    .child(center_pane)
                    .child(right_capture_col),
            )
            .child(footer_bar)
            .when(self.state.presets_drawer_open, |el| el.child(preset_rows))
            // Click-outside transparent backdrop for top menu bar
            .when(open_menu.is_some(), |el| {
                el.child(
                    div()
                        .id("cap-menu-backdrop")
                        .occlude()
                        .absolute()
                        .top(px(36.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.state.open_menu = None;
                                cx.notify();
                            }),
                        ),
                )
            })
            // Topmost Dropdown Menu Layer (`File`, `Edit`, `View`, `Help > About`)
            .when_some(open_menu, |el, menu_kind| {
                let menu_left = match menu_kind {
                    CaptureTopMenu::File => 38.0,
                    CaptureTopMenu::Edit => 74.0,
                    CaptureTopMenu::View => 114.0,
                    CaptureTopMenu::Help => 158.0,
                };
                let menu_content = match menu_kind {
                    CaptureTopMenu::File => div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            Button::new("m-file-cap", "Capture Now")
                                .variant(ButtonVariant::Ghost)
                                .align_left()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.state.open_menu = None;
                                    if let Some(ref cb) = on_capture_menu {
                                        cb(&active_preset_for_menu, window, cx);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("m-file-ed", "Open Editor")
                                .variant(ButtonVariant::Ghost)
                                .align_left()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.state.open_menu = None;
                                    if let Some(ref cb) = on_open_editor_menu {
                                        cb(window, cx);
                                    } else {
                                        crate::open_or_focus_editor_window(None, cx);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("m-file-prefs", "Capture Preferences...")
                                .variant(ButtonVariant::Ghost)
                                .align_left()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.open_menu = None;
                                    crate::open_or_focus_preferences_window(
                                        PreferencesTab::General,
                                        cx,
                                    );
                                    cx.notify();
                                })),
                        )
                        .child(div().my(px(2.0)).h(px(1.0)).w_full().bg(theme.border))
                        .child(
                            Button::new("m-file-exit", "Exit SnagSoup")
                                .variant(ButtonVariant::Ghost)
                                .align_left()
                                .on_click(|_, _, cx| {
                                    crate::quit_snagsoup_application(cx);
                                }),
                        )
                        .into_any_element(),

                    // Menu Edit: clean, without Capture Preferences or Configure Hotkeys!
                    CaptureTopMenu::Edit => div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            Button::new("m-edit-reset", "Reset Current Tab Settings")
                                .variant(ButtonVariant::Ghost)
                                .align_left()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.open_menu = None;
                                    this.state.reset_current_tab_defaults();
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),

                    CaptureTopMenu::View => div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            Button::new("m-view-presets", "Toggle Presets Drawer")
                                .variant(ButtonVariant::Ghost)
                                .align_left()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.state.open_menu = None;
                                    this.state.presets_drawer_open =
                                        !this.state.presets_drawer_open;
                                    this.sync_window_height(window);
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),

                    // Menu Help > About sub-menu!
                    CaptureTopMenu::Help => div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            Button::new("m-help-about", "About")
                                .variant(ButtonVariant::Ghost)
                                .align_left()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.open_menu = None;
                                    crate::open_or_focus_about_window(cx);
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                };

                el.child(
                    div()
                        .id("cap-menu-popover")
                        .occlude()
                        .absolute()
                        .top(px(36.0))
                        .left(px(menu_left))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(Popover::new(menu_content).width(px(210.0))),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_tabs_update_model_and_presets_roundtrip() {
        let mut st = CaptureWindowState::default();
        assert_eq!(st.active.kind, CaptureKind::AllInOne);
        assert!(
            !st.presets_drawer_open,
            "Presets drawer must start compact/closed like Snagit"
        );

        st.set_tab(CaptureKind::Image);
        assert_eq!(st.active.kind, CaptureKind::Image);
        assert_eq!(st.active.selection_mode, SelectionMode::Region);

        st.set_tab(CaptureKind::Video);
        assert_eq!(st.active.kind, CaptureKind::Video);
        assert!(st.active.video_config.is_some());

        let before = st.presets.len();
        st.save_current_as_preset("My Video Preset");
        assert_eq!(st.presets.len(), before + 1);

        let json = st.presets_to_json().unwrap();
        let mut st2 = CaptureWindowState::default();
        let count = st2.load_presets_from_json(&json).unwrap();
        assert_eq!(count, before + 1);
        assert_eq!(st2.presets.last().unwrap().name, "My Video Preset");
    }

    #[test]
    fn preferences_hotkeys_and_reset_tab_work() {
        let orig = CapturePreferences::load_or_default();
        let mut st = CaptureWindowState::default();
        st.active.preview_in_editor = false;
        st.active.timed_capture.enabled = true;
        st.reset_current_tab_defaults();
        assert!(st.active.preview_in_editor);
        assert!(!st.active.timed_capture.enabled);

        st.set_hotkey_slot(HotkeySlot::GlobalCapture, "Ctrl+Shift+S".to_string());
        assert_eq!(st.prefs.global_capture_hotkey, "Ctrl+Shift+S");
        assert_eq!(st.active.hotkey.as_deref(), Some("Ctrl+Shift+S"));

        let val = st.prefs.to_json_value();
        let decoded = CapturePreferences::from_json_value(&val);
        assert_eq!(decoded.global_capture_hotkey, "Ctrl+Shift+S");

        st.prefs.restore_default_hotkeys();
        assert_eq!(st.prefs.global_capture_hotkey, "Print Screen");
        orig.save_to_disk();
    }
}
