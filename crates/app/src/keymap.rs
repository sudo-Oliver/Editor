//! The user's own shortcuts, kept in `keymap.json` next to the settings. Each
//! entry replaces the default keys of one command:
//!
//! ```json
//! {
//!   "save": "secondary-s",
//!   "redo": ["secondary-shift-z", "secondary-y"],
//!   "project_hub": null
//! }
//! ```
//!
//! A string is one shortcut, a list several, and `null` none at all.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use gpui::{Keystroke, OperatingSystem};
use serde_json::Value;

use crate::actions::{COMMANDS, Command};

/// keymap.json names an add-on's command `addon.<add-on id>.<command id>`.
pub const ADDON_PREFIX: &str = "addon.";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keymap {
    /// By command id. An empty list unbinds the command.
    overrides: BTreeMap<String, Vec<String>>,
}

impl Keymap {
    /// A missing file means the defaults. Entries that cannot be read are left
    /// out and named in the problems, so that one typo keeps the rest.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut keymap = Self::default();
        let mut problems = Vec::new();
        let value: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => return (keymap, vec![format!("keymap.json is not valid JSON: {error}")]),
        };
        let Value::Object(entries) = value else {
            return (keymap, vec!["keymap.json must be an object of command ids and keys".to_string()]);
        };
        for (id, keys) in entries {
            // Add-on commands are known only once their add-on loads, so their
            // entries stay, and do nothing while it is off.
            if !COMMANDS.iter().any(|command| command.id == id) && !id.starts_with(ADDON_PREFIX) {
                problems.push(format!("{id}: no such command"));
                continue;
            }
            let keys = match keys {
                Value::Null => Vec::new(),
                Value::String(keys) => vec![keys],
                Value::Array(list) => {
                    let keys: Option<Vec<String>> = list.into_iter().map(|keys| keys.as_str().map(str::to_string)).collect();
                    let Some(keys) = keys else {
                        problems.push(format!("{id}: a list of keys holds only strings"));
                        continue;
                    };
                    keys
                }
                _ => {
                    problems.push(format!("{id}: keys are a string, a list of strings or null"));
                    continue;
                }
            };
            keymap.overrides.insert(id, keys.into_iter().map(|keys| normalize(&keys)).collect());
        }
        (keymap, problems)
    }

    pub fn load(path: &Path) -> (Self, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => (Self::default(), Vec::new()),
        }
    }

    pub fn to_json(&self) -> String {
        let entries: serde_json::Map<String, Value> = self
            .overrides
            .iter()
            .map(|(id, keys)| {
                let value = match keys.as_slice() {
                    [] => Value::Null,
                    [keys] => Value::String(keys.clone()),
                    keys => Value::Array(keys.iter().cloned().map(Value::String).collect()),
                };
                (id.clone(), value)
            })
            .collect();
        let mut text = serde_json::to_string_pretty(&Value::Object(entries)).expect("JSON of strings");
        text.push('\n');
        text
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::atomic::write(path, self.to_json().as_bytes())
    }

    /// The keys of `command`: the user's, or else the defaults.
    pub fn keys(&self, command: &Command) -> Vec<String> {
        match self.overrides.get(command.id) {
            Some(keys) => keys.clone(),
            None => command.keys.iter().map(|keys| keys.to_string()).collect(),
        }
    }

    /// The keys of an add-on's command, by its id in keymap.json.
    pub fn addon_keys(&self, id: &str, defaults: &[String]) -> Vec<String> {
        self.overrides.get(id).cloned().unwrap_or_else(|| defaults.to_vec())
    }

    pub fn is_changed(&self, command: &Command) -> bool {
        self.overrides.contains_key(command.id)
    }

    /// Gives `command` these keys. The defaults clear the entry.
    pub fn set(&mut self, command: &Command, keys: Vec<String>) {
        if keys.iter().map(String::as_str).eq(command.keys.iter().copied()) {
            self.overrides.remove(command.id);
        } else {
            self.overrides.insert(command.id.to_string(), keys);
        }
    }

    pub fn reset(&mut self, command: &Command) {
        self.overrides.remove(command.id);
    }

    /// The other commands that answer to `keys` where `command` does, since
    /// one of them would never run.
    pub fn conflicts(&self, command: &Command, keys: &str, os: OperatingSystem) -> Vec<&'static Command> {
        let wanted = resolve(keys, os);
        COMMANDS
            .iter()
            .filter(|other| other.id != command.id && other.context == command.context)
            .filter(|other| self.keys(other).iter().any(|keys| resolve(keys, os) == wanted))
            .collect()
    }

    /// The first shortcut of the command `id` as menus show it, such as `⇧⌘P`.
    pub fn label(&self, id: &str, os: OperatingSystem) -> String {
        COMMANDS
            .iter()
            .find(|command| command.id == id)
            .and_then(|command| self.keys(command).into_iter().next())
            .map_or_else(String::new, |keys| display(&keys, os))
    }
}

/// `secondary` spelled out, so that `secondary-s` and `cmd-s` count as the same.
fn resolve(keys: &str, os: OperatingSystem) -> Vec<String> {
    keys.split_whitespace()
        .map(|stroke| Keystroke::parse_with_os(stroke, os).map_or_else(|_| stroke.to_string(), |keystroke| keystroke.unparse(os)))
        .collect()
}

/// Shortcuts the way the platform writes them: `⇧⌘P` on macOS, `Ctrl+Shift+P` elsewhere.
pub fn display(keys: &str, os: OperatingSystem) -> String {
    keys.split_whitespace()
        .map(|stroke| Keystroke::parse_with_os(stroke, os).map_or_else(|_| stroke.to_string(), |keystroke| display_stroke(&keystroke, os)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_stroke(keystroke: &Keystroke, os: OperatingSystem) -> String {
    let modifiers = keystroke.modifiers;
    let mac = os == OperatingSystem::Mac;
    let key = keystroke.key.as_str();
    let named = match key {
        "enter" => if mac { "↩" } else { "Enter" },
        "backspace" => if mac { "⌫" } else { "Backspace" },
        "delete" => if mac { "⌦" } else { "Delete" },
        "escape" => if mac { "⎋" } else { "Esc" },
        "tab" => if mac { "⇥" } else { "Tab" },
        "up" => "↑",
        "down" => "↓",
        "left" => "←",
        "right" => "→",
        "space" => "Space",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "home" => "Home",
        "end" => "End",
        _ => "",
    };
    let key = if named.is_empty() { key.to_uppercase() } else { named.to_string() };
    if mac {
        // Apple's order.
        let mut text = String::new();
        for (on, symbol) in [(modifiers.control, "⌃"), (modifiers.alt, "⌥"), (modifiers.shift, "⇧"), (modifiers.platform, "⌘")] {
            if on {
                text.push_str(symbol);
            }
        }
        text + &key
    } else {
        let mut parts: Vec<&str> = Vec::new();
        for (on, name) in [(modifiers.control, "Ctrl"), (modifiers.platform, "Super"), (modifiers.alt, "Alt"), (modifiers.shift, "Shift")] {
            if on {
                parts.push(name);
            }
        }
        parts.push(&key);
        parts.join("+")
    }
}

/// Keys in the case gpui reads: `Cmd-S` becomes `cmd-s`, while a lone
/// uppercase letter stays, since gpui reads it as that letter with shift.
fn normalize(keys: &str) -> String {
    keys.split_whitespace()
        .map(|stroke| match stroke.rsplit_once('-') {
            Some((modifiers, key)) if !key.is_empty() => format!("{}-{key}", modifiers.to_ascii_lowercase()),
            _ => stroke.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Keys from a keystroke the user pressed, in the spelling of `keymap.json`.
/// The platform modifier becomes `secondary` on the platform it stands for,
/// so that the file works on other systems too.
pub fn record(keystroke: &Keystroke, os: OperatingSystem) -> String {
    let mut keystroke = keystroke.clone();
    let mac = os == OperatingSystem::Mac;
    let secondary = if mac { keystroke.modifiers.platform } else { keystroke.modifiers.control };
    if secondary {
        if mac {
            keystroke.modifiers.platform = false;
        } else {
            keystroke.modifiers.control = false;
        }
    }
    let rest = keystroke.unparse(os);
    if secondary { format!("secondary-{rest}") } else { rest }
}

/// The system the app runs on, for labels outside of a window.
pub fn host_os() -> OperatingSystem {
    if cfg!(target_os = "macos") {
        OperatingSystem::Mac
    } else if cfg!(target_os = "windows") {
        OperatingSystem::Windows
    } else {
        OperatingSystem::Linux
    }
}

pub fn default_path() -> Option<PathBuf> {
    Some(crate::settings::config_dir()?.join("keymap.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC_BUILD: bool = cfg!(target_os = "macos");

    fn command(id: &str) -> &'static Command {
        COMMANDS.iter().find(|command| command.id == id).unwrap()
    }

    #[test]
    fn command_ids_are_unique() {
        let mut ids: Vec<&str> = COMMANDS.iter().map(|command| command.id).collect();
        ids.sort();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn entries_replace_the_defaults() {
        let (keymap, problems) = Keymap::parse(r#"{ "save": "Cmd-Shift-S", "redo": ["secondary-y"], "project_hub": null }"#);
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(keymap.keys(command("save")), ["cmd-shift-S"]);
        assert_eq!(keymap.keys(command("redo")), ["secondary-y"]);
        assert_eq!(keymap.keys(command("project_hub")), Vec::<String>::new());
        assert_eq!(keymap.keys(command("bold")), ["secondary-b"]);
    }

    #[test]
    fn broken_entries_keep_the_rest() {
        let (keymap, problems) = Keymap::parse(r#"{ "nothing": "a", "save": 3, "bold": ["secondary-k", 1], "italic": "secondary-u" }"#);
        assert_eq!(problems.len(), 3);
        assert_eq!(keymap.keys(command("italic")), ["secondary-u"]);
        assert_eq!(keymap.keys(command("save")), ["secondary-s"]);
        assert_eq!(Keymap::parse("{").1.len(), 1);
    }

    #[test]
    fn the_file_survives_a_round_trip() {
        let mut keymap = Keymap::default();
        keymap.set(command("save"), vec!["secondary-shift-s".to_string()]);
        keymap.set(command("project_hub"), Vec::new());
        keymap.set(command("redo"), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(Keymap::parse(&keymap.to_json()), (keymap.clone(), Vec::new()));
        // Setting the defaults again leaves no entry.
        keymap.set(command("save"), vec!["secondary-s".to_string()]);
        assert!(!keymap.is_changed(command("save")));
    }

    #[test]
    fn conflicts_are_found_in_the_same_context() {
        let mut keymap = Keymap::default();
        keymap.set(command("save"), vec!["cmd-b".to_string()]);
        let os = OperatingSystem::Mac;
        assert!(keymap.conflicts(command("save"), "cmd-b", os).is_empty(), "bold is in the editor only");
        let found = keymap.conflicts(command("open_folder"), "cmd-b", os);
        assert_eq!(found.iter().map(|command| command.id).collect::<Vec<_>>(), ["save"]);
        assert_eq!(keymap.conflicts(command("bold"), "secondary-i", os).iter().map(|command| command.id).collect::<Vec<_>>(), ["italic"]);
    }

    #[test]
    fn pressed_keys_use_secondary() {
        let keystroke = Keystroke::parse("cmd-shift-p").unwrap();
        assert_eq!(record(&keystroke, OperatingSystem::Mac), "secondary-shift-p");
        let keystroke = Keystroke::parse("ctrl-k").unwrap();
        assert_eq!(record(&keystroke, OperatingSystem::Mac), "ctrl-k");
        assert_eq!(record(&keystroke, OperatingSystem::Linux), "secondary-k");
    }

    #[test]
    fn labels_follow_the_platform() {
        let keymap = Keymap::default();
        assert_eq!(keymap.label("project_hub", OperatingSystem::Mac), "⇧⌘P");
        assert_eq!(keymap.label("project_hub", OperatingSystem::Linux), "Ctrl+Shift+P");
        assert_eq!(keymap.label("recompile", OperatingSystem::Mac), "⌘↩");
        assert_eq!(keymap.label("layout_split", OperatingSystem::Mac), if MAC_BUILD { "⌃⌘↓" } else { "⌃⌥↓" });
        assert_eq!(keymap.label("start_tour", OperatingSystem::Mac), "");
    }
}
