//! The content of the keyboard shortcuts dialog.

use crate::{
    actions::COMMANDS,
    keymap::{self, Keymap},
};

pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(String, &'static str)>,
}

fn plain(rows: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    rows.iter().map(|(keys, label)| (keys.to_string(), *label)).collect()
}

/// Groups of commands that the dialog lists; the keys that move the cursor
/// are the usual ones and stay on the shortcuts page of the settings.
const LISTED_GROUPS: &[&str] = &["General", "Editing", "View", "PDF", "Presentation"];

/// The shortcuts as `keymap` binds them. The vim keys only appear for those
/// who turned the mode on.
pub fn sections(vim_mode: bool, keymap: &Keymap) -> Vec<Section> {
    let os = keymap::host_os();
    let mut sections: Vec<Section> = LISTED_GROUPS
        .iter()
        .map(|group| Section {
            title: group,
            rows: COMMANDS
                .iter()
                .filter(|command| command.group == *group && command.available())
                .filter_map(|command| {
                    let keys = keymap.keys(command);
                    (!keys.is_empty()).then(|| (keys.iter().map(|keys| keymap::display(keys, os)).collect::<Vec<_>>().join(", "), command.label))
                })
                .collect(),
        })
        .collect();
    let click = if os == gpui::OperatingSystem::Mac { "⌘Click" } else { "Ctrl+Click" };
    if let Some(pdf) = sections.iter_mut().find(|section| section.title == "PDF") {
        pdf.rows.insert(0, (click.to_string(), "Jump from the PDF to the source"));
    }
    sections.insert(1, Section {
        title: "Editor tour (Help menu)",
        rows: plain(&[("→ or Return", "Next step"), ("←", "Previous step"), ("Esc", "End the tour")]),
    });
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
                    ("Space hp", "Project hub"),
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
        let has_vim = |on: bool| sections(on, &Keymap::default()).iter().any(|section| section.title.starts_with("Vim"));
        assert!(!has_vim(false));
        assert!(has_vim(true));
    }

    #[test]
    fn the_dialog_shows_the_user_keys() {
        let (keymap, _) = Keymap::parse(r#"{ "save": "ctrl-alt-s", "project_hub": null }"#);
        let general = sections(false, &keymap).into_iter().next().unwrap();
        let save = general.rows.iter().find(|(_, label)| *label == "Save").unwrap();
        assert_eq!(save.0, keymap::display("ctrl-alt-s", keymap::host_os()));
        assert!(!general.rows.iter().any(|(_, label)| *label == "Project hub"));
    }
}
