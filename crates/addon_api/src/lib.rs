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
pub const FEATURES: &[&str] = &[
    "markdown", "tree_edits", "notify", "events", "pdf", "bib", "insert", "files", "review", "outline", "status", "commands", "drag", "documents", "selection", "synctex", "fixes", "build_control", "git", "packages", "references", "word_count", "pdf_search", "edit_groups", "more_events", "external_access",
];

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
    /// Says that the add-on passes what it may do on to programs outside the
    /// editor, such as Claude through a connector. It allows no request by
    /// itself; the user sees it before turning the add-on on.
    ExternalAccess,
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
            Self::ExternalAccess => "Lets programs outside the editor, such as Claude, use what this add-on can",
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
    /// Actions the user can run with keys or from menus. They exist only while
    /// the add-on is turned on, and running one starts it if needed.
    #[serde(default)]
    pub commands: Vec<AddonCommand>,
}

/// `[[commands]]` in `addon.toml`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddonCommand {
    /// Unique within the add-on, a-z, 0-9, - and _.
    pub id: String,
    pub label: String,
    /// Default keys, written like `secondary-shift-k`. The user can change them
    /// in keymap.json under `addon.<add-on id>.<command id>`.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Where else it shows: `editor` is the right-click menu of the editor.
    #[serde(default)]
    pub menus: Vec<CommandMenu>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandMenu {
    Editor,
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
    /// Its children can be dragged into a document. Text lands where it is
    /// dropped; a picture is copied into the project and placed as a figure.
    /// `HostMessage::Dropped` says where.
    Draggable { id: String, payload: DragPayload, children: Vec<Node> },
    /// A node from a newer editor, which this one cannot draw and skips.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DragPayload {
    Text { text: String },
    /// An absolute path, such as a picture the add-on made.
    File { path: String },
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
            Self::Scroll { id, .. } | Self::Draggable { id, .. } | Self::Button { id, .. } | Self::Input { id, .. } | Self::Select { id, .. } | Self::Dropdown { id, .. } | Self::Check { id, .. } | Self::Collapsible { id, .. } | Self::Chips { id, .. } => {
                Some(id)
            }
            Self::Alert { .. } | Self::Divider | Self::Image { .. } | Self::Unknown => None,
        }
    }

    pub fn children(&self) -> &[Node] {
        match self {
            Self::Column { children, .. }
            | Self::Row { children, .. }
            | Self::Scroll { children, .. }
            | Self::Message { children, .. }
            | Self::Collapsible { children, .. }
            | Self::Card { children, .. }
            | Self::Draggable { children, .. } => children,
            _ => &[],
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<Node>> {
        match self {
            Self::Column { children, .. }
            | Self::Row { children, .. }
            | Self::Scroll { children, .. }
            | Self::Message { children, .. }
            | Self::Collapsible { children, .. }
            | Self::Card { children, .. }
            | Self::Draggable { children, .. } => Some(children),
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
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
    /// Small facts in a line at the top of the panel, such as the model and
    /// how much of a limit is used, replacing the ones before. An item with
    /// options opens them, and choosing one sends `HostMessage::Select`; a
    /// plain item sends `HostMessage::Click`.
    SetStatus { items: Vec<StatusItem> },
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct StatusItem {
    pub id: String,
    pub text: String,
    /// Shown on hover, for what does not fit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tooltip: String,
    #[serde(default)]
    pub tone: Tone,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<DropdownOption>,
    #[serde(default)]
    pub selected: usize,
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
    /// `HostMessage::BuildStarted`.
    BuildStart,
    /// `HostMessage::DocumentChanged`, half a second after typing stops.
    Document,
    /// `HostMessage::ProjectOpened` and `HostMessage::ProjectClosed`.
    Project,
    /// `HostMessage::DiagnosticsChanged`.
    Diagnostics,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CitationEntry {
    pub key: String,
    /// Shown beside the key and searched with it.
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
    /// `group` names the group of `BeginEdits` that it belongs to.
    EditFile {
        path: String,
        old: String,
        new: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    /// Creates the file and opens it with this unsaved text.
    CreateFile {
        path: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    /// Replaces a whole file on disk, and its document if open. Fails when that
    /// document has unsaved changes.
    WriteFile { path: String, text: String },
    /// Saves all documents and compiles. The reply comes when the build ends.
    /// `clean` deletes what earlier builds left first, which is slower.
    Compile {
        #[serde(default, skip_serializing_if = "is_false")]
        clean: bool,
    },
    /// Stops the running build, whose `Compile` replies fail.
    CancelBuild,
    /// Starts a group of edits that `DiscardEdits` can take back. Each `group`
    /// is tracked on its own, so that several clients of one add-on, such as
    /// two sessions of a connector, do not start or discard each other's edits.
    /// Without a name, the group is the add-on's default one.
    BeginEdits {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    /// Takes back the edits of the group since `BeginEdits`. In each file the
    /// text from before the group's first edit returns, so later edits to that
    /// file go with it, also those of other groups or the user.
    DiscardEdits {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
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
    /// The open documents, in the order of their tabs.
    OpenDocuments,
    /// Saves one open document. A file that is not open has nothing to save.
    SaveFile { path: String },
    /// Renames or moves a file or folder in the project. Open documents follow
    /// it with their unsaved text. The reply is the new path, as `Reply::File`.
    RenameFile { from: String, to: String },
    /// Moves a file or folder to the Trash, from where Finder can put it back.
    /// Fails while one of its documents has unsaved changes.
    DeleteFile { path: String },
    /// The cursor, selection and visible lines of the active document.
    Selection,
    /// Where a source line lies in the PDF, from SyncTeX. Lines start at 1.
    /// Fails with the reason when there is no SyncTeX data, such as for Typst.
    SourceToPdf { path: String, line: usize },
    /// The source line of a point on a page, from SyncTeX. Pages start at 1,
    /// and `x` and `y` are PDF points from the top left of the page.
    PdfToSource { page: usize, x: f32, y: f32 },
    /// Makes the change of a `FixInfo` from `Diagnostics` or `PackageHints`.
    ApplyFix { id: String },
    /// The branch of the project's Git repository and its changed files.
    GitStatus,
    /// The changes of a file, or of all changed files without `path`, as a
    /// unified diff without context lines.
    GitDiff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default)]
        against: DiffBase,
    },
    /// The last commits, newest first, 50 by default.
    GitLog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// Missing and unused LaTeX packages in the whole project.
    PackageHints,
    /// Labels and citation keys with the places that use them, and what is
    /// missing or unused.
    References,
    /// The words of the main document with what it includes, or of one file.
    WordCount {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// The bookmarks of the PDF.
    PdfOutline,
    /// Where text occurs in the PDF, ignoring case. At most 200 hits.
    PdfSearch { query: String },
    /// The text selected in the PDF view, as `Reply::Text`, empty for none.
    PdfSelection,
}

/// What `Request::GitDiff` compares the text of a document with.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffBase {
    /// The last commit, with unsaved changes counting.
    #[default]
    Head,
    /// The file on disk, so only unsaved changes show.
    Working,
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
            | Self::Outline
            | Self::OpenDocuments
            | Self::Selection
            | Self::SourceToPdf { .. }
            | Self::PdfToSource { .. }
            | Self::GitStatus
            | Self::GitDiff { .. }
            | Self::GitLog { .. }
            | Self::PackageHints
            | Self::References
            | Self::WordCount { .. }
            | Self::PdfOutline
            | Self::PdfSearch { .. }
            | Self::PdfSelection => Permission::ReadProject,
            Self::EditFile { .. }
            | Self::CreateFile { .. }
            | Self::WriteFile { .. }
            | Self::BeginEdits { .. }
            | Self::DiscardEdits { .. }
            | Self::InsertText { .. }
            | Self::InsertAt { .. }
            | Self::CopyIntoProject { .. }
            | Self::ProposeEdits { .. }
            | Self::CancelReview { .. }
            | Self::RenameFile { .. }
            | Self::DeleteFile { .. }
            | Self::ApplyFix { .. } => Permission::EditDocuments,
            Self::Compile { .. } | Self::CancelBuild | Self::SaveFile { .. } => Permission::Compile,
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct DiagnosticInfo {
    pub path: Option<String>,
    pub line: Option<usize>,
    pub error: bool,
    pub message: String,
    /// Starts at 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    /// `error` or `warning`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub severity: String,
    /// What the editor makes of `message`, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// Advice from the editor and the compiler, one per line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The source text TeX had read when it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixInfo>,
}

/// A change the editor can make by itself, through `Request::ApplyFix`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct FixInfo {
    /// Such as `add_package:amsmath`. It stays the same across builds.
    pub id: String,
    /// Such as `Add \usepackage{amsmath}`.
    pub label: String,
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
    /// As `.editor.toml` names it: `pdflatex`, `lualatex`, `tectonic`,
    /// `typst` or `pandoc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Whether a build runs now.
    #[serde(default)]
    pub building: bool,
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
    /// Such as `article`, in lowercase.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entry_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doi: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// The entry as written in the file, from `@` to its closing brace.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct OpenDocument {
    /// Relative to the project, or absolute for a file outside it.
    pub path: String,
    pub unsaved: bool,
    pub active: bool,
}

/// A place in a document. Lines and columns start at 1, and columns count
/// characters.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TextPoint {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SelectionInfo {
    pub path: String,
    /// The cursor, which is one end of the selection.
    pub cursor: TextPoint,
    /// Both the cursor when nothing is selected.
    pub start: TextPoint,
    pub end: TextPoint,
    pub text: String,
    /// The first and last line on screen, `None` before the document was drawn.
    #[serde(default)]
    pub visible: Option<LineRange>,
}

/// Lines from `first` to `last`, both included, from 1.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct LineRange {
    pub first: usize,
    pub last: usize,
}

/// A rectangle on a page of the PDF, in points from its top left.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct PdfRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PdfHit {
    /// Starts at 1.
    pub page: usize,
    /// One per line that the hit spans.
    pub rects: Vec<PdfRect>,
    /// The line around the hit.
    pub context: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PdfOutlineEntry {
    pub title: String,
    /// 0 for the top level.
    pub level: usize,
    /// Starts at 1.
    pub page: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GitStatusInfo {
    /// `None` on a detached HEAD.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<GitFileInfo>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GitFileInfo {
    /// Relative to the root of the repository, which can lie above the project.
    pub path: String,
    /// The earlier name of a renamed file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// The letter of the change in the index, such as `M` or `A`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<String>,
    /// The letter of the change in the working tree. `?` means untracked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unstaged: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub conflicted: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GitCommitInfo {
    pub hash: String,
    pub author: String,
    /// Such as `2 hours ago`.
    pub age: String,
    /// Such as `HEAD -> main` and `origin/main`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    pub subject: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct PackageHintInfo {
    /// Relative to the project.
    pub path: String,
    /// Starts at 1.
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub fix: FixInfo,
}

/// A place in a source file, relative to the project. Lines start at 1.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SourcePlace {
    pub path: String,
    pub line: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct LabelInfo {
    pub name: String,
    /// Where it is defined.
    pub path: String,
    pub line: usize,
    /// The `\ref` and its kin, or `@` in Typst, that point to it.
    pub uses: Vec<SourcePlace>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CitationInfo {
    pub key: String,
    /// Whether a bibliography file of the project has the key.
    pub defined: bool,
    pub uses: Vec<SourcePlace>,
}

/// A use of a name that nothing defines.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct MissingName {
    pub name: String,
    pub path: String,
    pub line: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ReferenceReport {
    /// By name.
    pub labels: Vec<LabelInfo>,
    /// Every key that the documents cite, by key.
    pub citations: Vec<CitationInfo>,
    pub undefined_refs: Vec<MissingName>,
    pub undefined_citations: Vec<MissingName>,
    /// Labels that nothing points to.
    pub unused_labels: Vec<String>,
    /// Bibliography keys that no document cites. Empty after `\nocite{*}`.
    pub unused_entries: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct WordCountInfo {
    /// The main document, or the file asked for, relative to the project.
    pub path: String,
    /// `body`, `headings` and `captions` together.
    pub total: usize,
    pub body: usize,
    pub headings: usize,
    pub captions: usize,
    /// Formulas, which the total leaves out.
    pub inline_math: usize,
    pub display_math: usize,
    pub sections: Vec<SectionWords>,
    /// Included files that could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct SectionWords {
    pub title: String,
    pub words: usize,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
    Documents { documents: Vec<OpenDocument> },
    /// `None` when no document is active.
    Selection { selection: Option<SelectionInfo> },
    /// Where a source line lies in the PDF. `page` starts at 1.
    PdfPlace { page: usize, rect: PdfRect },
    SourcePlace { place: SourcePlace },
    GitStatus { status: GitStatusInfo },
    Commits { commits: Vec<GitCommitInfo> },
    PackageHints { hints: Vec<PackageHintInfo> },
    References { references: ReferenceReport },
    WordCount { count: WordCountInfo },
    PdfOutline { entries: Vec<PdfOutlineEntry> },
    PdfHits { hits: Vec<PdfHit> },
}

/// Lines to the add-on.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
    /// A `Draggable` was dropped into a document, which put it at this line.
    Dropped { id: String, path: String, line: usize },
    /// The user ran a command of the add-on, with where the cursor was.
    Command { id: String, context: Option<ActiveFile> },
    /// Another document became active, or none.
    ActiveFileChanged { path: Option<String> },
    /// Where the cursor rests. Lines and columns start at 1.
    SelectionChanged { path: String, line: usize, column: usize, selected_chars: usize },
    BuildFinished { success: bool, errors: usize, warnings: usize },
    BuildStarted,
    /// The text of a document changed, by typing or otherwise. Comes half a
    /// second after the changes stop, once per document.
    DocumentChanged { path: String },
    /// The project the add-on works on. The program starts again for each
    /// project, so this comes once, right after it subscribes.
    ProjectOpened { root: String },
    /// The project closes, and stdin with it.
    ProjectClosed,
    /// A build brought new diagnostics, which `Request::Diagnostics` lists.
    DiagnosticsChanged { errors: usize, warnings: usize },
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
    fn commands_come_from_the_manifest() {
        let manifest: Manifest = toml::from_str(
            "id = \"a\"\nname = \"A\"\nversion = \"1\"\nauthor = \"B\"\napi = 2\nprogram = \"a\"\n[[commands]]\nid = \"ask\"\nlabel = \"Ask about the selection\"\nkeys = [\"secondary-shift-k\"]\nmenus = [\"editor\"]\n",
        )
        .unwrap();
        assert_eq!(manifest.commands, [AddonCommand { id: "ask".into(), label: "Ask about the selection".into(), keys: vec!["secondary-shift-k".into()], menus: vec![CommandMenu::Editor] }]);
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
        assert_eq!(Request::Compile { clean: false }.permission(), Permission::Compile);
        assert_eq!(Request::DeleteFile { path: "a.tex".to_string() }.permission(), Permission::EditDocuments);
        assert_eq!(Request::RenameFile { from: "a".to_string(), to: "b".to_string() }.permission(), Permission::EditDocuments);
        assert_eq!(Request::GitDiff { path: None, against: DiffBase::Head }.permission(), Permission::ReadProject);
        assert_eq!(Request::InsertCitation { key: "k".to_string() }.permission(), Permission::Citations);
        assert_eq!(Request::GetSecret { key: "k".to_string() }.permission(), Permission::Secrets);
    }

    /// Requests of an older add-on still read after their variants grew fields.
    #[test]
    fn grown_requests_read_as_before() {
        let compile: Request = serde_json::from_str(r#"{"request":"compile"}"#).unwrap();
        assert_eq!(compile, Request::Compile { clean: false });
        assert_eq!(serde_json::to_string(&compile).unwrap(), r#"{"request":"compile"}"#);
        let begin: Request = serde_json::from_str(r#"{"request":"begin_edits"}"#).unwrap();
        assert_eq!(begin, Request::BeginEdits { group: None });
        assert_eq!(serde_json::to_string(&begin).unwrap(), r#"{"request":"begin_edits"}"#);
        let edit: Request = serde_json::from_str(r#"{"request":"edit_file","path":"a.tex","old":"x","new":"y"}"#).unwrap();
        assert_eq!(edit, Request::EditFile { path: "a.tex".into(), old: "x".into(), new: "y".into(), group: None });
        let diagnostic: DiagnosticInfo = serde_json::from_str(r#"{"path":null,"line":null,"error":true,"message":"m"}"#).unwrap();
        assert_eq!(diagnostic, DiagnosticInfo { error: true, message: "m".into(), ..Default::default() });
    }

    /// The wire format of the connector requests, written out like version 2.
    #[test]
    fn connector_requests_read_as_written() {
        let cases: &[(&str, Request)] = &[
            (r#"{"request":"discard_edits","group":"s1"}"#, Request::DiscardEdits { group: Some("s1".into()) }),
            (r#"{"request":"compile","clean":true}"#, Request::Compile { clean: true }),
            (r#"{"request":"git_diff","path":"a.tex","against":"working"}"#, Request::GitDiff { path: Some("a.tex".into()), against: DiffBase::Working }),
            (r#"{"request":"pdf_to_source","page":2,"x":10.5,"y":20.0}"#, Request::PdfToSource { page: 2, x: 10.5, y: 20.0 }),
            (r#"{"request":"word_count"}"#, Request::WordCount { path: None }),
            (r#"{"request":"apply_fix","id":"add_package:amsmath"}"#, Request::ApplyFix { id: "add_package:amsmath".into() }),
        ];
        for (json, request) in cases {
            assert_eq!(&serde_json::from_str::<Request>(json).unwrap(), request, "{json}");
            assert_eq!(serde_json::to_string(request).unwrap(), *json);
        }
        let reply = Reply::PdfPlace { page: 1, rect: PdfRect { left: 1.0, top: 2.0, right: 3.0, bottom: 4.0 } };
        assert_eq!(serde_json::to_string(&reply).unwrap(), r#"{"reply":"pdf_place","page":1,"rect":{"left":1.0,"top":2.0,"right":3.0,"bottom":4.0}}"#);
        let event = HostMessage::DocumentChanged { path: "a.tex".into() };
        assert_eq!(serde_json::to_string(&event).unwrap(), r#"{"message":"document_changed","path":"a.tex"}"#);
        assert_eq!(serde_json::to_string(&EventKind::BuildStart).unwrap(), r#""build_start""#);
        assert_eq!(serde_json::to_string(&Permission::ExternalAccess).unwrap(), r#""external_access""#);
    }
}
