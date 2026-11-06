//! The content of the keyboard shortcuts dialog.

#[cfg(target_os = "macos")]
const MODIFIER: &str = "⌘";
#[cfg(not(target_os = "macos"))]
const MODIFIER: &str = "Ctrl+";

pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(String, &'static str)>,
}

fn modified(key: &str) -> String {
    format!("{MODIFIER}{key}")
}

fn plain(rows: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    rows.iter().map(|(keys, label)| (keys.to_string(), *label)).collect()
}

/// The vim keys only appear for those who turned the mode on.
pub fn sections(vim_mode: bool) -> Vec<Section> {
    let mut sections = vec![
        Section {
            title: "General",
            rows: vec![
                (modified("S"), "Save"),
                (modified("Return"), "Recompile"),
                (modified("N"), "New file"),
                (modified("O"), "Open folder"),
                (modified("W"), "Close tab"),
                (modified("1 to 9"), "Go to tab"),
                (modified(","), "Settings"),
            ],
        },
        Section {
            title: "Editing",
            rows: vec![
                (modified("Z"), "Undo"),
                (modified("Shift+Z"), "Redo"),
                (modified("B"), "Bold"),
                (modified("I"), "Italic"),
                (modified("D"), "Select next occurrence"),
                (modified("Shift+C"), "Insert citation"),
                (modified("Shift+D"), "Cite from a DOI"),
                (modified("."), "Apply the package fix on this line"),
            ],
        },
        Section {
            title: "PDF",
            rows: vec![
                (modified("Click"), "Jump from the PDF to the source"),
                (modified("C"), "Copy the selected PDF text"),
                (modified("["), "Back after following a link"),
                (modified("]"), "Forward again"),
            ],
        },
    ];
    if vim_mode {
        sections.extend([
            Section {
                title: "Vim: project",
                rows: plain(&[
                    ("Space w", "Save"),
                    ("Space x", "Close tab"),
                    ("Space f", "Find file"),
                    ("Space g", "Find text"),
                    ("Space ee", "Toggle file tree"),
                    ("Space ef", "Show file in tree"),
                    ("Space tc", "Select theme"),
                    ("Space p", "Toggle PDF preview"),
                    ("Space d", "Show error"),
                    ("Space r", "Cite from a DOI"),
                    ("Space .", "Apply the package fix on this line"),
                    ("] d  [ d", "Next and previous error"),
                    ("Tab", "Switch between editor and tree"),
                    ("H  L", "Previous and next tab"),
                ]),
            },
            Section {
                title: "Vim: file tree",
                rows: plain(&[
                    ("j  k", "Move down and up"),
                    ("l  Enter", "Open"),
                    ("a", "New file"),
                    ("A", "New folder"),
                    ("u", "Add files from elsewhere"),
                    ("h", "Close folder"),
                    ("gg  G", "First and last row"),
                ]),
            },
            Section {
                title: "Vim: editing",
                rows: plain(&[
                    ("h j k l", "Move"),
                    ("w b e", "Words"),
                    ("0 ^ $", "Line start and end"),
                    ("gg G { } %", "Document, paragraph, bracket"),
                    ("f t F T ; ,", "Find on line"),
                    ("Ctrl+d  Ctrl+u", "Half a page"),
                    ("d c y > <", "Operators, with a motion or iw aw i( i{ i[ i\" i$"),
                    ("i a I A o O", "Insert"),
                    ("v V", "Visual and visual line"),
                    ("x r J p P", "Change text"),
                    ("u  Ctrl+r  .", "Undo, redo, repeat"),
                    ("/ ? n N *", "Search"),
                    (":w :q :wq :x :12", "Commands"),
                ]),
            },
        ]);
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vim_keys_only_appear_in_vim_mode() {
        let has_vim = |on: bool| sections(on).iter().any(|section| section.title.starts_with("Vim"));
        assert!(!has_vim(false));
        assert!(has_vim(true));
    }
}
