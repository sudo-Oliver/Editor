//! The protocol between the editor and an add-on.
//!
//! An add-on is a program of its own. It reads one JSON message per line from
//! stdin and writes one per line to stdout. It describes its panel as a tree of
//! nodes, which the editor draws. When stdin closes, the add-on should exit.

use serde::{Deserialize, Serialize};

pub const API_VERSION: u32 = 1;

/// What an add-on may ask the editor for. Add-ons are not sandboxed, so this
/// limits the requests below and not the program itself.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ReadProject,
    EditDocuments,
    Compile,
    Secrets,
}

impl Permission {
    pub fn description(self) -> &'static str {
        match self {
            Self::ReadProject => "Read project files",
            Self::EditDocuments => "Change documents",
            Self::Compile => "Save and compile",
            Self::Secrets => "Keep secrets in the system keychain",
        }
    }
}

/// `addon.toml`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub api: u32,
    /// Relative to the folder of the add-on.
    pub program: String,
    /// An SVG file, relative to the folder of the add-on.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Column { children: Vec<Node> },
    Row { children: Vec<Node> },
    /// A column that scrolls and fills the free height. New text keeps its end in view.
    Scroll { id: String, children: Vec<Node> },
    /// `id` lets `AddonMessage::Append` add to the text later.
    Text {
        #[serde(default)]
        id: Option<String>,
        text: String,
        #[serde(default)]
        muted: bool,
    },
    Code {
        #[serde(default)]
        id: Option<String>,
        text: String,
    },
    Button {
        id: String,
        label: String,
        #[serde(default)]
        primary: bool,
        #[serde(default = "enabled")]
        enabled: bool,
        /// Sends the text of this input with the click and empties it.
        #[serde(default)]
        submits: Option<String>,
    },
    Input {
        id: String,
        #[serde(default)]
        placeholder: String,
    },
    /// All options stay visible, as in the settings of the editor.
    Select {
        id: String,
        options: Vec<String>,
        selected: usize,
    },
    Divider,
}

fn enabled() -> bool {
    true
}

impl Default for Node {
    fn default() -> Self {
        Self::Column { children: Vec::new() }
    }
}

/// Lines from the add-on.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum AddonMessage {
    /// Replaces the panel.
    SetView { view: Node },
    /// Adds text to the `Text` or `Code` node with this id.
    Append { id: String, text: String },
    /// The editor answers with `HostMessage::Reply` and the same `id`.
    Request { id: u64, request: Request },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// The files of the project without those that Git ignores, relative to it.
    ListFiles,
    /// An open document with its unsaved changes, else the file on disk.
    ReadFile { path: String },
    Search { query: String },
    Diagnostics,
    ActiveFile,
    /// Replaces `old`, which must occur exactly once, in the document. Saves nothing.
    EditFile { path: String, old: String, new: String },
    /// Creates the file and opens it with this unsaved text.
    CreateFile { path: String, text: String },
    /// Saves all documents and compiles. The reply comes when the build ends.
    Compile,
    /// Starts a group of edits that `DiscardEdits` can take back.
    BeginEdits,
    /// Takes back the edits since `BeginEdits`.
    DiscardEdits,
    GetSecret { key: String },
    SetSecret { key: String, value: String },
}

impl Request {
    pub fn permission(&self) -> Permission {
        match self {
            Self::ListFiles | Self::ReadFile { .. } | Self::Search { .. } | Self::Diagnostics | Self::ActiveFile => {
                Permission::ReadProject
            }
            Self::EditFile { .. } | Self::CreateFile { .. } | Self::BeginEdits | Self::DiscardEdits => {
                Permission::EditDocuments
            }
            Self::Compile => Permission::Compile,
            Self::GetSecret { .. } | Self::SetSecret { .. } => Permission::Secrets,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub path: String,
    /// Starts at 1.
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct DiagnosticInfo {
    pub path: Option<String>,
    pub line: Option<usize>,
    pub error: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ActiveFile {
    pub path: String,
    /// Starts at 1.
    pub line: usize,
    pub selection: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Done,
    Files { files: Vec<String> },
    Text { text: String },
    Hits { hits: Vec<SearchHit> },
    Diagnostics { diagnostics: Vec<DiagnosticInfo> },
    Active { file: Option<ActiveFile> },
    Compiled { success: bool, diagnostics: Vec<DiagnosticInfo> },
    Secret { value: Option<String> },
}

/// Lines to the add-on.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum HostMessage {
    /// The first message. Without a project, requests about files fail.
    Init { api: u32, project: Option<String> },
    Reply { id: u64, result: Result<Reply, String> },
    Click { id: String },
    /// A button with `submits` was clicked.
    Submit { id: String, text: String },
    Select { id: String, selected: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_reads_with_defaults() {
        let message: AddonMessage = serde_json::from_str(
            r#"{"message":"set_view","view":{"type":"column","children":[
                {"type":"text","text":"a"},
                {"type":"button","id":"b","label":"Go"}
            ]}}"#,
        )
        .unwrap();
        let AddonMessage::SetView { view: Node::Column { children } } = message else {
            panic!("not a column");
        };
        assert_eq!(children[0], Node::Text { id: None, text: "a".to_string(), muted: false });
        assert_eq!(
            children[1],
            Node::Button { id: "b".to_string(), label: "Go".to_string(), primary: false, enabled: true, submits: None }
        );
    }

    #[test]
    fn messages_stay_on_one_line() {
        let message = HostMessage::Submit { id: "a".to_string(), text: "line\nbreak".to_string() };
        let line = serde_json::to_string(&message).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<HostMessage>(&line).unwrap(), message);
    }

    #[test]
    fn requests_name_their_permission() {
        assert_eq!(Request::ListFiles.permission(), Permission::ReadProject);
        assert_eq!(Request::Compile.permission(), Permission::Compile);
        assert_eq!(Request::GetSecret { key: "k".to_string() }.permission(), Permission::Secrets);
    }
}
