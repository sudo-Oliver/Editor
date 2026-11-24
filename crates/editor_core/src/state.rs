use std::{
    ops::Range,
    time::{Duration, Instant},
};

use tree_sitter::{InputEdit, Point};

use crate::{
    buffer::Buffer,
    selection::{Selection, SelectionSet},
    snippet::Snippet,
    syntax::{Language, ParseJob, ParseResult, Syntax},
};

const INDENT: &str = "  ";
const INDENT_WIDTH: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkSeverity {
    Error,
    Warning,
    /// Advice from the editor, not from the compiler.
    Hint,
}

/// Who put a mark there, so that each replaces only its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkSource {
    /// The last build's log.
    #[default]
    Compiler,
    /// Advice about packages.
    Packages,
    /// Keys, labels and bibliographies, checked as one types.
    Citations,
}

/// A diagnostic range that follows edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub range: Range<usize>,
    pub severity: MarkSeverity,
    pub message: String,
    pub source: MarkSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

/// Replaces `range` of the text before the transaction with `text`.
#[derive(Clone, Debug)]
struct Edit {
    range: Range<usize>,
    text: String,
}

impl Operation {
    fn bytes(&self) -> usize {
        self.removed.len() + self.inserted.len()
    }
}

#[derive(Clone, Debug)]
struct Operation {
    start: usize,
    removed: String,
    inserted: String,
}

/// More than a long day of work, and small enough that an undo history never
/// becomes the reason for running out of memory.
const MAX_UNDO_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Typing,
    Other,
}

impl Transaction {
    fn bytes(&self) -> usize {
        self.operations.iter().map(Operation::bytes).sum()
    }
}

struct Transaction {
    operations: Vec<Operation>,
    selections_before: SelectionSet,
    selections_after: SelectionSet,
    revision_before: u64,
    revision_after: u64,
    kind: GroupKind,
    last_edit: Instant,
}

/// Lines that one buffer change replaced, for view caches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineChange {
    pub start_line: usize,
    pub removed_lines: usize,
    pub inserted_lines: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    /// The selection of someone else.
    Selection,
    /// The cursor of someone else, with their name.
    Caret,
    Comment,
    /// Text suggested for insertion.
    Inserted,
    /// Where text was suggested for deletion.
    Deleted,
}

/// A colored range drawn over the text that follows edits, like a mark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoration {
    pub range: Range<usize>,
    pub kind: DecorationKind,
    pub color: u32,
    pub label: Option<String>,
}

/// One buffer change in the coordinates of the text right before it, for
/// sharing edits with other editors. Changes follow each other in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextChange {
    pub start: usize,
    pub removed: usize,
    pub inserted: String,
    /// What `removed` covered. Only local changes know it.
    pub removed_text: String,
}

impl TextChange {
    pub fn new(start: usize, removed: usize, inserted: impl Into<String>) -> Self {
        Self {
            start,
            removed,
            inserted: inserted.into(),
            removed_text: String::new(),
        }
    }
}

/// Document model without any UI state: text, selections, history and syntax.
pub struct EditorState {
    buffer: Buffer,
    language: Language,
    selections: SelectionSet,
    undo_stack: Vec<Transaction>,
    /// The text held by `undo_stack`.
    undo_bytes: usize,
    /// Transactions dropped from the start of `undo_stack`, which checkpoints count.
    undo_dropped: usize,
    redo_stack: Vec<Transaction>,
    group_timeout: Duration,
    version: u64,
    revision: u64,
    next_revision: u64,
    saved_revision: u64,
    marks: Vec<Mark>,
    syntax: Syntax,
    edits_since_job: Vec<InputEdit>,
    line_changes: Vec<LineChange>,
    active_snippet: Option<ActiveSnippet>,
    /// Local changes not yet taken, while someone listens for them.
    change_log: Option<Vec<TextChange>>,
    decorations: Vec<Decoration>,
    /// Edits of the user do nothing, edits of others still arrive.
    read_only: bool,
}

/// The stops of the snippet that `Tab` is walking through, in document order.
struct ActiveSnippet {
    stops: Vec<Range<usize>>,
    current: usize,
}

impl ActiveSnippet {
    /// Follows an edit. An edit outside the current stop ends the snippet.
    fn edited(&mut self, range: &Range<usize>, inserted: usize) -> bool {
        let current = &self.stops[self.current];
        if range.start < current.start || range.end > current.end {
            return false;
        }
        let delta = inserted as isize - range.len() as isize;
        let shift = |offset: usize| (offset as isize + delta) as usize;
        self.stops[self.current].end = shift(self.stops[self.current].end);
        for stop in &mut self.stops[self.current + 1..] {
            *stop = shift(stop.start)..shift(stop.end);
        }
        true
    }
}

impl EditorState {
    pub fn new(text: &str, language: Language) -> Self {
        Self {
            buffer: Buffer::new(text),
            language,
            selections: SelectionSet::default(),
            undo_stack: Vec::new(),
            undo_bytes: 0,
            undo_dropped: 0,
            redo_stack: Vec::new(),
            group_timeout: Duration::from_millis(600),
            version: 1,
            revision: 0,
            next_revision: 1,
            saved_revision: 0,
            marks: Vec::new(),
            syntax: Syntax::new(language),
            edits_since_job: Vec::new(),
            line_changes: Vec::new(),
            active_snippet: None,
            change_log: None,
            decorations: Vec::new(),
            read_only: false,
        }
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn selections(&self) -> &SelectionSet {
        &self.selections
    }

    pub fn set_selections(&mut self, selections: Vec<Selection>) {
        self.selections = SelectionSet::from_vec(self.clip_selections(selections));
        self.check_snippet();
        self.seal_undo_group();
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn syntax(&self) -> &Syntax {
        &self.syntax
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    /// Counts the text as unsaved, for a file whose disk copy no longer matches.
    pub fn mark_unsaved(&mut self) {
        // No revision ever reaches this.
        self.saved_revision = u64::MAX;
    }

    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    pub fn set_marks(&mut self, marks: Vec<Mark>) {
        self.marks = marks;
    }

    pub fn decorations(&self) -> &[Decoration] {
        &self.decorations
    }

    pub fn set_decorations(&mut self, mut decorations: Vec<Decoration>) {
        let length = self.buffer.len();
        for decoration in &mut decorations {
            let start = self.buffer.clip(decoration.range.start.min(length));
            let end = self.buffer.clip(decoration.range.end.min(length)).max(start);
            decoration.range = start..end;
        }
        self.decorations = decorations;
    }

    pub fn take_line_changes(&mut self) -> Vec<LineChange> {
        std::mem::take(&mut self.line_changes)
    }

    /// Starts or stops collecting local changes for `take_changes`.
    pub fn set_change_log(&mut self, on: bool) {
        self.change_log = on.then(Vec::new);
    }

    /// Local changes since the last call, undo and redo included.
    pub fn take_changes(&mut self) -> Vec<TextChange> {
        self.change_log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Puts back text that a tracked deletion took out, since the session
    /// keeps it, struck through. The deletion leaves the undo history as if
    /// it never happened, so that undo cannot bring the text back twice.
    pub fn restore_deleted(&mut self, restores: &[TextChange]) {
        let log = self.change_log.take();
        let mut selections = self.selections.to_vec_primary_last();
        for restore in restores {
            let at = self.buffer.clip(restore.start.min(self.buffer.len()));
            let inserted = restore.inserted.len();
            self.raw_replace(at..at, &restore.inserted);
            for selection in &mut selections {
                selection.anchor = map_mark_offset(selection.anchor, &(at..at), inserted);
                selection.head = map_mark_offset(selection.head, &(at..at), inserted);
            }
            if !self.forget_deletion(at, &restore.inserted) {
                self.rebase_history(at..at, inserted);
            }
        }
        self.change_log = log;
        self.selections = SelectionSet::from_vec(selections);
        self.redo_stack.clear();
    }

    /// Applies changes made by someone else. They stay out of the undo
    /// history, and the history moves around them, so that undo only ever
    /// reverts local edits. History that a change runs into is dropped.
    pub fn apply_remote(&mut self, changes: &[TextChange]) {
        let log = self.change_log.take();
        self.active_snippet = None;
        self.redo_stack.clear();
        let mut selections = self.selections.to_vec_primary_last();
        for change in changes {
            let range = self.buffer.clip(change.start)..self.buffer.clip(change.start + change.removed);
            let inserted = change.inserted.len();
            self.raw_replace(range.clone(), &change.inserted);
            for selection in &mut selections {
                selection.anchor = map_mark_offset(selection.anchor, &range, inserted);
                selection.head = map_mark_offset(selection.head, &range, inserted);
            }
            self.rebase_history(range, inserted);
        }
        self.change_log = log;
        self.selections = SelectionSet::from_vec(selections);
        self.seal_undo_group();
        self.revision = self.next_revision;
        self.next_revision += 1;
    }

    /// Starts a parse of the current text. Edits made after this call are
    /// replayed onto the result in `finish_parse`.
    pub fn start_parse(&mut self) -> Option<ParseJob> {
        self.edits_since_job.clear();
        self.syntax.parse_job(self.buffer.rope(), self.version)
    }

    pub fn finish_parse(&mut self, result: ParseResult) -> bool {
        let edits = std::mem::take(&mut self.edits_since_job);
        self.syntax.apply(result, &edits)
    }

    /// Closes the current undo group, so that the next edit starts a new one.
    pub fn seal_undo_group(&mut self) {
        if let Some(last) = self.undo_stack.last_mut() {
            last.kind = GroupKind::Other;
        }
    }

    /// Marks the current point in the history for `merge_undo_since`.
    pub fn undo_checkpoint(&self) -> usize {
        self.undo_dropped + self.undo_stack.len()
    }

    /// Turns everything done since `checkpoint` into one undo step.
    pub fn merge_undo_since(&mut self, checkpoint: usize) {
        // Everything since then may be gone already, if the history got too large.
        let Some(checkpoint) = checkpoint.checked_sub(self.undo_dropped) else {
            return;
        };
        if self.undo_stack.len() <= checkpoint + 1 {
            return;
        }
        let mut later = self.undo_stack.split_off(checkpoint + 1);
        let last = later.last().map(|transaction| (transaction.selections_after.clone(), transaction.revision_after, transaction.last_edit));
        let first = &mut self.undo_stack[checkpoint];
        for transaction in &mut later {
            first.operations.append(&mut transaction.operations);
        }
        if let Some((selections_after, revision_after, last_edit)) = last {
            first.selections_after = selections_after;
            first.revision_after = revision_after;
            first.last_edit = last_edit;
        }
        first.kind = GroupKind::Other;
    }

    // Motion

    pub fn move_selections(&mut self, motion: Motion, extend: bool) {
        let buffer = &self.buffer;
        let selections = self
            .selections
            .to_vec_primary_last()
            .into_iter()
            .map(|selection| {
                if !extend && !selection.is_empty() {
                    match motion {
                        Motion::Left => return Selection::cursor(selection.start()),
                        Motion::Right => return Selection::cursor(selection.end()),
                        _ => {}
                    }
                }
                let head = motion_target(buffer, selection.head, motion);
                if extend {
                    Selection::new(selection.anchor, head)
                } else {
                    Selection::cursor(head)
                }
            })
            .collect();
        self.set_selections(selections);
    }

    pub fn select_all(&mut self) {
        self.set_selections(vec![Selection::new(0, self.buffer.len())]);
    }

    pub fn select_word_at(&mut self, offset: usize) {
        let range = self.buffer.word_range(self.buffer.clip(offset));
        self.set_selections(vec![Selection::new(range.start, range.end)]);
    }

    pub fn select_line_at(&mut self, offset: usize) {
        let line = self.buffer.offset_to_line(offset);
        let end = if line + 1 < self.buffer.line_count() {
            self.buffer.line_start(line + 1)
        } else {
            self.buffer.len()
        };
        self.set_selections(vec![Selection::new(self.buffer.line_start(line), end)]);
    }

    /// Adds a cursor at `offset`. A second click on a cursor removes this cursor,
    /// unless it is the only one.
    pub fn toggle_cursor(&mut self, offset: usize) {
        let offset = self.buffer.clip(offset);
        let mut selections = self.selections.to_vec_primary_last();
        if let Some(index) = selections
            .iter()
            .position(|selection| selection.is_empty() && selection.head == offset)
        {
            if selections.len() > 1 {
                selections.remove(index);
            }
        } else {
            selections.push(Selection::cursor(offset));
        }
        self.set_selections(selections);
    }

    /// Selects the word under an empty primary cursor. With a selection, adds
    /// the next occurrence of the selected text, wrapping at the document end.
    pub fn select_next_occurrence(&mut self) {
        let primary = *self.selections.primary();
        if primary.is_empty() {
            let range = self.buffer.word_range(primary.head);
            if range.is_empty() {
                return;
            }
            let mut selections = self.selections.to_vec_primary_last();
            selections.pop();
            selections.push(Selection::new(range.start, range.end));
            self.set_selections(selections);
            return;
        }

        let needle = self.buffer.slice(primary.range());
        let text = self.buffer.to_string();
        let search_from = self.selections.as_slice().last().map_or(0, Selection::end);
        let found = text[search_from..]
            .match_indices(&needle)
            .map(|(index, _)| index + search_from)
            .chain(text[..search_from].match_indices(&needle).map(|(index, _)| index))
            .find(|&start| {
                !self
                    .selections
                    .iter()
                    .any(|selection| selection.start() < start + needle.len() && start < selection.end())
            });
        if let Some(start) = found {
            let mut selections = self.selections.to_vec_primary_last();
            selections.push(Selection::new(start, start + needle.len()));
            self.set_selections(selections);
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let parts: Vec<String> = self
            .selections
            .iter()
            .filter(|selection| !selection.is_empty())
            .map(|selection| self.buffer.slice(selection.range()))
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    // Editing

    /// Types `text` at every selection, with bracket pairing for single characters.
    pub fn type_text(&mut self, text: &str) {
        let mut chars = text.chars();
        let single = match (chars.next(), chars.next()) {
            (Some(ch), None) => Some(ch),
            _ => None,
        };
        let is_newline = text.contains('\n');
        let language = self.language;
        let buffer = &self.buffer;
        let planned = self
            .selections
            .iter()
            .map(|selection| match single {
                Some(ch) => plan_typed_char(buffer, language, selection, ch),
                None => PlannedEdit::replace_selection(selection, text),
            })
            .collect();
        let kind = if is_newline {
            GroupKind::Other
        } else {
            GroupKind::Typing
        };
        self.apply_planned(planned, kind);
    }

    /// Inserts text verbatim at every selection, without bracket pairing.
    pub fn insert(&mut self, text: &str) {
        let planned = self
            .selections
            .iter()
            .map(|selection| PlannedEdit::replace_selection(selection, text))
            .collect();
        self.apply_planned(planned, GroupKind::Other);
    }

    /// Surrounds every selection with `prefix` and `suffix`, keeping the inner text selected.
    pub fn wrap_selections(&mut self, prefix: &str, suffix: &str) {
        let buffer = &self.buffer;
        let planned = self
            .selections
            .iter()
            .map(|selection| {
                let inner = buffer.slice(selection.range());
                PlannedEdit {
                    range: selection.range(),
                    text: format!("{prefix}{inner}{suffix}"),
                    anchor: prefix.len(),
                    head: prefix.len() + inner.len(),
                }
            })
            .collect();
        self.apply_planned(planned, GroupKind::Other);
    }

    /// Replaces text without moving the cursors of the user, except past the edit.
    pub fn edit_keeping_selections(&mut self, range: Range<usize>, text: &str) {
        self.apply_edits_mapping_selections(vec![Edit { range, text: text.to_string() }]);
    }

    /// Inserts a snippet at every selection, with selected text in the first slot.
    /// With `math`, the snippet gets `$` around it outside a formula. `Tab` walks
    /// the slots when there is one cursor.
    pub fn insert_snippet(&mut self, template: &str, math: bool) {
        let single = self.selections.len() == 1;
        let mut first_stops = None;
        let planned: Vec<PlannedEdit> = self
            .selections
            .iter()
            .map(|selection| {
                let start = selection.start();
                let line = self.buffer.line_text(self.buffer.offset_to_line(start));
                let indent = &line[..line.len() - line.trim_start().len()];
                let mut snippet = Snippet::parse(template, indent);
                if !selection.is_empty() {
                    snippet.fill_first(&self.buffer.slice(selection.range()));
                }
                if math && !self.syntax.in_math(self.buffer.rope(), start) {
                    snippet.surround("$", "$");
                } else {
                    let before = self.char_before(start);
                    let after = self.buffer.char_at(selection.end());
                    let (prefix, suffix) = word_padding(&snippet.text, before, after);
                    snippet.surround(prefix, suffix);
                }
                let (anchor, head) = snippet
                    .slots
                    .first()
                    .map_or((snippet.text.len(), snippet.text.len()), |slot| (slot.start, slot.end));
                if single {
                    first_stops = Some((start, snippet.slots.clone(), snippet.text.len()));
                }
                PlannedEdit {
                    range: selection.range(),
                    text: snippet.text,
                    anchor,
                    head,
                }
            })
            .collect();
        self.apply_planned(planned, GroupKind::Other);
        self.seal_undo_group();
        let Some((start, slots, end)) = first_stops.filter(|(_, slots, _)| !slots.is_empty()) else {
            return;
        };
        let mut stops: Vec<Range<usize>> = slots.iter().map(|slot| start + slot.start..start + slot.end).collect();
        // Inside another snippet, the new slots come before the next outer one.
        if let Some(active) = &mut self.active_snippet {
            active.stops.splice(active.current..=active.current, stops);
        } else {
            stops.push(start + end..start + end);
            self.active_snippet = Some(ActiveSnippet { stops, current: 0 });
        }
    }

    /// Replaces `prefix`, the text before the cursor, with a snippet. The typed
    /// text comes back with one undo.
    pub fn complete_prefix(&mut self, prefix: Range<usize>, template: &str) {
        self.seal_undo_group();
        let checkpoint = self.undo_checkpoint();
        let start = prefix.start;
        self.replace_range(prefix, "", Selection::cursor(start));
        self.insert_snippet(template, false);
        self.merge_undo_since(checkpoint);
    }

    /// Moves to the next slot of the active snippet. Returns false without one.
    pub fn next_snippet_slot(&mut self) -> bool {
        let Some(active) = &mut self.active_snippet else {
            return false;
        };
        active.current += 1;
        let stop = active.stops[active.current].clone();
        if active.current + 1 == active.stops.len() {
            self.active_snippet = None;
        }
        self.selections = SelectionSet::single(Selection::new(stop.start, stop.end));
        self.seal_undo_group();
        true
    }

    pub fn end_snippet(&mut self) {
        self.active_snippet = None;
    }

    pub fn in_math(&self, offset: usize) -> bool {
        self.syntax.in_math(self.buffer.rope(), offset)
    }

    /// Pastes one line per selection when the line count matches the cursor count.
    pub fn paste(&mut self, text: &str) {
        let lines: Vec<&str> = text.split('\n').collect();
        if self.selections.len() > 1 && lines.len() == self.selections.len() {
            let planned = self
                .selections
                .iter()
                .zip(lines)
                .map(|(selection, line)| PlannedEdit::replace_selection(selection, line))
                .collect();
            self.apply_planned(planned, GroupKind::Other);
        } else {
            self.insert(text);
        }
    }

    pub fn newline(&mut self) {
        let buffer = &self.buffer;
        let language = self.language;
        let planned = self
            .selections
            .iter()
            .map(|selection| plan_newline(buffer, language, selection))
            .collect();
        self.apply_planned(planned, GroupKind::Other);
    }

    pub fn backspace(&mut self) {
        let buffer = &self.buffer;
        let planned = self
            .selections
            .iter()
            .map(|selection| {
                if !selection.is_empty() {
                    return PlannedEdit::replace_selection(selection, "");
                }
                let head = selection.head;
                let start = buffer.previous_boundary(head);
                let mut end = head;
                if let (Some(open), Some(close)) = (buffer.char_before(head), buffer.char_at(head))
                    && closer_for(open) == Some(close)
                {
                    end = head + close.len_utf8();
                }
                PlannedEdit::cursor_after(start..end, String::new())
            })
            .collect();
        self.apply_planned(planned, GroupKind::Typing);
    }

    pub fn delete_forward(&mut self) {
        self.delete_motion(Motion::Right);
    }

    pub fn delete_motion(&mut self, motion: Motion) {
        let buffer = &self.buffer;
        let planned = self
            .selections
            .iter()
            .map(|selection| {
                if !selection.is_empty() {
                    return PlannedEdit::replace_selection(selection, "");
                }
                let target = motion_target(buffer, selection.head, motion);
                PlannedEdit::cursor_after(
                    target.min(selection.head)..target.max(selection.head),
                    String::new(),
                )
            })
            .collect();
        self.apply_planned(planned, GroupKind::Other);
    }

    /// Indents selected lines. Empty cursors advance to the next tab stop.
    pub fn indent(&mut self) {
        let multiline = self.selections.iter().any(|selection| {
            self.buffer.offset_to_line(selection.start()) != self.buffer.offset_to_line(selection.end())
        });
        if multiline {
            let edits = self
                .selected_lines()
                .into_iter()
                .map(|line| {
                    let start = self.buffer.line_start(line);
                    Edit {
                        range: start..start,
                        text: INDENT.to_string(),
                    }
                })
                .collect();
            self.apply_edits_mapping_selections(edits);
            return;
        }
        let buffer = &self.buffer;
        let planned = self
            .selections
            .iter()
            .map(|selection| {
                let (_, column) = buffer.offset_to_point(selection.start());
                let width = INDENT_WIDTH - column % INDENT_WIDTH;
                PlannedEdit::replace_selection(selection, &" ".repeat(width))
            })
            .collect();
        self.apply_planned(planned, GroupKind::Other);
    }

    pub fn outdent(&mut self) {
        let edits = self
            .selected_lines()
            .into_iter()
            .filter_map(|line| {
                let text = self.buffer.line_text(line);
                let width = text
                    .chars()
                    .take(INDENT_WIDTH)
                    .take_while(|ch| *ch == ' ')
                    .count();
                let width = if text.starts_with('\t') { 1 } else { width };
                let start = self.buffer.line_start(line);
                (width > 0).then(|| Edit {
                    range: start..start + width,
                    text: String::new(),
                })
            })
            .collect();
        self.apply_edits_mapping_selections(edits);
    }

    /// Indents or outdents whole lines, whatever the selection is.
    pub fn indent_lines(&mut self, lines: Range<usize>, outdent: bool) {
        let edits = lines
            .filter_map(|line| {
                let text = self.buffer.line_text(line);
                let start = self.buffer.line_start(line);
                if !outdent {
                    return (!text.is_empty()).then(|| Edit {
                        range: start..start,
                        text: INDENT.to_string(),
                    });
                }
                let width = text
                    .chars()
                    .take(INDENT_WIDTH)
                    .take_while(|ch| *ch == ' ')
                    .count();
                let width = if text.starts_with('\t') { 1 } else { width };
                (width > 0).then(|| Edit {
                    range: start..start + width,
                    text: String::new(),
                })
            })
            .collect();
        self.apply_edits_mapping_selections(edits);
    }

    /// Replaces `range` with `text` and collapses to one selection. Used by IME input.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str, selection: Selection) {
        self.commit(
            vec![Edit {
                range,
                text: text.to_string(),
            }],
            SelectionSet::single(selection),
            GroupKind::Typing,
        );
    }

    pub fn undo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        self.active_snippet = None;
        let Some(transaction) = self.undo_stack.pop() else {
            return false;
        };
        self.undo_bytes -= transaction.bytes();
        for operation in transaction.operations.iter().rev() {
            self.raw_replace(
                operation.start..operation.start + operation.inserted.len(),
                &operation.removed,
            );
        }
        self.selections = transaction.selections_before.clone();
        self.revision = transaction.revision_before;
        self.redo_stack.push(transaction);
        true
    }

    pub fn redo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        self.active_snippet = None;
        let Some(transaction) = self.redo_stack.pop() else {
            return false;
        };
        for operation in &transaction.operations {
            self.raw_replace(
                operation.start..operation.start + operation.removed.len(),
                &operation.inserted,
            );
        }
        self.selections = transaction.selections_after.clone();
        self.revision = transaction.revision_after;
        self.undo_bytes += transaction.bytes();
        self.undo_stack.push(transaction);
        true
    }

    /// Takes the text from disk. The undo history stays usable, and only what
    /// differs goes through it, so that a small change stays small.
    pub fn reload(&mut self, text: &str) {
        let selections = self.selections.to_vec_primary_last();
        let current = self.buffer.to_string();
        let changed = changed_middle(&current, text);
        self.commit(
            vec![Edit {
                range: changed.clone(),
                text: text[changed.start..text.len() - (current.len() - changed.end)].to_string(),
            }],
            SelectionSet::single(Selection::cursor(0)),
            GroupKind::Other,
        );
        let selections = self.clip_selections(selections);
        self.selections = SelectionSet::from_vec(selections);
        self.mark_saved();
    }

    // Internals

    /// Ends the active snippet once the selection leaves the current stop.
    fn check_snippet(&mut self) {
        let Some(active) = &self.active_snippet else {
            return;
        };
        let stop = &active.stops[active.current];
        let primary = self.selections.primary();
        if self.selections.len() != 1 || primary.start() < stop.start || primary.end() > stop.end {
            self.active_snippet = None;
        }
    }

    fn char_before(&self, offset: usize) -> Option<char> {
        (offset > 0).then(|| self.buffer.char_at(self.buffer.previous_boundary(offset))).flatten()
    }

    fn selected_lines(&self) -> Vec<usize> {
        let mut lines = Vec::new();
        for selection in self.selections.iter() {
            let first = self.buffer.offset_to_line(selection.start());
            let (last, column) = self.buffer.offset_to_point(selection.end());
            // A selection that ends at column 0 does not include this line.
            let last = if column == 0 && last > first { last - 1 } else { last };
            for line in first..=last {
                if lines.last() != Some(&line) {
                    lines.push(line);
                }
            }
        }
        lines
    }

    fn clip_selections(&self, selections: Vec<Selection>) -> Vec<Selection> {
        selections
            .into_iter()
            .map(|selection| Selection {
                anchor: self.buffer.clip(selection.anchor),
                head: self.buffer.clip(selection.head),
                goal_x: selection.goal_x,
            })
            .collect()
    }

    fn apply_planned(&mut self, planned: Vec<PlannedEdit>, kind: GroupKind) {
        // `planned` follows document order, so running deltas give the new positions.
        let mut delta = 0isize;
        let mut edits = Vec::with_capacity(planned.len());
        let mut selections = Vec::with_capacity(planned.len());
        let mut previous_end = 0;
        for plan in planned {
            let start = plan.range.start.max(previous_end);
            let end = plan.range.end.max(start);
            previous_end = end;
            let new_start = (start as isize + delta) as usize;
            selections.push(Selection::new(new_start + plan.anchor, new_start + plan.head));
            delta += plan.text.len() as isize - (end - start) as isize;
            edits.push(Edit {
                range: start..end,
                text: plan.text,
            });
        }
        let primary_index = self
            .selections
            .iter()
            .position(|selection| selection == self.selections.primary())
            .unwrap_or(0);
        let primary = selections.remove(primary_index);
        selections.push(primary);
        self.commit(edits, SelectionSet::from_vec(selections), kind);
    }

    fn apply_edits_mapping_selections(&mut self, edits: Vec<Edit>) {
        if edits.is_empty() {
            return;
        }
        let selections = self
            .selections
            .to_vec_primary_last()
            .into_iter()
            .map(|selection| {
                Selection::new(
                    map_offset(selection.anchor, &edits),
                    map_offset(selection.head, &edits),
                )
            })
            .collect();
        self.commit(edits, SelectionSet::from_vec(selections), GroupKind::Other);
    }

    /// Applies `edits` (sorted, not overlapping, in old coordinates) as one undo step.
    fn commit(&mut self, edits: Vec<Edit>, selections_after: SelectionSet, kind: GroupKind) {
        if self.read_only {
            return;
        }
        let now = Instant::now();
        let selections_before = self.selections.clone();
        let mut operations = Vec::with_capacity(edits.len());
        for edit in edits.into_iter().rev() {
            if edit.range.is_empty() && edit.text.is_empty() {
                continue;
            }
            let removed = self.buffer.slice(edit.range.clone());
            self.raw_replace(edit.range.clone(), &edit.text);
            operations.push(Operation {
                start: edit.range.start,
                removed,
                inserted: edit.text,
            });
        }
        self.selections = selections_after;
        self.check_snippet();
        if operations.is_empty() {
            return;
        }
        self.redo_stack.clear();
        self.undo_bytes += operations.iter().map(Operation::bytes).sum::<usize>();

        let revision_before = self.revision;
        self.revision = self.next_revision;
        self.next_revision += 1;

        if kind == GroupKind::Typing
            && let Some(last) = self.undo_stack.last_mut()
            && last.kind == GroupKind::Typing
            && now.duration_since(last.last_edit) < self.group_timeout
        {
            for operation in operations {
                match last.operations.last_mut() {
                    // Typing on is one longer insertion, not one more entry per key.
                    Some(previous) if operation.removed.is_empty() && previous.start + previous.inserted.len() == operation.start => {
                        previous.inserted.push_str(&operation.inserted);
                    }
                    _ => last.operations.push(operation),
                }
            }
            last.selections_after = self.selections.clone();
            last.revision_after = self.revision;
            last.last_edit = now;
            self.trim_undo();
            return;
        }
        self.undo_stack.push(Transaction {
            operations,
            selections_before,
            selections_after: self.selections.clone(),
            revision_before,
            revision_after: self.revision,
            kind,
            last_edit: now,
        });
        self.trim_undo();
    }

    /// The newest step always stays, however large it is.
    fn trim_undo(&mut self) {
        let mut dropped = 0;
        while self.undo_bytes > MAX_UNDO_BYTES && dropped + 1 < self.undo_stack.len() {
            self.undo_bytes -= self.undo_stack[dropped].bytes();
            dropped += 1;
        }
        self.undo_stack.drain(..dropped);
        self.undo_dropped += dropped;
    }

    /// Drops the deletion of `text` at `at` from the newest undo step, now
    /// that the text is back. Returns false where no such deletion is there.
    fn forget_deletion(&mut self, at: usize, text: &str) -> bool {
        let Some(transaction) = self.undo_stack.last_mut() else {
            return false;
        };
        let Some(index) = transaction.operations.iter().rposition(|operation| operation.start == at && operation.removed == text) else {
            return false;
        };
        // Later operations of the step were made without the text.
        for operation in &mut transaction.operations[index + 1..] {
            if operation.start >= at {
                operation.start += text.len();
            }
        }
        map_selections(&mut transaction.selections_after, &(at..at), text.len());
        self.undo_bytes -= text.len();
        let operation = &mut transaction.operations[index];
        if operation.inserted.is_empty() {
            transaction.operations.remove(index);
        } else {
            operation.removed.clear();
            operation.start += text.len();
        }
        if transaction.operations.is_empty() {
            let transaction = self.undo_stack.pop().expect("the step was just looked at");
            self.revision = transaction.revision_before;
        }
        true
    }

    /// Moves the undo history past a change in the current text. The history
    /// is one chain of operations, so the change walks back through it,
    /// newest first, until it runs into text that an operation touched.
    fn rebase_history(&mut self, mut changed: Range<usize>, inserted: usize) {
        let delta = inserted as isize - changed.len() as isize;
        let mut keep_from = 0;
        'transactions: for (index, transaction) in self.undo_stack.iter_mut().enumerate().rev() {
            map_selections(&mut transaction.selections_after, &changed, inserted);
            for operation in transaction.operations.iter_mut().rev() {
                let operation_end = operation.start + operation.inserted.len();
                if changed.end <= operation.start {
                    operation.start = (operation.start as isize + delta) as usize;
                } else if changed.start >= operation_end {
                    let shift = operation.inserted.len() as isize - operation.removed.len() as isize;
                    changed = (changed.start as isize - shift) as usize..(changed.end as isize - shift) as usize;
                } else {
                    keep_from = index + 1;
                    break 'transactions;
                }
            }
            map_selections(&mut transaction.selections_before, &changed, inserted);
        }
        if keep_from > 0 {
            let dropped: Vec<_> = self.undo_stack.drain(..keep_from).collect();
            self.undo_bytes -= dropped.iter().map(Transaction::bytes).sum::<usize>();
            self.undo_dropped += keep_from;
        }
    }

    fn raw_replace(&mut self, range: Range<usize>, text: &str) {
        if let Some(log) = &mut self.change_log {
            log.push(TextChange {
                start: range.start,
                removed: range.len(),
                inserted: text.to_string(),
                removed_text: self.buffer.slice(range.clone()),
            });
        }
        let (start_line, start_column) = self.buffer.offset_to_point(range.start);
        let (old_end_line, old_end_column) = self.buffer.offset_to_point(range.end);
        self.buffer.replace(range.clone(), text);
        let new_end = range.start + text.len();
        let (new_end_line, new_end_column) = self.buffer.offset_to_point(new_end);

        let edit = InputEdit {
            start_byte: range.start,
            old_end_byte: range.end,
            new_end_byte: new_end,
            start_position: Point::new(start_line, start_column),
            old_end_position: Point::new(old_end_line, old_end_column),
            new_end_position: Point::new(new_end_line, new_end_column),
        };
        self.syntax.edit(&edit);
        self.edits_since_job.push(edit);
        self.line_changes.push(LineChange {
            start_line,
            removed_lines: old_end_line - start_line + 1,
            inserted_lines: new_end_line - start_line + 1,
        });
        self.version += 1;

        if self.active_snippet.as_mut().is_some_and(|active| !active.edited(&range, text.len())) {
            self.active_snippet = None;
        }
        for mark in &mut self.marks {
            mark.range.start = map_mark_offset(mark.range.start, &range, text.len());
            mark.range.end = map_mark_offset(mark.range.end, &range, text.len());
        }
        for decoration in &mut self.decorations {
            decoration.range.start = map_mark_offset(decoration.range.start, &range, text.len());
            decoration.range.end = map_mark_offset(decoration.range.end, &range, text.len());
        }
    }
}

fn map_mark_offset(offset: usize, removed: &Range<usize>, inserted: usize) -> usize {
    if offset <= removed.start {
        offset
    } else if offset >= removed.end {
        offset - (removed.end - removed.start) + inserted
    } else {
        removed.start + inserted
    }
}

fn map_selections(selections: &mut SelectionSet, removed: &Range<usize>, inserted: usize) {
    let mapped = selections
        .to_vec_primary_last()
        .into_iter()
        .map(|selection| Selection {
            anchor: map_mark_offset(selection.anchor, removed, inserted),
            head: map_mark_offset(selection.head, removed, inserted),
            goal_x: selection.goal_x,
        })
        .collect();
    *selections = SelectionSet::from_vec(mapped);
}

fn map_offset(offset: usize, edits: &[Edit]) -> usize {
    let mut delta = 0isize;
    for edit in edits {
        if offset < edit.range.start {
            break;
        }
        if offset < edit.range.end {
            return (edit.range.start as isize + delta) as usize;
        }
        delta += edit.text.len() as isize - (edit.range.end - edit.range.start) as isize;
    }
    (offset as isize + delta) as usize
}

fn motion_target(buffer: &Buffer, head: usize, motion: Motion) -> usize {
    match motion {
        Motion::Left => buffer.previous_boundary(head),
        Motion::Right => buffer.next_boundary(head),
        Motion::WordLeft => buffer.previous_word_start(head),
        Motion::WordRight => buffer.next_word_end(head),
        Motion::LineStart => {
            let line = buffer.offset_to_line(head);
            let first = buffer.first_non_whitespace(line);
            if head == first {
                buffer.line_start(line)
            } else {
                first
            }
        }
        Motion::LineEnd => buffer.line_end(buffer.offset_to_line(head)),
        Motion::DocumentStart => 0,
        Motion::DocumentEnd => buffer.len(),
    }
}

/// An edit for one selection. `anchor` and `head` are relative to the edit start.
struct PlannedEdit {
    range: Range<usize>,
    text: String,
    anchor: usize,
    head: usize,
}

impl PlannedEdit {
    fn replace_selection(selection: &Selection, text: &str) -> Self {
        Self::cursor_after(selection.range(), text.to_string())
    }

    fn cursor_after(range: Range<usize>, text: String) -> Self {
        let end = text.len();
        Self {
            range,
            text,
            anchor: end,
            head: end,
        }
    }

    fn cursor_at(range: Range<usize>, text: String, cursor: usize) -> Self {
        Self {
            range,
            text,
            anchor: cursor,
            head: cursor,
        }
    }
}

fn closer_for(open: char) -> Option<char> {
    match open {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        '$' => Some('$'),
        _ => None,
    }
}

/// The spaces that keep a snippet from merging with the word next to it, because
/// `xalpha` is one name in Typst, as is `\\alphax` in LaTeX.
fn word_padding(text: &str, before: Option<char>, after: Option<char>) -> (&'static str, &'static str) {
    let joins = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
    let starts = text.chars().next().is_some_and(|ch| ch.is_alphanumeric() || ch == '@');
    let ends = text.chars().next_back().is_some_and(char::is_alphanumeric);
    (if starts && joins(before) { " " } else { "" }, if ends && joins(after) { " " } else { "" })
}

fn plan_typed_char(
    buffer: &Buffer,
    language: Language,
    selection: &Selection,
    ch: char,
) -> PlannedEdit {
    let text = ch.to_string();
    let closer = closer_for(ch).filter(|_| language != Language::Plain || ch != '$');

    if !selection.is_empty() {
        let Some(closer) = closer else {
            return PlannedEdit::replace_selection(selection, &text);
        };
        let inner = buffer.slice(selection.range());
        let inner_len = inner.len();
        let width = ch.len_utf8();
        return PlannedEdit {
            range: selection.range(),
            text: format!("{ch}{inner}{closer}"),
            anchor: width,
            head: width + inner_len,
        };
    }

    let head = selection.head;
    let next = buffer.char_at(head);
    let previous = buffer.char_before(head);

    // A typed closer steps over the same character instead of doubling it.
    if matches!(ch, '}' | ']' | ')' | '$') && next == Some(ch) && previous != Some('\\') {
        return PlannedEdit::cursor_after(head..head + ch.len_utf8(), text);
    }

    if let Some(closer) = closer {
        let next_allows = next.is_none_or(|next| next.is_whitespace() || matches!(next, '}' | ']' | ')' | '$' | ',' | '.' | ';'));
        let previous_allows = previous != Some('\\')
            && (ch != '$' || previous.is_none_or(|previous| !previous.is_alphanumeric()));
        if next_allows && previous_allows {
            return PlannedEdit::cursor_at(head..head, format!("{ch}{closer}"), ch.len_utf8());
        }
    }
    PlannedEdit::cursor_after(head..head, text)
}

fn plan_newline(buffer: &Buffer, language: Language, selection: &Selection) -> PlannedEdit {
    let start = selection.start();
    let line = buffer.offset_to_line(start);
    let text = buffer.line_text(line);
    let current_indent = &text[..text.len() - text.trim_start().len()];
    let before = buffer.slice(buffer.line_start(line)..start);
    let trimmed = before.trim_end();

    let opens_block = matches!(trimmed.chars().last(), Some('{' | '[' | '('))
        || (language == Language::Latex && opens_latex_environment(trimmed));
    let closes_directly = selection.is_empty()
        && matches!(
            (trimmed.chars().last(), buffer.char_at(start)),
            (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
        );

    if closes_directly {
        let inner = format!("{current_indent}{INDENT}");
        let text = format!("\n{inner}\n{current_indent}");
        return PlannedEdit::cursor_at(selection.range(), text, 1 + inner.len());
    }
    let extra = if opens_block { INDENT } else { "" };
    PlannedEdit::cursor_after(
        selection.range(),
        format!("\n{current_indent}{extra}"),
    )
}

fn opens_latex_environment(line: &str) -> bool {
    line.trim_start().starts_with("\\begin{") && !line.contains("\\end{")
}

/// The part of `old` to replace to get `new`, on character boundaries of both.
pub fn changed_middle(old: &str, new: &str) -> Range<usize> {
    let (old_bytes, new_bytes) = (old.as_bytes(), new.as_bytes());
    let longest = old_bytes.len().min(new_bytes.len());
    let mut start = old_bytes.iter().zip(new_bytes).take_while(|(left, right)| left == right).count();
    while !(old.is_char_boundary(start) && new.is_char_boundary(start)) {
        start -= 1;
    }
    let same_end = old_bytes.iter().rev().zip(new_bytes.iter().rev()).take_while(|(left, right)| left == right).count();
    let mut end = same_end.min(longest - start);
    while !(old.is_char_boundary(old.len() - end) && new.is_char_boundary(new.len() - end)) {
        end -= 1;
    }
    start..old.len() - end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_on_keeps_one_undo_entry() {
        let mut state = EditorState::new("", Language::Plain);
        for ch in ["a", "b", "ä"] {
            state.type_text(ch);
        }
        assert_eq!(state.undo_stack.len(), 1);
        assert_eq!(state.undo_stack[0].operations.len(), 1);
        assert_eq!(state.undo_stack[0].operations[0].inserted, "abä");
        state.undo();
        assert_eq!(state.buffer().to_string(), "");
        state.redo();
        assert_eq!(state.buffer().to_string(), "abä");
    }

    #[test]
    fn undo_keeps_its_limit_and_its_checkpoints() {
        let mut state = EditorState::new("", Language::Plain);
        let large = "x".repeat(MAX_UNDO_BYTES / 2 + 1);
        let old_checkpoint = state.undo_checkpoint();
        let step = |state: &mut EditorState, text: &str| {
            state.replace_range(0..0, text, Selection::cursor(0));
            state.seal_undo_group();
        };
        step(&mut state, &large);
        step(&mut state, "a");
        let checkpoint = state.undo_checkpoint();
        step(&mut state, &large);
        step(&mut state, &large);
        assert!(state.undo_bytes <= MAX_UNDO_BYTES);
        assert_eq!(state.undo_stack.len(), 1);
        assert_eq!(state.undo_checkpoint(), checkpoint + 2);
        state.merge_undo_since(old_checkpoint);
        assert_eq!(state.undo_stack.len(), 1);
        assert!(state.undo());
        assert!(!state.undo());
        assert_eq!(state.undo_bytes, 0);
        assert!(state.redo());
        assert_eq!(state.undo_bytes, large.len());
    }

    #[test]
    fn reload_changes_only_what_differs() {
        assert_eq!(changed_middle("abcd", "abcd"), 4..4);
        assert_eq!(changed_middle("abXcd", "abYcd"), 2..3);
        assert_eq!(changed_middle("aaaa", "aa"), 2..4);
        assert_eq!(changed_middle("aa", "aaaa"), 2..2);
        assert_eq!(changed_middle("", "x"), 0..0);
        // ä and ö share their first byte.
        assert_eq!(changed_middle("xäy", "xöy"), 1..3);
        let mut state = EditorState::new("eins\nzwei\ndrei\n", Language::Plain);
        state.reload("eins\nZWEI\ndrei\n");
        assert_eq!(state.buffer().to_string(), "eins\nZWEI\ndrei\n");
        assert!(!state.is_dirty());
        assert_eq!(state.undo_stack.last().unwrap().operations[0].inserted, "ZWEI");
        state.undo();
        assert_eq!(state.buffer().to_string(), "eins\nzwei\ndrei\n");
        for (old, new) in [("ääää", "ää"), ("äöü", "äü"), ("a\r\nb", "a\nb"), ("🙂x", "🙃x")] {
            let mut state = EditorState::new(old, Language::Plain);
            state.reload(new);
            assert_eq!(state.buffer().to_string(), new);
            state.undo();
            assert_eq!(state.buffer().to_string(), old);
        }
    }

    fn state(text: &str) -> EditorState {
        EditorState::new(text, Language::Latex)
    }

    fn state_at(text: &str, offsets: &[usize]) -> EditorState {
        let mut state = state(text);
        state.set_selections(offsets.iter().map(|&offset| Selection::cursor(offset)).collect());
        state
    }

    fn heads(state: &EditorState) -> Vec<usize> {
        state.selections().iter().map(|selection| selection.head).collect()
    }

    #[test]
    fn multicursor_typing_keeps_offsets_valid() {
        let mut state = state_at("ab\ncd\nef", &[2, 5, 8]);
        state.type_text("X");
        assert_eq!(state.buffer().to_string(), "abX\ncdX\nefX");
        assert_eq!(heads(&state), vec![3, 7, 11]);
    }

    #[test]
    fn multicursor_backspace_deletes_before_each_cursor() {
        let mut state = state_at("abc def", &[3, 7]);
        state.backspace();
        assert_eq!(state.buffer().to_string(), "ab de");
        assert_eq!(heads(&state), vec![2, 5]);
    }

    #[test]
    fn adjacent_cursors_merge_after_collapse() {
        let mut state = state_at("abcd", &[1, 2]);
        state.backspace();
        assert_eq!(state.buffer().to_string(), "cd");
        assert_eq!(heads(&state), vec![0]);
    }

    #[test]
    fn opener_inserts_pair_and_closer_steps_over() {
        let mut state = state_at("", &[0]);
        state.type_text("{");
        assert_eq!(state.buffer().to_string(), "{}");
        assert_eq!(heads(&state), vec![1]);
        state.type_text("}");
        assert_eq!(state.buffer().to_string(), "{}");
        assert_eq!(heads(&state), vec![2]);
    }

    #[test]
    fn dollar_pairs_but_not_after_backslash_or_word() {
        let mut state = state_at("", &[0]);
        state.type_text("$");
        assert_eq!(state.buffer().to_string(), "$$");
        assert_eq!(heads(&state), vec![1]);

        let mut escaped = state_at("\\", &[1]);
        escaped.type_text("$");
        assert_eq!(escaped.buffer().to_string(), "\\$");

        let mut word = state_at("cost", &[4]);
        word.type_text("$");
        assert_eq!(word.buffer().to_string(), "cost$");
    }

    #[test]
    fn opener_does_not_pair_before_a_word() {
        let mut state = state_at("word", &[0]);
        state.type_text("(");
        assert_eq!(state.buffer().to_string(), "(word");
    }

    #[test]
    fn opener_wraps_selection() {
        let mut state = state("word");
        state.set_selections(vec![Selection::new(0, 4)]);
        state.type_text("{");
        assert_eq!(state.buffer().to_string(), "{word}");
        assert_eq!(state.selections().primary().range(), 1..5);
    }

    #[test]
    fn backspace_removes_empty_pair() {
        let mut state = state_at("()", &[1]);
        state.backspace();
        assert_eq!(state.buffer().to_string(), "");
    }

    #[test]
    fn newline_keeps_and_adds_indent() {
        let mut state = state_at("  foo", &[5]);
        state.newline();
        assert_eq!(state.buffer().to_string(), "  foo\n  ");

        let mut block = state_at("\\begin{a}", &[9]);
        block.newline();
        assert_eq!(block.buffer().to_string(), "\\begin{a}\n  ");

        let mut pair = state_at("{}", &[1]);
        pair.newline();
        assert_eq!(pair.buffer().to_string(), "{\n  \n}");
        assert_eq!(heads(&pair), vec![4]);
    }

    #[test]
    fn indent_and_outdent_selected_lines() {
        let mut state = state("a\nb\nc");
        state.set_selections(vec![Selection::new(0, 3)]);
        state.indent();
        assert_eq!(state.buffer().to_string(), "  a\n  b\nc");
        state.outdent();
        assert_eq!(state.buffer().to_string(), "a\nb\nc");
    }

    #[test]
    fn indent_moves_cursor_to_tab_stop() {
        let mut state = state_at("abc", &[3]);
        state.indent();
        assert_eq!(state.buffer().to_string(), "abc ");
    }

    #[test]
    fn undo_and_redo_restore_text_and_selections() {
        let mut state = state_at("ab\ncd", &[2, 5]);
        state.insert("X");
        assert_eq!(state.buffer().to_string(), "abX\ncdX");
        assert!(state.undo());
        assert_eq!(state.buffer().to_string(), "ab\ncd");
        assert_eq!(heads(&state), vec![2, 5]);
        assert!(state.redo());
        assert_eq!(state.buffer().to_string(), "abX\ncdX");
        assert_eq!(heads(&state), vec![3, 7]);
    }

    #[test]
    fn typing_groups_until_sealed() {
        let mut state = state_at("", &[0]);
        state.group_timeout = Duration::from_secs(60);
        state.type_text("a");
        state.type_text("b");
        state.seal_undo_group();
        state.type_text("c");
        state.undo();
        assert_eq!(state.buffer().to_string(), "ab");
        state.undo();
        assert_eq!(state.buffer().to_string(), "");
    }

    #[test]
    fn dirty_flag_follows_undo_to_saved_revision() {
        let mut state = state_at("", &[0]);
        assert!(!state.is_dirty());
        state.insert("a");
        assert!(state.is_dirty());
        state.mark_saved();
        assert!(!state.is_dirty());
        state.insert("b");
        assert!(state.is_dirty());
        state.undo();
        assert!(!state.is_dirty());
    }

    #[test]
    fn next_occurrence_selects_word_then_matches() {
        let mut state = state_at("foo bar foo foo", &[1]);
        state.select_next_occurrence();
        assert_eq!(state.selected_text().unwrap(), "foo");
        state.select_next_occurrence();
        state.select_next_occurrence();
        assert_eq!(state.selections().len(), 3);
        state.select_next_occurrence();
        assert_eq!(state.selections().len(), 3);
        state.type_text("x");
        assert_eq!(state.buffer().to_string(), "x bar x x");
    }

    #[test]
    fn wrap_keeps_inner_text_selected() {
        let mut state = state("a b");
        state.set_selections(vec![Selection::new(0, 1), Selection::new(2, 3)]);
        state.wrap_selections("\\textbf{", "}");
        assert_eq!(state.buffer().to_string(), "\\textbf{a} \\textbf{b}");
        assert_eq!(state.selected_text().unwrap(), "a\nb");
    }

    #[test]
    fn paste_distributes_lines_to_cursors() {
        let mut state = state_at("a\nb", &[1, 3]);
        state.paste("1\n2");
        assert_eq!(state.buffer().to_string(), "a1\nb2");
    }

    #[test]
    fn marks_follow_edits() {
        let mut state = state_at("hello world", &[0]);
        state.set_marks(vec![Mark {
            range: 6..11,
            severity: MarkSeverity::Error,
            message: "bad".into(),
            source: MarkSource::Compiler,
        }]);
        state.insert("xx");
        assert_eq!(state.marks()[0].range, 8..13);
        state.set_selections(vec![Selection::new(10, 12)]);
        state.insert("");
        assert_eq!(state.marks()[0].range, 8..11);
    }

    #[test]
    fn line_changes_report_replaced_lines() {
        let mut state = state_at("a\nb", &[1]);
        state.insert("\n\n");
        assert_eq!(
            state.take_line_changes(),
            vec![LineChange {
                start_line: 0,
                removed_lines: 1,
                inserted_lines: 3,
            }]
        );
    }

    #[test]
    fn edits_keep_syntax_in_sync_with_incremental_parse() {
        let mut state = EditorState::new("\\foo{a}\n", Language::Latex);
        let job = state.start_parse().unwrap();
        let result = job.run();
        state.set_selections(vec![Selection::cursor(0)]);
        state.insert("% note\n");
        assert!(state.finish_parse(result));
        let job = state.start_parse().unwrap();
        assert!(state.finish_parse(job.run()));
        let rope = state.buffer().rope().clone();
        let spans = state.syntax().highlights(&rope, 0..rope.len_bytes());
        assert!(spans.iter().any(|span| span.range == (0..6)));
    }

    #[test]
    fn tab_walks_snippet_slots() {
        let mut state = state("");
        state.insert_snippet("\\frac{${}}{${}}", false);
        assert_eq!(state.buffer().to_string(), "\\frac{}{}");
        state.type_text("ä");
        assert!(state.next_snippet_slot());
        state.type_text("b");
        assert_eq!(state.buffer().to_string(), "\\frac{ä}{b}");
        assert!(state.next_snippet_slot());
        assert_eq!(state.selections().primary().head, state.buffer().len());
        assert!(!state.next_snippet_slot());
    }

    #[test]
    fn default_slot_is_selected_and_typing_replaces_it() {
        let mut state = state("");
        state.insert_snippet("a{${old}}b{${}}", false);
        assert_eq!(state.selections().primary().range(), 2..5);
        state.type_text("new");
        assert!(state.next_snippet_slot());
        assert_eq!(state.selections().primary().head, "a{new}b{".len());
    }

    #[test]
    fn selection_fills_first_slot() {
        let mut state = state("word");
        state.set_selections(vec![Selection::new(0, 4)]);
        state.insert_snippet("\\sqrt{${}}", false);
        assert_eq!(state.buffer().to_string(), "\\sqrt{word}");
        assert_eq!(state.selections().primary().range(), 6..10);
    }

    #[test]
    fn leaving_the_slot_ends_the_snippet() {
        let mut state = state("x");
        state.set_selections(vec![Selection::cursor(0)]);
        state.insert_snippet("{${}}{${}}", false);
        state.set_selections(vec![Selection::cursor(state.buffer().len())]);
        assert!(!state.next_snippet_slot());
    }

    #[test]
    fn undo_removes_the_whole_snippet_and_ends_it() {
        let mut state = state("");
        state.insert_snippet("{${}}{${}}", false);
        assert!(state.undo());
        assert_eq!(state.buffer().to_string(), "");
        assert!(!state.next_snippet_slot());
    }

    #[test]
    fn snippet_inside_a_slot_comes_before_the_next_slot() {
        let mut state = state("");
        state.insert_snippet("A${}B${}", false);
        state.insert_snippet("(${})", false);
        state.type_text("x");
        assert!(state.next_snippet_slot());
        state.type_text("y");
        assert_eq!(state.buffer().to_string(), "A(x)By");
        assert!(state.next_snippet_slot());
        assert!(!state.next_snippet_slot());
    }

    #[test]
    fn plain_symbol_keeps_the_active_snippet() {
        let mut state = state("");
        state.insert_snippet("{${}}{${}}", false);
        state.insert_snippet("\\alpha", false);
        assert!(state.next_snippet_slot());
        assert_eq!(state.selections().primary().head, "{\\alpha}{".len());
    }

    #[test]
    fn several_cursors_get_no_slots() {
        let mut state = state_at("a b", &[0, 2]);
        state.insert_snippet("{${}}", false);
        assert_eq!(state.buffer().to_string(), "{}a {}b");
        assert!(!state.next_snippet_slot());
    }

    #[test]
    fn indents_following_lines_like_the_cursor_line() {
        let mut state = state_at("  x", &[3]);
        state.insert_snippet("\\a\nb", false);
        assert_eq!(state.buffer().to_string(), "  x\\a\n  b");
    }

    #[test]
    fn completion_replaces_the_prefix_and_walks_the_slots() {
        let mut state = EditorState::new("", Language::Latex);
        for ch in ["a", " ", "\\", "f", "r"] {
            state.type_text(ch);
        }
        state.complete_prefix(2..5, "\\frac{${}}{${}}");
        assert_eq!(state.buffer().to_string(), "a \\frac{}{}");
        assert_eq!(state.selections().primary().head, "a \\frac{".len());
        assert!(state.next_snippet_slot());
        assert_eq!(state.selections().primary().head, "a \\frac{}{".len());
        assert!(state.undo());
        assert_eq!(state.buffer().to_string(), "a \\fr");
    }

    #[test]
    fn completion_inside_a_slot_keeps_the_outer_snippet() {
        let mut state = EditorState::new("", Language::Latex);
        state.insert_snippet("{${}}[${}]", false);
        for ch in ["\\", "a", "l"] {
            state.type_text(ch);
        }
        state.complete_prefix(1..4, "\\alpha");
        assert_eq!(state.buffer().to_string(), "{\\alpha}[]");
        assert!(state.next_snippet_slot());
        assert_eq!(state.selections().primary().head, "{\\alpha}[".len());
    }

    #[test]
    fn word_padding_separates_names() {
        let mut state = state_at("xy", &[1]);
        state.insert_snippet("alpha", false);
        assert_eq!(state.buffer().to_string(), "x alpha y");
    }

    fn parsed(text: &str, language: Language) -> EditorState {
        let mut state = EditorState::new(text, language);
        let job = state.start_parse().unwrap();
        state.finish_parse(job.run());
        state
    }

    #[test]
    fn math_mode_comes_from_the_tree() {
        for (text, language, offset, expected) in [
            ("a $x$ b", Language::Latex, 3, true),
            ("a $x$ b", Language::Latex, 1, false),
            ("a $x$ b", Language::Latex, 5, false),
            ("\\[ x \\]", Language::Latex, 3, true),
            ("\\begin{equation}\nx\n\\end{equation}", Language::Latex, 17, true),
            ("a $x$ b", Language::Typst, 3, true),
            ("a $x$ b", Language::Typst, 1, false),
            ("a $x$ b", Language::Typst, 5, false),
            ("a $x$ b", Language::Markdown, 3, true),
            ("a $x$ b", Language::Markdown, 1, false),
            ("# T\n\n$$\nx\n$$\n", Language::Markdown, 9, true),
        ] {
            assert_eq!(parsed(text, language).in_math(offset), expected, "{text} at {offset}");
        }
    }

    #[test]
    fn math_snippet_gets_dollars_only_outside_formulas() {
        let mut outside = parsed("a ", Language::Latex);
        outside.set_selections(vec![Selection::cursor(2)]);
        outside.insert_snippet("\\alpha", true);
        assert_eq!(outside.buffer().to_string(), "a $\\alpha$");

        let mut inside = parsed("$x $", Language::Latex);
        inside.set_selections(vec![Selection::cursor(3)]);
        inside.insert_snippet("\\alpha", true);
        assert_eq!(inside.buffer().to_string(), "$x \\alpha$");

        let mut typst = parsed("$x$", Language::Typst);
        typst.set_selections(vec![Selection::cursor(2)]);
        typst.insert_snippet("alpha", true);
        assert_eq!(typst.buffer().to_string(), "$x alpha$");
    }

    fn remote(start: usize, removed: usize, inserted: &str) -> TextChange {
        TextChange::new(start, removed, inserted)
    }

    #[test]
    fn local_changes_are_logged_and_remote_ones_are_not() {
        let mut state = EditorState::new("ab", Language::Plain);
        state.type_text("x");
        assert!(state.take_changes().is_empty());
        state.set_change_log(true);
        state.set_selections(vec![Selection::cursor(3)]);
        state.type_text("ä");
        state.apply_remote(&[remote(0, 0, "R")]);
        state.undo();
        let undone = TextChange {
            removed_text: "ä".into(),
            ..remote(4, 2, "")
        };
        assert_eq!(state.take_changes(), vec![remote(3, 0, "ä"), undone]);
        assert!(state.take_changes().is_empty());
    }

    #[test]
    fn undo_reverts_only_local_edits_around_remote_ones() {
        let mut state = EditorState::new("hello world", Language::Plain);
        state.set_selections(vec![Selection::cursor(5)]);
        state.type_text(",");
        state.seal_undo_group();
        state.set_selections(vec![Selection::cursor(12)]);
        state.type_text("!");
        // Before, between and after the local edits.
        state.apply_remote(&[remote(0, 0, ">> "), remote(9, 0, "XY"), remote(18, 0, " :)")]);
        assert_eq!(state.buffer().to_string(), ">> hello,XY world! :)");
        assert_eq!(state.selections().primary().head, 18);
        assert!(state.undo());
        assert_eq!(state.buffer().to_string(), ">> hello,XY world :)");
        assert!(state.undo());
        assert_eq!(state.buffer().to_string(), ">> helloXY world :)");
        assert!(!state.undo());
    }

    #[test]
    fn remote_edit_into_local_text_drops_older_history() {
        let mut state = EditorState::new("", Language::Plain);
        state.type_text("one");
        state.seal_undo_group();
        state.type_text(" two");
        state.seal_undo_group();
        state.type_text(" three");
        // Lands inside " two": that step and everything before it go.
        state.apply_remote(&[remote(5, 1, "W")]);
        assert_eq!(state.buffer().to_string(), "one tWo three");
        assert!(state.undo());
        assert_eq!(state.buffer().to_string(), "one tWo");
        assert!(!state.undo());
        assert_eq!(state.undo_checkpoint(), 2);
    }

    #[test]
    fn restored_deletions_leave_the_undo_history() {
        let mut state = EditorState::new("abc", Language::Plain);
        state.set_change_log(true);
        state.set_selections(vec![Selection::cursor(2)]);
        state.type_text("X");
        state.seal_undo_group();
        state.set_selections(vec![Selection::cursor(2)]);
        state.backspace();
        let changes = state.take_changes();
        assert_eq!(changes.last().unwrap().removed_text, "b");
        state.restore_deleted(&[TextChange::new(1, 0, "b")]);
        assert_eq!(state.buffer().to_string(), "abXc");
        // Undo takes back the typing, not the deletion that never stuck.
        assert!(state.undo());
        assert_eq!(state.buffer().to_string(), "abc");
        assert!(!state.undo());
        assert!(state.take_changes().iter().all(|change| change.inserted.is_empty() || change.inserted == "X"));
    }

    #[test]
    fn read_only_blocks_the_user_but_not_others() {
        let mut state = EditorState::new("ab", Language::Plain);
        state.type_text("x");
        state.set_read_only(true);
        state.set_selections(vec![Selection::cursor(0)]);
        state.type_text("y");
        assert!(!state.undo());
        assert_eq!(state.buffer().to_string(), "xab");
        state.apply_remote(&[TextChange::new(0, 0, "R")]);
        assert_eq!(state.buffer().to_string(), "Rxab");
    }
}

