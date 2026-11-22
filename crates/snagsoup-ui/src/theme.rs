//! Design system and Light/Dark theme tokens for SnagSoup.
//!
//! - `ThemeMode::Light` matches TechSmith Snagit 2022 `EditorTheme` (crisp light chrome,
//!   TechSmith blue primary accent `#1B75BC`, iconic Snagit Capture red `#D92B2B`).
//! - `ThemeMode::Dark` provides a modern high-contrast dark studio theme.

use gpui::{Global, Hsla, Pixels, px, rgb, rgba};

/// Active UI color theme mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Classic Snagit 2022 Light theme.
    #[default]
    Light,
    /// Modern Dark studio theme.
    Dark,
}

impl ThemeMode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// Complete semantic color and spacing token palette for SnagSoup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnagTheme {
    pub mode: ThemeMode,

    // Surfaces
    pub app_bg: Hsla,
    pub toolbar_bg: Hsla,
    pub panel_bg: Hsla,
    pub canvas_well_bg: Hsla,
    pub surface_elevated: Hsla,
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub input_bg: Hsla,

    // Borders & Dividers
    pub border: Hsla,
    pub border_strong: Hsla,
    pub focus_ring: Hsla,

    // Typography
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub text_on_accent: Hsla,

    // Brand & Semantic Accents
    pub accent_primary: Hsla,
    pub accent_primary_hover: Hsla,
    pub capture_red: Hsla,
    pub capture_red_hover: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,

    // Tooltip & Popover
    pub tooltip_bg: Hsla,
    pub tooltip_border: Hsla,
    pub tooltip_text: Hsla,
    pub shadow_color: Hsla,

    // Geometry tokens
    pub radius_sm: Pixels,
    pub radius_md: Pixels,
    pub radius_lg: Pixels,
    pub toolbar_height: Pixels,
    pub panel_width: Pixels,
}

impl Global for SnagTheme {}

impl Default for SnagTheme {
    fn default() -> Self {
        Self::light()
    }
}

impl SnagTheme {
    /// Returns the theme tokens for the requested [`ThemeMode`].
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
        }
    }

    /// Snagit 2022 Light theme tokens (`EditorTheme`).
    pub fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            app_bg: rgb(0xF3F4F6).into(),
            toolbar_bg: rgb(0xFFFFFF).into(),
            panel_bg: rgb(0xF8F9FA).into(),
            canvas_well_bg: rgb(0xE5E7EB).into(),
            surface_elevated: rgb(0xFFFFFF).into(),
            surface_hover: rgb(0xE9ECEF).into(),
            surface_active: rgb(0xDCEAF7).into(),
            input_bg: rgb(0xFFFFFF).into(),

            border: rgb(0xD5D9DF).into(),
            border_strong: rgb(0xB8C0CC).into(),
            focus_ring: rgb(0x1B75BC).into(),

            text_primary: rgb(0x1F2328).into(),
            text_secondary: rgb(0x4B5563).into(),
            text_muted: rgb(0x6B7280).into(),
            text_on_accent: rgb(0xFFFFFF).into(),

            accent_primary: rgb(0x1B75BC).into(),
            accent_primary_hover: rgb(0x155E99).into(),
            capture_red: rgb(0xD92B2B).into(),
            capture_red_hover: rgb(0xB81F1F).into(),
            success: rgb(0x16A34A).into(),
            warning: rgb(0xD97706).into(),
            danger: rgb(0xDC2626).into(),

            tooltip_bg: rgb(0x1E2229).into(),
            tooltip_border: rgb(0x374151).into(),
            tooltip_text: rgb(0xF9FAFB).into(),
            shadow_color: rgba(0x00000028).into(),

            radius_sm: px(4.0),
            radius_md: px(6.0),
            radius_lg: px(10.0),
            toolbar_height: px(44.0),
            panel_width: px(280.0),
        }
    }

    /// Modern Dark studio theme tokens.
    pub fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            app_bg: rgb(0x181A1F).into(),
            toolbar_bg: rgb(0x21242B).into(),
            panel_bg: rgb(0x1E2026).into(),
            canvas_well_bg: rgb(0x121418).into(),
            surface_elevated: rgb(0x282C34).into(),
            surface_hover: rgb(0x323742).into(),
            surface_active: rgb(0x243B55).into(),
            input_bg: rgb(0x16181D).into(),

            border: rgb(0x343944).into(),
            border_strong: rgb(0x4B5263).into(),
            focus_ring: rgb(0x3B9AE1).into(),

            text_primary: rgb(0xF3F4F6).into(),
            text_secondary: rgb(0xB8C0CC).into(),
            text_muted: rgb(0x8892A2).into(),
            text_on_accent: rgb(0xFFFFFF).into(),

            accent_primary: rgb(0x2D8CE0).into(),
            accent_primary_hover: rgb(0x439CEB).into(),
            capture_red: rgb(0xE03131).into(),
            capture_red_hover: rgb(0xF03E3E).into(),
            success: rgb(0x22C55E).into(),
            warning: rgb(0xF59E0B).into(),
            danger: rgb(0xEF4444).into(),

            tooltip_bg: rgb(0x111318).into(),
            tooltip_border: rgb(0x3E4552).into(),
            tooltip_text: rgb(0xF3F4F6).into(),
            shadow_color: rgba(0x00000066).into(),

            radius_sm: px(4.0),
            radius_md: px(6.0),
            radius_lg: px(10.0),
            toolbar_height: px(44.0),
            panel_width: px(280.0),
        }
    }
}

/// Default Snagit-inspired quick color swatch palette.
pub const DEFAULT_SWATCHES: &[u32] = &[
    0xD92B2B, // Snagit Red
    0xF97316, // Orange
    0xFACC15, // Yellow
    0x22C55E, // Green
    0x1B75BC, // TechSmith Blue
    0x6366F1, // Indigo
    0xA855F7, // Purple
    0xEC4899, // Pink
    0x111827, // Near Black
    0x6B7280, // Slate Gray
    0xD1D5DB, // Light Gray
    0xFFFFFF, // White
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_mode_toggle_and_tokens() {
        assert_eq!(ThemeMode::Light.toggled(), ThemeMode::Dark);
        assert_eq!(ThemeMode::Dark.toggled(), ThemeMode::Light);

        let light = SnagTheme::light();
        let dark = SnagTheme::dark();
        assert_eq!(light.mode, ThemeMode::Light);
        assert_eq!(dark.mode, ThemeMode::Dark);
        assert_ne!(light.app_bg, dark.app_bg);
        assert_ne!(light.text_primary, dark.text_primary);
        assert_eq!(DEFAULT_SWATCHES.len(), 12);
    }
}
