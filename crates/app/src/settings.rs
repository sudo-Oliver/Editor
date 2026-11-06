use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Settings of the user, unlike `.editor.toml`, which belongs to one project.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct Settings {
    pub vim_mode: bool,
    pub smooth_cursor: bool,
    /// The PDF follows the editor cursor.
    pub auto_scroll_pdf: bool,
    /// The popup with commands and project names while typing.
    pub auto_suggestions: bool,
    /// Underlines missing and unused LaTeX packages before a compiler complains.
    pub package_hints: bool,
    /// The Neovim colorscheme, or none for the built-in theme.
    pub theme: Option<String>,
    /// The path and the heading of the cursor above the editor.
    pub show_breadcrumbs: bool,
    pub show_tabs: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            vim_mode: false,
            smooth_cursor: true,
            auto_scroll_pdf: false,
            auto_suggestions: true,
            package_hints: true,
            theme: None,
            show_breadcrumbs: true,
            show_tabs: true,
        }
    }
}

impl Settings {
    /// A missing or broken file means the defaults, so that the app always starts.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        crate::atomic::write(path, text.as_bytes())
    }
}

pub fn default_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("editor").join("settings.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vim_mode_is_off_without_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings::load(&directory.path().join("settings.toml"));
        assert!(!settings.vim_mode);
    }

    #[test]
    fn vim_mode_is_off_with_a_broken_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "vim_mode = maybe").unwrap();
        assert!(!Settings::load(&path).vim_mode);
    }

    #[test]
    fn settings_survive_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("settings.toml");
        let settings = Settings { vim_mode: true, smooth_cursor: false, auto_scroll_pdf: true, auto_suggestions: false, package_hints: false, theme: Some("vesper".to_string()), show_breadcrumbs: false, show_tabs: false };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "vim_mode = true").unwrap();
        assert_eq!(Settings::load(&path), Settings { vim_mode: true, smooth_cursor: true, auto_scroll_pdf: false, auto_suggestions: true, package_hints: true, theme: None, show_breadcrumbs: true, show_tabs: true });
    }
}
