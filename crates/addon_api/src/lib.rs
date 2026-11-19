//! The protocol between the editor and an add-on.
//!
//! An add-on is a program of its own. It reads one JSON message per line from
//! stdin and writes one per line to stdout. It describes its panel as a tree of
//! nodes, which the editor draws. When stdin closes, the add-on should exit.

use serde::{Deserialize, Serialize};

pub const API_VERSION: u32 = 2;
/// The oldest version the editor still runs. Messages that came later only go
/// to add-ons that declare a version that has them.
pub const OLDEST_API_VERSION: u32 = 1;

/// What this editor can do beyond `API_VERSION`, sent with `HostMessage::Init`.
/// Add-ons check these rather than the version, so that they work on an
/// editor that lacks one by doing without it.
pub const FEATURES: &[&str] = &["markdown", "tree_edits", "notify", "events", "pdf", "bib", "insert", "files", "review", "outline"];

/// What an add-on may ask the editor for. Add-ons are not sandboxed, so this
/// limits the requests below and not the program itself.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ReadProject,
    EditDocuments,
    Compile,
    Secrets,
    Citations,
    LookupDoi,
}

impl Permission {
    pub fn description(self) -> &'static str {
        match self {
            Self::ReadProject => "Read project files",
            Self::EditDocuments => "Change documents",
            Self::Compile => "Save and compile",
            Self::Secrets => "Keep secrets in the system keychain",
            Self::Citations => "Offer citations",
            Self::LookupDoi => "Look up DOIs online",
        }
    }
}

/// When the program of an add-on starts.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    /// When its panel first shows.
    #[default]
    Panel,
    /// As soon as a project is open.
    Project,
}

/// `addon.toml`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    #[serde(default)]
    pub description: String,
    pub api: u32,
    #[serde(default)]
    pub activation: Activation,
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
    /// `id` lets `AddonMessage::Insert` add children later.
    Column {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        children: Vec<Node>,
    },
    Row {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        children: Vec<Node>,
    },
    /// A column that scrolls and fills the free height. New text keeps its end in view.
    Scroll {
        id: String,
        children: Vec<Node>,
        /// Turn this off for lists, which should stay where the user scrolled to.
        #[serde(default = "enabled")]
        follow: bool,
    },
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
        /// The text of a new input. An id that the editor knows keeps its text.
        #[serde(default)]
        value: String,
        /// Grows with its text up to a few lines, and Enter sends it with the
        /// button that submits it while Shift-Enter starts a new line.
        #[serde(default, skip_serializing_if = "is_false")]
        multiline: bool,
    },
    /// All options stay visible, as in the settings of the editor.
    Select {
        id: String,
        options: Vec<String>,
        selected: usize,
    },
    /// A button that shows the chosen option and opens a list of them. Choosing one
    /// sends `HostMessage::Select`, the same as `Select`.
    Dropdown {
        id: String,
        options: Vec<DropdownOption>,
        selected: usize,
    },
    /// A box that sends `HostMessage::Click` and waits for a new view to change.
    Check {
        id: String,
        label: String,
        checked: bool,
        /// Levels of a tree.
        #[serde(default)]
        indent: u32,
    },
    /// Text that must not be overlooked.
    Alert { text: String },
    Divider,
    /// Markdown: headings, paragraphs, bold, italic, inline code, code blocks,
    /// lists, quotes and links. `AddonMessage::Append` streams into it, and the
    /// editor only parses again what changed.
    Markdown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        text: String,
    },
    /// One message of a conversation, drawn by who wrote it.
    Message {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: Role,
        children: Vec<Node>,
    },
    /// A title that shows or hides its children. The editor opens and closes it
    /// by itself; `open` is how it starts.
    Collapsible {
        id: String,
        title: String,
        #[serde(default, skip_serializing_if = "is_false")]
        open: bool,
        children: Vec<Node>,
    },
    /// A framed group, such as a question that waits for an answer.
    Card {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tone: Tone,
        children: Vec<Node>,
    },
    /// Small labels in a row. Clicking the cross of a removable one sends
    /// `HostMessage::Click` with its id.
    Chips { id: String, items: Vec<Chip> },
    /// A picture from a file, as wide as the panel at most.
    Image {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_height: Option<u32>,
    },
    /// A node from a newer editor, which this one cannot draw and skips.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Info,
    Success,
    Warning,
    Danger,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Chip {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub removable: bool,
}

fn enabled() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !value
}

impl Default for Node {
    fn default() -> Self {
        Self::Column { id: None, children: Vec::new() }
    }
}

impl Node {
    /// The id that messages address this node by, if it has one.
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Column { id, .. } | Self::Row { id, .. } | Self::Text { id, .. } | Self::Code { id, .. } | Self::Markdown { id, .. } | Self::Message { id, .. } | Self::Card { id, .. } => {
                id.as_deref()
            }
            Self::Scroll { id, .. } | Self::Button { id, .. } | Self::Input { id, .. } | Self::Select { id, .. } | Self::Dropdown { id, .. } | Self::Check { id, .. } | Self::Collapsible { id, .. } | Self::Chips { id, .. } => {
                Some(id)
            }
            Self::Alert { .. } | Self::Divider | Self::Image { .. } | Self::Unknown => None,
        }
    }

    pub fn children(&self) -> &[Node] {
        match self {
            Self::Column { children, .. } | Self::Row { children, .. } | Self::Scroll { children, .. } | Self::Message { children, .. } | Self::Collapsible { children, .. } | Self::Card { children, .. } => {
                children
            }
            _ => &[],
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<Node>> {
        match self {
            Self::Column { children, .. } | Self::Row { children, .. } | Self::Scroll { children, .. } | Self::Message { children, .. } | Self::Collapsible { children, .. } | Self::Card { children, .. } => {
                Some(children)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct DropdownOption {
    pub label: String,
    /// A second, muted line in the open list.
    #[serde(default)]
    pub description: String,
}

/// Lines from the add-on.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum AddonMessage {
    /// Replaces the panel.
    SetView { view: Node },
    /// Adds text to the `Text`, `Code` or `Markdown` node with this id.
    Append { id: String, text: String },
    /// Adds `node` to the children of the node `parent`, at `index` or at the end.
    Insert {
        parent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        node: Node,
    },
    /// Replaces the node with this id.
    Replace { id: String, node: Node },
    /// Removes the node with this id.
    Remove { id: String },
    /// A short note in the editor's status line, such as "Saved figure".
    Notify {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    /// Which events the add-on wants, replacing an earlier choice. The editor
    /// does the work for an event only while someone wants it.
    Subscribe { events: Vec<EventKind> },
    /// The editor answers with `HostMessage::Reply` and the same `id`.
    Request { id: u64, request: Request },
    /// Replaces what this add-on offers in the citation picker.
    SetCitations { entries: Vec<CitationEntry> },
    /// Shows this view above the editor, or closes it. `Enter` acts like the first
    /// enabled primary button and `Esc` closes it, which `HostMessage::DialogClosed`
    /// reports. Needs `Permission::LookupDoi`.
    SetDialog { view: Option<Node> },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// `HostMessage::ActiveFileChanged`.
    ActiveFile,
    /// `HostMessage::SelectionChanged`, a quarter second after the cursor stops.
    Selection,
    /// `HostMessage::Saved`, for any activation.
    Saved,
    /// `HostMessage::BuildFinished`.
    Build,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CitationEntry {
    pub key: String,
    /// Shown beside the key and searched with it.
    pub detail: String,
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
    /// Replaces a whole file on disk, and its document if open. Fails when that
    /// document has unsaved changes.
    WriteFile { path: String, text: String },
    /// Saves all documents and compiles. The reply comes when the build ends.
    Compile,
    /// Starts a group of edits that `DiscardEdits` can take back.
    BeginEdits,
    /// Takes back the edits since `BeginEdits`.
    DiscardEdits,
    GetSecret { key: String },
    SetSecret { key: String, value: String },
    /// Writes a citation of this key at the cursor, in the form of the document.
    /// Elsewhere the key goes to the clipboard. The reply says which, as text.
    InsertCitation { key: String },
    /// Opens the file and puts the cursor there. Lines and columns start at 1.
    Reveal {
        path: String,
        #[serde(default)]
        line: Option<usize>,
        #[serde(default)]
        column: Option<usize>,
    },
    /// The root, the main file and its language, and whether its compiler is there.
    ProjectInfo,
    /// The end of the last build's output.
    BuildLog {
        #[serde(default)]
        max_bytes: Option<usize>,
    },
    /// The pages of the PDF and the one in view.
    PdfInfo,
    /// The text of a page of the PDF, the one in view by default. Pages start at 1.
    PdfText {
        #[serde(default)]
        page: Option<usize>,
    },
    /// A page of the PDF as a PNG file, which the reply names. It lies in a
    /// folder only this user can read and goes when the editor quits.
    PdfPage {
        #[serde(default)]
        page: Option<usize>,
        #[serde(default)]
        width: Option<u32>,
    },
    /// The entries of the project's bibliography files.
    BibEntries,
    /// A map of the whole project for finding one's way in it without reading
    /// it all: each source file with its headings, figures, tables, labels and
    /// words, open documents with their unsaved text. Small even for a book.
    Outline,
    /// Puts text at the cursor of the open document, in place of the selection.
    InsertText { text: String },
    /// Puts text at a place in a file, which opens if it is not. Lines and
    /// columns start at 1, and a column past the end means the end of the line.
    InsertAt { path: String, line: usize, column: usize, text: String },
    /// Copies a file from anywhere, such as a picture the user dropped, into the
    /// project. `dest` is relative to it; an existing file gets a number. The
    /// reply is the path it got, as text.
    CopyIntoProject { source: String, dest: String },
    /// Shows changes to a file for the user to accept or reject, block by
    /// block, before anything changes. Either `edits`, each `old` occurring
    /// once, or the whole new `text`. The reply comes when every block is
    /// decided, or the review ends early.
    ProposeEdits {
        path: String,
        /// What the change is for, shown above it.
        #[serde(default)]
        title: String,
        #[serde(default)]
        edits: Vec<TextEdit>,
        #[serde(default)]
        text: Option<String>,
    },
    /// Ends an open review of the file without changing it.
    CancelReview { path: String },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TextEdit {
    pub old: String,
    pub new: String,
}

impl Request {
    pub fn permission(&self) -> Permission {
        match self {
            Self::ListFiles
            | Self::ReadFile { .. }
            | Self::Search { .. }
            | Self::Diagnostics
            | Self::ActiveFile
            | Self::Reveal { .. }
            | Self::ProjectInfo
            | Self::BuildLog { .. }
            | Self::PdfInfo
            | Self::PdfText { .. }
            | Self::PdfPage { .. }
            | Self::BibEntries
            | Self::Outline => Permission::ReadProject,
            Self::EditFile { .. }
            | Self::CreateFile { .. }
            | Self::WriteFile { .. }
            | Self::BeginEdits
            | Self::DiscardEdits
            | Self::InsertText { .. }
            | Self::InsertAt { .. }
            | Self::CopyIntoProject { .. }
            | Self::ProposeEdits { .. }
            | Self::CancelReview { .. } => Permission::EditDocuments,
            Self::Compile => Permission::Compile,
            Self::GetSecret { .. } | Self::SetSecret { .. } => Permission::Secrets,
            Self::InsertCitation { .. } => Permission::Citations,
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
pub struct ProjectInfo {
    pub root: String,
    /// Relative to the root.
    pub main: Option<String>,
    /// `latex`, `typst` or `markdown`.
    pub language: Option<String>,
    /// The program that compiles it, such as `pdflatex` or `typst`.
    pub compiler: Option<String>,
    pub compiler_ready: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct BibInfo {
    pub key: String,
    pub authors: String,
    pub year: String,
    pub title: String,
    pub venue: String,
    /// The file it is in, relative to the project.
    pub file: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct FileOutline {
    /// Relative to the project.
    pub path: String,
    pub words: usize,
    pub lines: usize,
    pub entries: Vec<OutlineEntry>,
    /// `\label` names, which `\ref` points to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct OutlineEntry {
    /// `section`, `figure`, `table` or `references`.
    pub kind: String,
    /// 1 for a part or chapter in LaTeX, a `=` heading in Typst, and so on down.
    pub level: usize,
    pub title: String,
    /// Starts at 1.
    pub line: usize,
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
    Project { project: ProjectInfo },
    /// `page` is the one in view, from 1.
    Pdf { pages: usize, page: usize },
    File { path: String },
    Bib { entries: Vec<BibInfo> },
    /// The main file first, then the others by path.
    Outline { main: Option<String>, files: Vec<FileOutline> },
    /// The blocks of a review by their order in the file, from 0.
    Reviewed { accepted: Vec<usize>, rejected: Vec<usize> },
}

/// Lines to the add-on.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum HostMessage {
    /// The first message. Without a project, requests about files fail.
    Init {
        api: u32,
        project: Option<String>,
        /// The version of the editor, for messages to the user.
        #[serde(default)]
        host_version: String,
        /// See `FEATURES`.
        #[serde(default)]
        features: Vec<String>,
    },
    Reply { id: u64, result: Result<Reply, String> },
    Click { id: String },
    /// A button with `submits` was clicked.
    Submit { id: String, text: String },
    Select { id: String, selected: usize },
    /// A document was saved. Only for add-ons with `Activation::Project`.
    Saved { path: String },
    /// The user chose an entry that this add-on offered.
    CitationPicked { key: String },
    /// The user asked to look up a DOI. `suggestion` is the selection or clipboard
    /// if it looks like a DOI or arXiv id, else empty.
    LookupDoi { suggestion: String },
    /// The user closed the dialog.
    DialogClosed,
    /// Another document became active, or none.
    ActiveFileChanged { path: Option<String> },
    /// Where the cursor rests. Lines and columns start at 1.
    SelectionChanged { path: String, line: usize, column: usize, selected_chars: usize },
    BuildFinished { success: bool, errors: usize, warnings: usize },
    /// Files dropped on the panel. Absolute paths.
    FilesDropped { paths: Vec<String> },
    /// A picture pasted into an input of the panel, saved as a PNG file.
    ImagePasted { path: String },
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
        let AddonMessage::SetView { view: Node::Column { children, .. } } = message else {
            panic!("not a column");
        };
        assert_eq!(children[0], Node::Text { id: None, text: "a".to_string(), muted: false });
        assert_eq!(
            children[1],
            Node::Button { id: "b".to_string(), label: "Go".to_string(), primary: false, enabled: true, submits: None }
        );
    }

    #[test]
    fn manifest_defaults_to_starting_with_the_panel() {
        let manifest: Manifest =
            toml::from_str("id = \"a\"\nname = \"A\"\nversion = \"1\"\nauthor = \"B\"\napi = 1\nprogram = \"a\"\n").unwrap();
        assert_eq!(manifest.activation, Activation::Panel);
        assert_eq!(manifest.description, "");
    }

    #[test]
    fn scroll_follows_unless_told_otherwise() {
        let scroll: Node = serde_json::from_str(r#"{"type":"scroll","id":"s","children":[]}"#).unwrap();
        assert!(matches!(scroll, Node::Scroll { follow: true, .. }));
    }

    #[test]
    fn messages_stay_on_one_line() {
        let message = HostMessage::Submit { id: "a".to_string(), text: "line\nbreak".to_string() };
        let line = serde_json::to_string(&message).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<HostMessage>(&line).unwrap(), message);
    }

    #[test]
    fn a_node_from_a_newer_editor_is_skipped() {
        let view: Node = serde_json::from_str(r#"{"type":"column","children":[{"type":"hologram","depth":3},{"type":"divider"}]}"#).unwrap();
        assert_eq!(view.children(), [Node::Unknown, Node::Divider]);
    }

    /// The wire format of version 2, written out, so that a change to it shows
    /// in review and add-ons in other repositories can rely on it.
    #[test]
    fn version_two_reads_as_written() {
        let cases: &[(&str, AddonMessage)] = &[
            (
                r#"{"message":"insert","parent":"chat","node":{"type":"message","id":"m1","role":"assistant","children":[{"type":"markdown","id":"t1","text":"**Hi**"}]}}"#,
                AddonMessage::Insert {
                    parent: "chat".into(),
                    index: None,
                    node: Node::Message {
                        id: Some("m1".into()),
                        role: Role::Assistant,
                        children: vec![Node::Markdown { id: Some("t1".into()), text: "**Hi**".into() }],
                    },
                },
            ),
            (r#"{"message":"replace","id":"m1","node":{"type":"divider"}}"#, AddonMessage::Replace { id: "m1".into(), node: Node::Divider }),
            (r#"{"message":"remove","id":"m1"}"#, AddonMessage::Remove { id: "m1".into() }),
            (r#"{"message":"notify","text":"Saved","tone":"success"}"#, AddonMessage::Notify { text: "Saved".into(), tone: Tone::Success }),
            (
                r#"{"message":"set_view","view":{"type":"card","tone":"warning","children":[{"type":"chips","id":"c","items":[{"id":"x","label":"main.tex","removable":true}]},{"type":"collapsible","id":"k","title":"Read main.tex","children":[]},{"type":"image","path":"/tmp/a.png","max_height":200}]}}"#,
                AddonMessage::SetView {
                    view: Node::Card {
                        id: None,
                        tone: Tone::Warning,
                        children: vec![
                            Node::Chips { id: "c".into(), items: vec![Chip { id: "x".into(), label: "main.tex".into(), removable: true }] },
                            Node::Collapsible { id: "k".into(), title: "Read main.tex".into(), open: false, children: vec![] },
                            Node::Image { path: "/tmp/a.png".into(), max_height: Some(200) },
                        ],
                    },
                },
            ),
        ];
        for (json, message) in cases {
            assert_eq!(&serde_json::from_str::<AddonMessage>(json).unwrap(), message, "{json}");
            assert_eq!(serde_json::to_string(message).unwrap(), *json);
        }
        let init: HostMessage = serde_json::from_str(r#"{"message":"init","api":1,"project":null}"#).unwrap();
        assert_eq!(init, HostMessage::Init { api: 1, project: None, host_version: String::new(), features: Vec::new() });
    }

    #[test]
    fn requests_name_their_permission() {
        assert_eq!(Request::ListFiles.permission(), Permission::ReadProject);
        assert_eq!(Request::Compile.permission(), Permission::Compile);
        assert_eq!(Request::InsertCitation { key: "k".to_string() }.permission(), Permission::Citations);
        assert_eq!(Request::GetSecret { key: "k".to_string() }.permission(), Permission::Secrets);
    }
}
