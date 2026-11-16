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
    /// The name others see in a live session. Empty means the user name of
    /// the computer.
    pub collab_name: String,
    /// Fetches now and then, and fast-forwards when nothing is in the way.
    pub git_background_sync: bool,
    /// Projects whose repository may run git by itself, since its config can
    /// start programs. Opening the git panel for a project adds it.
    pub trusted_git_roots: Vec<PathBuf>,
    /// Saves and compiles while typing, in projects whose `.editor.toml` does
    /// not choose.
    pub compile_while_typing: bool,
    /// The setup of the first start ran or was skipped.
    pub first_run_done: bool,
    /// The editor tour ran or was skipped, so it does not start by itself again.
    pub tour_done: bool,
    /// The hub shows projects as cards rather than rows.
    pub hub_cards: bool,
    /// Compilers come from Homebrew when it is installed, rather than into the
    /// app's own folder.
    pub use_homebrew: bool,
    /// Paper texture, ink-like text and a calm, monochrome editor. It sets the
    /// colors over `theme`, which stays for when it is off.
    pub paper_mode: bool,
    /// Lines under the text, like ruled paper. Only in paper mode.
    pub ruled_lines: bool,
    /// Dark paper with pale ink instead of parchment. Only in paper mode.
    pub paper_dark: bool,
    /// None means the default of the mode: iA Writer Quattro on paper, else Menlo.
    pub editor_font: Option<String>,
    /// None means the default of the mode: iA Writer Quattro on paper, else the system font.
    pub ui_font: Option<String>,
    /// In points. None means the default of the mode.
    pub editor_font_size: Option<u32>,
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
            collab_name: String::new(),
            git_background_sync: true,
            trusted_git_roots: Vec::new(),
            compile_while_typing: false,
            first_run_done: false,
            tour_done: false,
            hub_cards: true,
            use_homebrew: true,
            paper_mode: false,
            ruled_lines: false,
            paper_dark: false,
            editor_font: None,
            ui_font: None,
            editor_font_size: None,
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

    /// The colors and the type these settings ask for.
    pub fn look(&self) -> crate::theme::Look {
        crate::theme::Look::new(self.paper_mode, self.ruled_lines, self.editor_font.as_deref(), self.ui_font.as_deref(), self.editor_font_size.map(|size| size as f32))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        crate::atomic::write(path, text.as_bytes())
    }
}

/// Where the editor keeps what it can make again, such as the list of packages.
pub fn cache_dir() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(cache.join("editor"))
}

/// Where the editor keeps what belongs to the user and not to a project.
pub fn config_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("editor"))
}

pub fn default_path() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_files_load_without_paper_mode() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "vim_mode = true\ntheme = 'nvchad'\n").unwrap();
        let settings = Settings::load(&path);
        assert!(settings.vim_mode && !settings.paper_mode && settings.editor_font.is_none());
    }

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
        let settings = Settings { vim_mode: true, smooth_cursor: false, auto_scroll_pdf: true, auto_suggestions: false, package_hints: false, theme: Some("vesper".to_string()), show_breadcrumbs: false, show_tabs: false, collab_name: "Ada".to_string(), git_background_sync: false, trusted_git_roots: vec![PathBuf::from("/a")], compile_while_typing: true, first_run_done: true, tour_done: true, hub_cards: false, use_homebrew: false, paper_mode: true, ruled_lines: true, paper_dark: true, editor_font: Some("Fira Code".to_string()), ui_font: None, editor_font_size: Some(14) };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        std::fs::write(&path, "vim_mode = true").unwrap();
        assert_eq!(Settings::load(&path), Settings { vim_mode: true, smooth_cursor: true, auto_scroll_pdf: false, auto_suggestions: true, package_hints: true, theme: None, show_breadcrumbs: true, show_tabs: true, collab_name: String::new(), git_background_sync: true, trusted_git_roots: Vec::new(), compile_while_typing: false, first_run_done: false, tour_done: false, hub_cards: true, use_homebrew: true, paper_mode: false, ruled_lines: false, paper_dark: false, editor_font: None, ui_font: None, editor_font_size: None });
    }
}
