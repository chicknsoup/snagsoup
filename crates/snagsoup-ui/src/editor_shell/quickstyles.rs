//! QuickStyles Grid, "Ghost QuickStyle" auto-save, Favorites (`Favorites.json`),
//! and Tool Themes (`.snagtheme`) import/export/switcher (`W3-09` Part B).

use std::path::Path;

use snagsoup_model::{
    ColorRgba, FavoritesFile, QuickStyle, SnagTheme as ModelToolTheme, ToolMode,
};

/// Manages tool QuickStyles, Favorites persistence, and `.snagtheme` tool themes.
#[derive(Debug, Clone)]
pub struct QuickStyleManager {
    pub themes: Vec<ModelToolTheme>,
    pub active_theme_idx: usize,
    pub favorites: FavoritesFile,
}

impl Default for QuickStyleManager {
    fn default() -> Self {
        let basic_theme = ModelToolTheme {
            id: "snagsoup-basic".to_string(),
            name: "SnagSoup Basic".to_string(),
            version: 1.0,
            quick_styles: vec![
                QuickStyle {
                    tool_mode: Some(ToolMode::Arrow),
                    foreground_color: Some(ColorRgba::rgb(217, 43, 43)),
                    stroke_width: Some(6.0),
                    opacity: Some(100.0),
                    ..QuickStyle::default()
                },
                QuickStyle {
                    tool_mode: Some(ToolMode::Arrow),
                    foreground_color: Some(ColorRgba::rgb(27, 117, 188)),
                    stroke_width: Some(4.0),
                    opacity: Some(100.0),
                    ..QuickStyle::default()
                },
                QuickStyle {
                    tool_mode: Some(ToolMode::Callout),
                    foreground_color: Some(ColorRgba::rgb(31, 35, 40)),
                    background_color: Some(ColorRgba::rgb(250, 204, 21)),
                    stroke_width: Some(3.0),
                    opacity: Some(100.0),
                    ..QuickStyle::default()
                },
            ],
            extra: Default::default(),
        };

        Self {
            themes: vec![basic_theme],
            active_theme_idx: 0,
            favorites: FavoritesFile::default(),
        }
    }
}

impl QuickStyleManager {
    pub fn active_theme(&self) -> &ModelToolTheme {
        &self.themes[self.active_theme_idx.min(self.themes.len().saturating_sub(1))]
    }

    /// Return all QuickStyles in the active theme matching `tool`.
    pub fn styles_for_tool(&self, tool: &ToolMode) -> Vec<&QuickStyle> {
        self.active_theme()
            .quick_styles
            .iter()
            .filter(|s| s.tool_mode.as_ref() == Some(tool))
            .collect()
    }

    /// Returns `true` when the user's current `(tool, color, stroke_width)` differs
    /// from every existing QuickStyle in the active theme, triggering the "Ghost QuickStyle" `+` tile.
    pub fn should_show_ghost_style(
        &self,
        tool: &ToolMode,
        color: ColorRgba,
        stroke_width: f64,
    ) -> bool {
        !self.styles_for_tool(tool).iter().any(|s| {
            s.foreground_color == Some(color)
                && s.stroke_width
                    .map_or(false, |w| (w - stroke_width).abs() < 0.25)
        })
    }

    /// Save the Ghost QuickStyle into the active theme and return its index.
    pub fn save_ghost_style(
        &mut self,
        tool: ToolMode,
        color: ColorRgba,
        stroke_width: f64,
        opacity: f64,
    ) -> usize {
        let idx = self.active_theme_idx.min(self.themes.len().saturating_sub(1));
        let style = QuickStyle {
            tool_mode: Some(tool),
            foreground_color: Some(color),
            stroke_width: Some(stroke_width),
            opacity: Some(opacity),
            ..QuickStyle::default()
        };
        self.themes[idx].quick_styles.push(style);
        self.themes[idx].quick_styles.len() - 1
    }

    /// Star / unstar a QuickStyle in `Favorites`. Returns `true` if now favorited.
    pub fn toggle_favorite(&mut self, style: QuickStyle) -> bool {
        if let Some(pos) = self
            .favorites
            .quick_styles
            .iter()
            .position(|f| f == &style)
        {
            self.favorites.quick_styles.remove(pos);
            false
        } else {
            self.favorites.quick_styles.push(style);
            true
        }
    }

    /// Import a `.snagtheme` file and make it the active theme.
    pub fn import_snagtheme(&mut self, path: &Path) -> Result<usize, String> {
        let theme = ModelToolTheme::from_file(path)
            .map_err(|e| format!("failed to import .snagtheme: {e}"))?;
        self.themes.push(theme);
        self.active_theme_idx = self.themes.len() - 1;
        Ok(self.active_theme_idx)
    }

    /// Export the active theme to a `.snagtheme` JSON file.
    pub fn export_active_snagtheme(&self, path: &Path) -> Result<(), String> {
        self.active_theme()
            .to_file(path)
            .map_err(|e| format!("write .snagtheme failed: {e}"))
    }

    /// Save Favorites to a `Favorites.json` file.
    pub fn save_favorites(&self, path: &Path) -> Result<(), String> {
        self.favorites
            .to_file(path)
            .map_err(|e| format!("write Favorites.json failed: {e}"))
    }

    /// Load Favorites from a `Favorites.json` file.
    pub fn load_favorites(&mut self, path: &Path) -> Result<usize, String> {
        let fav = FavoritesFile::from_file(path)
            .map_err(|e| format!("load Favorites.json failed: {e}"))?;
        let n = fav.quick_styles.len();
        self.favorites = fav;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghost_quickstyle_favorites_and_snagtheme_roundtrip() {
        let mut mgr = QuickStyleManager::default();
        let custom_color = ColorRgba::rgb(168, 85, 247);

        // Modified properties trigger Ghost QuickStyle
        assert!(mgr.should_show_ghost_style(&ToolMode::Arrow, custom_color, 8.0));
        let idx = mgr.save_ghost_style(ToolMode::Arrow, custom_color, 8.0, 100.0);
        assert!(!mgr.should_show_ghost_style(&ToolMode::Arrow, custom_color, 8.0));

        // Toggle favorite & persist Favorites.json roundtrip
        let style = mgr.active_theme().quick_styles[idx].clone();
        assert!(mgr.toggle_favorite(style));
        let fav_path = std::env::temp_dir().join("snagsoup_w3_09_favorites.json");
        mgr.save_favorites(&fav_path).unwrap();

        let mut mgr2 = QuickStyleManager::default();
        assert_eq!(mgr2.load_favorites(&fav_path).unwrap(), 1);
        let _ = std::fs::remove_file(&fav_path);

        // Export & re-import .snagtheme
        let theme_path = std::env::temp_dir().join("snagsoup_w3_09_theme.snagtheme");
        mgr.export_active_snagtheme(&theme_path).unwrap();
        let imported_idx = mgr2.import_snagtheme(&theme_path).unwrap();
        assert_eq!(mgr2.themes[imported_idx].name, "SnagSoup Basic");
        let _ = std::fs::remove_file(&theme_path);
    }
}
