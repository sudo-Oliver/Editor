use gpui::{Action, App, KeyBinding, KeyBindingContextPredicate, actions};

use crate::keymap::{ADDON_PREFIX, Keymap};

/// Runs a command that an add-on declared in its manifest.
#[derive(Clone, Debug, PartialEq, Eq, Action)]
#[action(namespace = editor, no_json)]
pub struct RunAddonCommand {
    pub addon: String,
    pub command: String,
}

/// An add-on command with its default keys, for binding.
pub struct AddonBinding {
    pub addon: String,
    pub command: String,
    pub keys: Vec<String>,
}

actions!(
    editor,
    [
        Backspace,
        DeleteForward,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveWordLeft,
        MoveWordRight,
        MoveLineStart,
        MoveLineEnd,
        MoveDocumentStart,
        MoveDocumentEnd,
        PageUp,
        PageDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectLineStart,
        SelectLineEnd,
        SelectDocumentStart,
        SelectDocumentEnd,
        SelectAll,
        SelectNextOccurrence,
        AddCursorAbove,
        AddCursorBelow,
        CollapseSelections,
        Newline,
        Indent,
        AcceptCompletion,
        NextCompletion,
        PreviousCompletion,
        CloseCompletion,
        Outdent,
        Undo,
        Redo,
        Copy,
        Cut,
        Paste,
        Bold,
        Italic,
    ]
);

actions!(
    workspace,
    [
        NewFile,
        OpenFolder,
        Save,
        InsertCitation,
        LookupDoi,
        QuickFix,
        Recompile,
        CloseTab,
        GoToTab1,
        GoToTab2,
        GoToTab3,
        GoToTab4,
        GoToTab5,
        GoToTab6,
        GoToTab7,
        GoToTab8,
        GoToTab9,
        ToggleFileTree,
        TogglePreview,
        ToggleLogs,
        CloseMenu,
        OpenSettings,
        Quit,
        PdfBack,
        PdfForward,
        LayoutSplit,
        LayoutEditorOnly,
        LayoutPdfOnly,
        LayoutPdfTab,
        ToggleFocusMode,
        ToggleHub,
        CommandPalette,
        StartTour,
        OpenGitPanel,
        StartPresentation,
        ExitPresentation,
        PdfNextPage,
        PdfPreviousPage,
    ]
);

/// What a shortcut can do. The id names it in `keymap.json`, and `keys` are its
/// defaults, which may name `secondary`: `cmd` on macOS and `ctrl` elsewhere.
pub struct Command {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub context: Option<&'static str>,
    pub keys: &'static [&'static str],
    action: fn() -> Box<dyn Action>,
}

impl Command {
    /// Whether it makes sense to run from the palette, outside a special state
    /// such as an open completion list or a presentation.
    pub fn runs_anywhere(&self) -> bool {
        self.id != "close_menu" && self.id != "command_palette" && self.context.is_none_or(|context| context == "Editor")
    }

    pub fn action(&self) -> Box<dyn Action> {
        (self.action)()
    }
}

const MAC: bool = cfg!(target_os = "macos");
const EDITOR: Option<&str> = Some("Editor");
const PDF: Option<&str> = Some("PdfView");
const COMPLETION: Option<&str> = Some("Completion");
const PRESENTATION: Option<&str> = Some("Presentation");

macro_rules! command {
    ($id:literal, $group:literal, $label:literal, $action:ident, $context:expr, $keys:expr) => {
        Command { id: $id, group: $group, label: $label, context: $context, keys: $keys, action: || Box::new($action) }
    };
}

/// In the order gpui gets them: of two bindings for the same keys, the later wins.
pub const COMMANDS: &[Command] = &[
    // The backdrop of an open menu holds the focus, so this does not reach the editor.
    command!("close_menu", "General", "Close menu", CloseMenu, None, &["escape"]),
    command!("save", "General", "Save", Save, None, &["secondary-s"]),
    command!("recompile", "General", "Recompile", Recompile, None, &["secondary-enter"]),
    command!("new_file", "General", "New file", NewFile, None, &["secondary-n"]),
    command!("open_folder", "General", "Open folder", OpenFolder, None, &["secondary-o"]),
    command!("project_hub", "General", "Project hub", ToggleHub, None, &["secondary-shift-p"]),
    command!("command_palette", "General", "Command palette", CommandPalette, None, &["secondary-k"]),
    command!("git_panel", "General", "Git panel", OpenGitPanel, None, &["secondary-shift-g"]),
    command!("close_tab", "General", "Close tab", CloseTab, None, &["secondary-w"]),
    command!("tab_1", "General", "Go to tab 1", GoToTab1, None, &["secondary-1"]),
    command!("tab_2", "General", "Go to tab 2", GoToTab2, None, &["secondary-2"]),
    command!("tab_3", "General", "Go to tab 3", GoToTab3, None, &["secondary-3"]),
    command!("tab_4", "General", "Go to tab 4", GoToTab4, None, &["secondary-4"]),
    command!("tab_5", "General", "Go to tab 5", GoToTab5, None, &["secondary-5"]),
    command!("tab_6", "General", "Go to tab 6", GoToTab6, None, &["secondary-6"]),
    command!("tab_7", "General", "Go to tab 7", GoToTab7, None, &["secondary-7"]),
    command!("tab_8", "General", "Go to tab 8", GoToTab8, None, &["secondary-8"]),
    command!("tab_9", "General", "Go to tab 9", GoToTab9, None, &["secondary-9"]),
    command!("settings", "General", "Settings", OpenSettings, None, &["secondary-,"]),
    command!("quit", "General", "Quit", Quit, None, &["secondary-q"]),
    command!("start_tour", "General", "Editor tour", StartTour, None, &[]),
    command!("undo", "Editing", "Undo", Undo, EDITOR, &["secondary-z"]),
    command!("redo", "Editing", "Redo", Redo, EDITOR, &["secondary-shift-z", "secondary-y"]),
    command!("copy", "Editing", "Copy", Copy, EDITOR, &["secondary-c"]),
    command!("cut", "Editing", "Cut", Cut, EDITOR, &["secondary-x"]),
    command!("paste", "Editing", "Paste", Paste, EDITOR, &["secondary-v"]),
    command!("bold", "Editing", "Bold", Bold, EDITOR, &["secondary-b"]),
    command!("italic", "Editing", "Italic", Italic, EDITOR, &["secondary-i"]),
    command!("select_all", "Editing", "Select all", SelectAll, EDITOR, &["secondary-a"]),
    command!("select_next_occurrence", "Editing", "Select next occurrence", SelectNextOccurrence, EDITOR, &["secondary-d"]),
    command!("add_cursor_above", "Editing", "Add cursor above", AddCursorAbove, EDITOR, &["secondary-alt-up"]),
    command!("add_cursor_below", "Editing", "Add cursor below", AddCursorBelow, EDITOR, &["secondary-alt-down"]),
    command!("insert_citation", "Editing", "Insert citation", InsertCitation, None, &["secondary-shift-c"]),
    command!("lookup_doi", "Editing", "Cite from a DOI", LookupDoi, None, &["secondary-shift-d"]),
    command!("quick_fix", "Editing", "Apply the package fix on this line", QuickFix, None, &["secondary-."]),
    command!("backspace", "Text", "Delete left", Backspace, EDITOR, &["backspace", "shift-backspace"]),
    command!("delete", "Text", "Delete right", DeleteForward, EDITOR, &["delete"]),
    command!("delete_word_left", "Text", "Delete word left", DeleteWordLeft, EDITOR, if MAC { &["alt-backspace"] } else { &["alt-backspace", "ctrl-backspace"] }),
    command!("delete_word_right", "Text", "Delete word right", DeleteWordRight, EDITOR, if MAC { &["alt-delete"] } else { &["alt-delete", "ctrl-delete"] }),
    command!("delete_to_line_start", "Text", "Delete to line start", DeleteToLineStart, EDITOR, if MAC { &["cmd-backspace"] } else { &[] }),
    command!("newline", "Text", "New line", Newline, EDITOR, &["enter"]),
    command!("indent", "Text", "Indent", Indent, EDITOR, &["tab"]),
    command!("outdent", "Text", "Outdent", Outdent, EDITOR, &["shift-tab"]),
    command!("collapse_selections", "Text", "Keep one cursor", CollapseSelections, EDITOR, &["escape"]),
    command!("move_left", "Cursor", "Left", MoveLeft, EDITOR, &["left"]),
    command!("move_right", "Cursor", "Right", MoveRight, EDITOR, &["right"]),
    command!("move_up", "Cursor", "Up", MoveUp, EDITOR, &["up"]),
    command!("move_down", "Cursor", "Down", MoveDown, EDITOR, &["down"]),
    command!("move_word_left", "Cursor", "Word left", MoveWordLeft, EDITOR, if MAC { &["alt-left"] } else { &["alt-left", "ctrl-left"] }),
    command!("move_word_right", "Cursor", "Word right", MoveWordRight, EDITOR, if MAC { &["alt-right"] } else { &["alt-right", "ctrl-right"] }),
    command!("move_line_start", "Cursor", "Line start", MoveLineStart, EDITOR, if MAC { &["home", "cmd-left"] } else { &["home"] }),
    command!("move_line_end", "Cursor", "Line end", MoveLineEnd, EDITOR, if MAC { &["end", "cmd-right"] } else { &["end"] }),
    command!("move_document_start", "Cursor", "Document start", MoveDocumentStart, EDITOR, if MAC { &["cmd-up"] } else { &["ctrl-home"] }),
    command!("move_document_end", "Cursor", "Document end", MoveDocumentEnd, EDITOR, if MAC { &["cmd-down"] } else { &["ctrl-end"] }),
    command!("page_up", "Cursor", "Page up", PageUp, EDITOR, &["pageup"]),
    command!("page_down", "Cursor", "Page down", PageDown, EDITOR, &["pagedown"]),
    command!("select_left", "Selection", "Select left", SelectLeft, EDITOR, &["shift-left"]),
    command!("select_right", "Selection", "Select right", SelectRight, EDITOR, &["shift-right"]),
    command!("select_up", "Selection", "Select up", SelectUp, EDITOR, &["shift-up"]),
    command!("select_down", "Selection", "Select down", SelectDown, EDITOR, &["shift-down"]),
    command!("select_word_left", "Selection", "Select word left", SelectWordLeft, EDITOR, if MAC { &["alt-shift-left"] } else { &["alt-shift-left", "ctrl-shift-left"] }),
    command!("select_word_right", "Selection", "Select word right", SelectWordRight, EDITOR, if MAC { &["alt-shift-right"] } else { &["alt-shift-right", "ctrl-shift-right"] }),
    command!("select_line_start", "Selection", "Select to line start", SelectLineStart, EDITOR, if MAC { &["shift-home", "cmd-shift-left"] } else { &["shift-home"] }),
    command!("select_line_end", "Selection", "Select to line end", SelectLineEnd, EDITOR, if MAC { &["shift-end", "cmd-shift-right"] } else { &["shift-end"] }),
    command!("select_document_start", "Selection", "Select to document start", SelectDocumentStart, EDITOR, if MAC { &["cmd-shift-up"] } else { &[] }),
    command!("select_document_end", "Selection", "Select to document end", SelectDocumentEnd, EDITOR, if MAC { &["cmd-shift-down"] } else { &[] }),
    // Only while the popup shows, and later than the editor keys, so they win.
    command!("accept_completion", "Suggestions", "Accept suggestion", AcceptCompletion, COMPLETION, &["tab"]),
    command!("next_completion", "Suggestions", "Next suggestion", NextCompletion, COMPLETION, &["down", "ctrl-n"]),
    command!("previous_completion", "Suggestions", "Previous suggestion", PreviousCompletion, COMPLETION, &["up", "ctrl-p"]),
    command!("close_completion", "Suggestions", "Close suggestions", CloseCompletion, COMPLETION, &["escape"]),
    command!("layout_split", "View", "Editor and PDF side by side", LayoutSplit, None, if MAC { &["ctrl-cmd-down"] } else { &["ctrl-alt-down"] }),
    command!("layout_editor", "View", "Editor only", LayoutEditorOnly, None, if MAC { &["ctrl-cmd-left"] } else { &["ctrl-alt-left"] }),
    command!("layout_pdf", "View", "PDF only", LayoutPdfOnly, None, if MAC { &["ctrl-cmd-right"] } else { &["ctrl-alt-right"] }),
    command!("layout_pdf_tab", "View", "PDF in its own tab", LayoutPdfTab, None, if MAC { &["ctrl-cmd-up"] } else { &["ctrl-alt-up"] }),
    command!("focus_mode", "View", "Focus mode", ToggleFocusMode, None, &["secondary-shift-m"]),
    command!("toggle_file_tree", "View", "Toggle file tree", ToggleFileTree, None, &[]),
    command!("toggle_preview", "View", "Toggle PDF preview", TogglePreview, None, &[]),
    command!("toggle_logs", "View", "Toggle log", ToggleLogs, None, &[]),
    command!("start_presentation", "View", "Presentation mode", StartPresentation, None, &[]),
    command!("pdf_copy", "PDF", "Copy the selected PDF text", Copy, PDF, &["secondary-c"]),
    command!("pdf_back", "PDF", "Back after following a link", PdfBack, PDF, &["secondary-[", "alt-left"]),
    command!("pdf_forward", "PDF", "Forward again", PdfForward, PDF, &["secondary-]", "alt-right"]),
    // The slides of a talk turn with every key a clicker or a hand reaches first.
    command!("next_slide", "Presentation", "Next slide", PdfNextPage, PRESENTATION, &["j", "space", "down", "right", "pagedown"]),
    command!("previous_slide", "Presentation", "Previous slide", PdfPreviousPage, PRESENTATION, &["k", "shift-space", "up", "left", "pageup"]),
    command!("end_presentation", "Presentation", "End presentation mode", ExitPresentation, PRESENTATION, &["escape"]),
];

/// Binds every command to its keys from `keymap`, in place of what was bound
/// before. Returns the keys that gpui could not read.
pub fn bind_keys(keymap: &Keymap, addon_commands: &[AddonBinding], cx: &mut App) -> Vec<String> {
    let mut problems = Vec::new();
    let mut bindings = Vec::new();
    for command in COMMANDS {
        let predicate: Option<std::rc::Rc<KeyBindingContextPredicate>> =
            command.context.map(|context| KeyBindingContextPredicate::parse(context).expect("a valid context").into());
        for keys in keymap.keys(command) {
            match KeyBinding::load(&keys, command.action(), predicate.clone(), false, None, cx) {
                Ok(binding) => bindings.push(binding),
                Err(_) => problems.push(format!("{}: cannot read the keys \"{keys}\"", command.id)),
            }
        }
    }
    // After the editor's own, so that an add-on's keys win where they overlap,
    // as the user chose to turn it on.
    for command in addon_commands {
        let id = format!("{ADDON_PREFIX}{}.{}", command.addon, command.command);
        for keys in keymap.addon_keys(&id, &command.keys) {
            let action = RunAddonCommand { addon: command.addon.clone(), command: command.command.clone() };
            match KeyBinding::load(&keys, Box::new(action), None, false, None, cx) {
                Ok(binding) => bindings.push(binding),
                Err(_) => problems.push(format!("{id}: cannot read the keys \"{keys}\"")),
            }
        }
    }
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
    problems
}
