//! Optional modal editing on top of `EditorState`. The caller feeds keys and
//! carries out the returned effects, so this module knows nothing about windows.

use std::ops::Range;

use ropey::RopeSlice;

use crate::{buffer::Buffer, selection::Selection, state::EditorState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Esc,
    Enter,
    Tab,
    Backspace,
    Delete,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
    Command,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::Command => "COMMAND",
        }
    }
}

/// What the view around the editor has to do after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Text for the system clipboard, like `clipboard=unnamedplus`.
    Yank(String),
    Save,
    Close,
    SaveAndClose,
    CloseWithoutSaving,
    FocusTree,
    ToggleTree,
    RevealInTree,
    TogglePreview,
    FindFiles,
    FindText,
    ShowError,
    LookupDoi,
    QuickFix,
    NextError,
    PreviousError,
    PreviousTab,
    NextTab,
    /// Scrolls the viewport by this many rows.
    Scroll(isize),
}

/// Values that only the caller knows.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub clipboard: Option<String>,
    pub half_page: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FindKind {
    To,
    Till,
    BackTo,
    BackTill,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Pending {
    #[default]
    None,
    G,
    Find(FindKind),
    Replace,
    Bracket(char),
    Object {
        around: bool,
    },
    Leader(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordStart { big: bool },
    WordEnd { big: bool },
    WordBack { big: bool },
    LineStart,
    FirstNonBlank,
    LineEnd,
    DocumentStart,
    DocumentEnd,
    ParagraphNext,
    ParagraphPrevious,
    MatchBracket,
    Find { kind: FindKind, ch: char, repeat: bool },
    RepeatFind { reverse: bool },
    SearchNext { reverse: bool },
    HalfDown,
    HalfUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Exclusive,
    Inclusive,
    Linewise,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Search {
    query: String,
    whole_word: bool,
    forward: bool,
}

#[derive(Clone, Debug, Default)]
struct CommandLine {
    prefix: char,
    text: String,
    origin: usize,
    from: Mode,
    search_before: Option<Search>,
    highlight_before: bool,
}

#[derive(Default)]
pub struct Vim {
    mode: Mode,
    pending: Pending,
    count: Option<usize>,
    operator: Option<(Operator, usize)>,
    visual_anchor: usize,
    visual_cursor: usize,
    goal_column: Option<usize>,
    last_find: Option<(FindKind, char)>,
    search: Option<Search>,
    highlight: bool,
    command: Option<CommandLine>,
    message: Option<String>,
    recording: Vec<Key>,
    recording_version: u64,
    discard_recording: bool,
    last_change: Vec<Key>,
    replaying: bool,
    undo_checkpoint: Option<usize>,
}

impl Vim {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn accepts_text(&self) -> bool {
        self.mode == Mode::Insert
    }

    /// The offset under the block cursor.
    pub fn cursor(&self, state: &EditorState) -> usize {
        match self.mode {
            Mode::Visual | Mode::VisualLine => state.buffer().clip(self.visual_cursor),
            _ => state.selections().primary().head,
        }
    }

    /// The line below the status bar: `/query`, `:command` or the last message.
    pub fn command_line(&self) -> Option<String> {
        match &self.command {
            Some(command) => Some(format!("{}{}", command.prefix, command.text)),
            None => self.message.clone(),
        }
    }

    /// Keys of an unfinished command, for the status bar.
    pub fn pending_keys(&self) -> String {
        let mut keys = String::new();
        if let Some(count) = self.count {
            keys.push_str(&count.to_string());
        }
        if let Some((operator, _)) = self.operator {
            keys.push(match operator {
                Operator::Delete => 'd',
                Operator::Change => 'c',
                Operator::Yank => 'y',
                Operator::Indent => '>',
                Operator::Outdent => '<',
            });
        }
        match &self.pending {
            Pending::None => {}
            Pending::G => keys.push('g'),
            Pending::Find(kind) => keys.push(match kind {
                FindKind::To => 'f',
                FindKind::Till => 't',
                FindKind::BackTo => 'F',
                FindKind::BackTill => 'T',
            }),
            Pending::Replace => keys.push('r'),
            Pending::Bracket(bracket) => keys.push(*bracket),
            Pending::Object { around } => keys.push(if *around { 'a' } else { 'i' }),
            Pending::Leader(prefix) => {
                keys.push_str("Space ");
                keys.push_str(prefix);
            }
        }
        keys
    }

    /// Search hits within one line of text, while highlighting is on.
    pub fn highlights_in_line(&self, text: &str) -> Vec<Range<usize>> {
        match &self.search {
            Some(search) if self.highlight => search.matches_in_line(text),
            _ => Vec::new(),
        }
    }

    /// Records a key that the normal text input handles in insert mode, for `.`.
    pub fn record_insert(&mut self, key: Key) {
        if self.mode == Mode::Insert && key != Key::Esc {
            self.recording.push(key);
        }
    }

    /// Records an accepted suggestion, so that `.` types the same result. The
    /// keys that typed `prefix` are already recorded.
    pub fn record_completion(&mut self, prefix: &str, text: &str) {
        for _ in prefix.chars() {
            self.record_insert(Key::Backspace);
        }
        for ch in text.chars() {
            self.record_insert(if ch == '\n' { Key::Enter } else { Key::Char(ch) });
        }
    }

    /// Starts inserting after a snippet from outside the keyboard. Stays out of
    /// `.`, which could not replay it.
    pub fn enter_insert(&mut self, state: &EditorState) {
        if self.mode == Mode::Insert {
            return;
        }
        self.reset_pending();
        self.command = None;
        self.discard_recording = true;
        self.undo_checkpoint.get_or_insert(state.undo_checkpoint());
        self.mode = Mode::Insert;
    }

    /// Takes over a mouse selection, and keeps a click off the line end.
    pub fn after_mouse(&mut self, state: &mut EditorState) {
        match self.mode {
            Mode::Insert | Mode::Command => {}
            Mode::Normal | Mode::Visual | Mode::VisualLine => {
                self.reset_pending();
                self.mode = Mode::Normal;
                if !state.selections().primary().is_empty() {
                    self.enter_visual_from_selection(state);
                } else {
                    clamp_to_last_char(state);
                }
            }
        }
    }

    pub fn key(&mut self, key: Key, state: &mut EditorState, input: &Input) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.message = None;
        if self.mode == Mode::Normal && !state.selections().primary().is_empty() {
            self.enter_visual_from_selection(state);
        }
        if self.undo_checkpoint.is_none() {
            self.undo_checkpoint = Some(state.undo_checkpoint());
        }
        if self.recording.is_empty() {
            self.recording_version = state.version();
        }
        if self.mode != Mode::Command {
            self.recording.push(key);
        }
        self.dispatch(key, state, input, &mut effects);
        self.finish(state);
        effects
    }

    fn finish(&mut self, state: &mut EditorState) {
        if self.mode == Mode::Normal && self.pending == Pending::None && self.operator.is_none() && self.count.is_none() {
            clamp_to_last_char(state);
            let keys = std::mem::take(&mut self.recording);
            if !std::mem::take(&mut self.discard_recording) && state.version() != self.recording_version {
                self.last_change = keys;
            }
            if let Some(checkpoint) = self.undo_checkpoint.take() {
                state.merge_undo_since(checkpoint);
            }
            state.seal_undo_group();
        }
    }

    fn dispatch(&mut self, key: Key, state: &mut EditorState, input: &Input, effects: &mut Vec<Effect>) {
        match self.mode {
            Mode::Insert => self.insert_key(key, state),
            Mode::Command => self.command_key(key, state, effects),
            Mode::Normal | Mode::Visual | Mode::VisualLine => self.normal_key(key, state, input, effects),
        }
    }

    // Insert mode

    fn insert_key(&mut self, key: Key, state: &mut EditorState) {
        match key {
            Key::Esc => {
                let buffer = state.buffer();
                let head = state.selections().primary().head;
                let (_, column) = buffer.offset_to_point(head);
                let target = if column == 0 { head } else { buffer.previous_boundary(head) };
                state.end_snippet();
                set_cursor(state, target);
                self.mode = Mode::Normal;
            }
            Key::Enter => state.newline(),
            Key::Tab => {
                if !state.next_snippet_slot() {
                    state.indent();
                }
            }
            Key::Backspace => state.backspace(),
            Key::Delete => state.delete_forward(),
            Key::Char(ch) => state.type_text(ch.encode_utf8(&mut [0; 4])),
            Key::Ctrl(_) => {}
        }
    }

    // Command line

    fn open_command(&mut self, prefix: char, state: &EditorState) {
        self.command = Some(CommandLine {
            prefix,
            text: String::new(),
            origin: self.cursor(state),
            from: self.mode,
            search_before: self.search.clone(),
            highlight_before: self.highlight,
        });
        self.mode = Mode::Command;
    }

    fn command_key(&mut self, key: Key, state: &mut EditorState, effects: &mut Vec<Effect>) {
        let Some(mut command) = self.command.take() else {
            self.mode = Mode::Normal;
            return;
        };
        match key {
            Key::Esc => {
                self.mode = command.from;
                self.search = command.search_before;
                self.highlight = command.highlight_before;
                self.place(command.origin, state);
                return;
            }
            Key::Enter => {
                self.mode = command.from;
                if command.prefix == ':' {
                    self.place(command.origin, state);
                    self.run_command(command.text.trim(), state, effects);
                } else {
                    self.finish_search(&command, state);
                }
                return;
            }
            Key::Backspace => {
                if command.text.pop().is_none() {
                    self.mode = command.from;
                    self.search = command.search_before;
                    self.highlight = command.highlight_before;
                    self.place(command.origin, state);
                    return;
                }
            }
            Key::Char(ch) => command.text.push(ch),
            Key::Ctrl(_) | Key::Tab | Key::Delete => {}
        }
        if command.prefix != ':' {
            self.preview_search(&command, state);
        }
        self.command = Some(command);
    }

    fn preview_search(&mut self, command: &CommandLine, state: &mut EditorState) {
        let forward = command.prefix == '/';
        if command.text.is_empty() {
            self.search = command.search_before.clone();
            self.highlight = command.highlight_before;
            self.place(command.origin, state);
            return;
        }
        let search = Search { query: command.text.clone(), whole_word: false, forward };
        let hit = search.next_after(state.buffer(), command.origin, forward);
        self.search = Some(search);
        self.highlight = true;
        self.place(hit.unwrap_or(command.origin), state);
    }

    fn finish_search(&mut self, command: &CommandLine, state: &mut EditorState) {
        if command.text.is_empty() {
            self.search = command.search_before.clone();
            self.highlight = self.search.is_some();
        }
        let Some(search) = &self.search else {
            self.place(command.origin, state);
            return;
        };
        match search.next_after(state.buffer(), command.origin, search.forward) {
            Some(hit) => self.place(hit, state),
            None => {
                self.message = Some(format!("E486: Pattern not found: {}", search.query));
                self.place(command.origin, state);
            }
        }
    }

    fn run_command(&mut self, command: &str, state: &mut EditorState, effects: &mut Vec<Effect>) {
        match command {
            "" => {}
            "w" | "write" => effects.push(Effect::Save),
            "q" | "quit" | "bd" => effects.push(Effect::Close),
            "q!" | "quit!" | "bd!" => effects.push(Effect::CloseWithoutSaving),
            "wq" | "x" | "xit" => effects.push(Effect::SaveAndClose),
            "noh" | "nohlsearch" => self.highlight = false,
            _ => match command.parse::<usize>() {
                Ok(line) => {
                    let line = line.saturating_sub(1).min(state.buffer().line_count() - 1);
                    self.place(state.buffer().first_non_whitespace(line), state);
                }
                Err(_) => self.message = Some(format!("E492: Not an editor command: {command}")),
            },
        }
    }

    // Normal and visual mode

    fn normal_key(&mut self, key: Key, state: &mut EditorState, input: &Input, effects: &mut Vec<Effect>) {
        if key == Key::Esc {
            let was_idle = self.pending == Pending::None && self.operator.is_none() && self.count.is_none();
            self.reset_pending();
            if self.is_visual() {
                self.leave_visual(state);
            } else if was_idle {
                self.highlight = false;
            }
            return;
        }
        match std::mem::take(&mut self.pending) {
            Pending::None => {}
            Pending::G => return self.after_g(key, state, input, effects),
            Pending::Find(kind) => {
                if let Key::Char(ch) = key {
                    self.last_find = Some((kind, ch));
                    self.motion(Motion::Find { kind, ch, repeat: false }, state, input, effects);
                } else {
                    self.reset_pending();
                }
                return;
            }
            Pending::Replace => {
                if let Key::Char(ch) = key {
                    self.replace_chars(ch, state);
                }
                self.reset_pending();
                return;
            }
            Pending::Bracket(bracket) => {
                self.reset_pending();
                if key == Key::Char('d') {
                    effects.push(if bracket == ']' { Effect::NextError } else { Effect::PreviousError });
                }
                return;
            }
            Pending::Object { around } => {
                self.text_object(key, around, state, effects);
                return;
            }
            Pending::Leader(prefix) => return self.leader_key(key, prefix, effects),
        }

        let Key::Char(ch) = key else {
            match key {
                Key::Ctrl('d') => self.motion(Motion::HalfDown, state, input, effects),
                Key::Ctrl('u') => self.motion(Motion::HalfUp, state, input, effects),
                Key::Ctrl('r') => self.undo_or_redo(state, false),
                Key::Tab => {
                    self.reset_pending();
                    effects.push(Effect::FocusTree);
                }
                Key::Enter => self.motion(Motion::Down, state, input, effects),
                Key::Backspace => self.motion(Motion::Left, state, input, effects),
                Key::Delete => {
                    self.operator = Some((Operator::Delete, 1));
                    self.motion(Motion::Right, state, input, effects);
                }
                _ => self.reset_pending(),
            }
            return;
        };

        if ch.is_ascii_digit() && (ch != '0' || self.count.is_some()) {
            let digit = ch as usize - '0' as usize;
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(digit).min(1_000_000));
            return;
        }

        if let Some(motion) = motion_for(ch) {
            return self.motion(motion, state, input, effects);
        }
        match ch {
            'g' => self.pending = Pending::G,
            'f' => self.pending = Pending::Find(FindKind::To),
            't' => self.pending = Pending::Find(FindKind::Till),
            'F' => self.pending = Pending::Find(FindKind::BackTo),
            'T' => self.pending = Pending::Find(FindKind::BackTill),
            'i' | 'a' if self.operator.is_some() || self.is_visual() => {
                self.pending = Pending::Object { around: ch == 'a' };
            }
            _ if self.operator.is_some() => self.operator_key(ch, state, effects),
            _ if self.is_visual() => self.visual_key(ch, state, input, effects),
            _ => self.command_char(ch, state, input, effects),
        }
    }

    fn after_g(&mut self, key: Key, state: &mut EditorState, input: &Input, effects: &mut Vec<Effect>) {
        if key == Key::Char('g') {
            self.motion(Motion::DocumentStart, state, input, effects);
        } else {
            self.reset_pending();
        }
    }

    fn leader_key(&mut self, key: Key, mut prefix: String, effects: &mut Vec<Effect>) {
        self.reset_pending();
        let Key::Char(ch) = key else {
            return;
        };
        prefix.push(ch);
        match leader_lookup(&prefix) {
            Leader::Effect(effect) => effects.push(effect),
            Leader::Prefix => self.pending = Pending::Leader(prefix),
            Leader::Unknown => {}
        }
    }

    fn command_char(&mut self, ch: char, state: &mut EditorState, input: &Input, effects: &mut Vec<Effect>) {
        let count = self.count.take().unwrap_or(1);
        let head = state.selections().primary().head;
        let line = state.buffer().offset_to_line(head);
        match ch {
            'd' => self.operator = Some((Operator::Delete, count)),
            'c' => self.operator = Some((Operator::Change, count)),
            'y' => self.operator = Some((Operator::Yank, count)),
            '>' => self.operator = Some((Operator::Indent, count)),
            '<' => self.operator = Some((Operator::Outdent, count)),
            ' ' => self.pending = Pending::Leader(String::new()),
            ']' | '[' => self.pending = Pending::Bracket(ch),
            'r' => {
                self.count = Some(count);
                self.pending = Pending::Replace;
            }
            'i' => self.mode = Mode::Insert,
            'a' => {
                let end = state.buffer().line_end(line);
                let target = state.buffer().next_boundary(head).min(end);
                set_cursor(state, target);
                self.mode = Mode::Insert;
            }
            'I' => {
                let target = state.buffer().first_non_whitespace(line);
                set_cursor(state, target);
                self.mode = Mode::Insert;
            }
            'A' => {
                let target = state.buffer().line_end(line);
                set_cursor(state, target);
                self.mode = Mode::Insert;
            }
            'o' => {
                let target = state.buffer().line_end(line);
                set_cursor(state, target);
                state.newline();
                self.mode = Mode::Insert;
            }
            'O' => {
                let text = state.buffer().line_text(line);
                let indent = text[..text.len() - text.trim_start().len()].to_string();
                let start = state.buffer().line_start(line);
                let cursor = start + indent.len();
                replace(state, start..start, &format!("{indent}\n"), cursor);
                self.mode = Mode::Insert;
            }
            'x' | 'X' | 's' | 'D' | 'C' | 'Y' | 'S' => {
                self.count = Some(count);
                let (operator, motion) = match ch {
                    'x' => (Operator::Delete, Some(Motion::Right)),
                    'X' => (Operator::Delete, Some(Motion::Left)),
                    's' => (Operator::Change, Some(Motion::Right)),
                    'D' => (Operator::Delete, Some(Motion::LineEnd)),
                    'C' => (Operator::Change, Some(Motion::LineEnd)),
                    'Y' => (Operator::Yank, None),
                    _ => (Operator::Change, None),
                };
                self.operator = Some((operator, 1));
                match motion {
                    Some(motion) => self.motion(motion, state, input, effects),
                    None => self.operator_key(if ch == 'Y' { 'y' } else { 'c' }, state, effects),
                }
            }
            'J' => self.join_lines(count.max(2) - 1, state),
            'p' | 'P' => self.put(ch == 'P', count, state, input),
            'u' => {
                self.count = Some(count);
                self.undo_or_redo(state, true);
            }
            '.' => self.repeat_change(count, state, input, effects),
            'v' | 'V' => {
                self.visual_anchor = head;
                self.visual_cursor = head;
                self.mode = if ch == 'v' { Mode::Visual } else { Mode::VisualLine };
                self.sync_visual(state);
            }
            '/' | '?' | ':' => self.open_command(ch, state),
            '*' | '#' => self.search_word(ch == '*', state),
            'H' => effects.push(Effect::PreviousTab),
            'L' => effects.push(Effect::NextTab),
            _ => {}
        }
    }

    fn visual_key(&mut self, ch: char, state: &mut EditorState, input: &Input, effects: &mut Vec<Effect>) {
        self.count = None;
        let operator = match ch {
            'd' | 'x' => Some(Operator::Delete),
            'c' | 's' => Some(Operator::Change),
            'y' => Some(Operator::Yank),
            '>' => Some(Operator::Indent),
            '<' => Some(Operator::Outdent),
            _ => None,
        };
        if let Some(operator) = operator {
            let (range, linewise) = self.visual_range(state);
            self.mode = Mode::Normal;
            return self.apply_operator(operator, range, linewise, state, effects);
        }
        match ch {
            'v' | 'V' => {
                let wanted = if ch == 'v' { Mode::Visual } else { Mode::VisualLine };
                if self.mode == wanted {
                    self.leave_visual(state);
                } else {
                    self.mode = wanted;
                    self.sync_visual(state);
                }
            }
            'o' => {
                std::mem::swap(&mut self.visual_anchor, &mut self.visual_cursor);
                self.sync_visual(state);
            }
            'p' | 'P' => self.put_over_selection(state, input, effects),
            'J' => {
                let buffer = state.buffer();
                let first = buffer.offset_to_line(self.visual_anchor.min(self.visual_cursor));
                let last = buffer.offset_to_line(self.visual_anchor.max(self.visual_cursor));
                let start = buffer.line_start(first);
                self.mode = Mode::Normal;
                set_cursor(state, start);
                self.join_lines((last - first).max(1), state);
            }
            '/' | '?' => self.open_command(ch, state),
            '*' | '#' => self.search_word(ch == '*', state),
            _ => {}
        }
    }

    fn operator_key(&mut self, ch: char, state: &mut EditorState, effects: &mut Vec<Effect>) {
        let Some((operator, operator_count)) = self.operator else {
            return;
        };
        let doubled = matches!(
            (operator, ch),
            (Operator::Delete, 'd') | (Operator::Change, 'c') | (Operator::Yank, 'y') | (Operator::Indent, '>') | (Operator::Outdent, '<')
        );
        if !doubled {
            return self.reset_pending();
        }
        let lines = operator_count.saturating_mul(self.count.unwrap_or(1)).max(1);
        self.reset_pending();
        let buffer = state.buffer();
        let head = state.selections().primary().head;
        let first = buffer.offset_to_line(head);
        let last = (first + lines - 1).min(buffer.line_count() - 1);
        let range = buffer.line_start(first)..buffer.line_end(last);
        self.apply_operator(operator, range, true, state, effects);
    }

    fn text_object(&mut self, key: Key, around: bool, state: &mut EditorState, effects: &mut Vec<Effect>) {
        let Key::Char(ch) = key else {
            return self.reset_pending();
        };
        let cursor = self.cursor(state);
        let Some(range) = object_range(state.buffer(), cursor, ch, around) else {
            let operator = self.operator.is_some();
            self.reset_pending();
            if !operator && self.is_visual() {
                self.sync_visual(state);
            }
            return;
        };
        if let Some((operator, _)) = self.operator {
            self.reset_pending();
            self.apply_operator(operator, range, false, state, effects);
        } else if !range.is_empty() {
            self.reset_pending();
            self.mode = Mode::Visual;
            self.visual_anchor = range.start;
            self.visual_cursor = state.buffer().previous_boundary(range.end);
            self.sync_visual(state);
        }
    }

    fn motion(&mut self, motion: Motion, state: &mut EditorState, input: &Input, effects: &mut Vec<Effect>) {
        let count = self.count.take();
        let operator = self.operator.take();
        let total = operator.map_or(count, |(_, first)| match (first, count) {
            (1, count) => count,
            (first, count) => Some(first.saturating_mul(count.unwrap_or(1))),
        });
        let from = self.cursor(state);
        let is_change_word = operator.is_some_and(|(operator, _)| operator == Operator::Change)
            && matches!(motion, Motion::WordStart { .. })
            && state.buffer().char_at(from).is_some_and(|ch| !ch.is_whitespace());
        let motion = match motion {
            // `cw` stops at the end of the word and keeps the following space.
            Motion::WordStart { big } if is_change_word => Motion::WordEnd { big },
            other => other,
        };
        let in_operator = operator.is_some();
        let Some((target, reach)) = self.resolve(motion, total, from, in_operator, state, input) else {
            return;
        };
        if matches!(motion, Motion::HalfDown | Motion::HalfUp) {
            let rows = input.half_page as isize;
            effects.push(Effect::Scroll(if motion == Motion::HalfDown { rows } else { -rows }));
        }
        if let Some((operator, _)) = operator {
            let buffer = state.buffer();
            let (range, linewise) = match reach {
                Reach::Linewise => {
                    let first = buffer.offset_to_line(from.min(target));
                    let last = buffer.offset_to_line(from.max(target));
                    (buffer.line_start(first)..buffer.line_end(last), true)
                }
                Reach::Inclusive => {
                    let end = buffer.next_boundary(from.max(target));
                    (from.min(target)..end, false)
                }
                Reach::Exclusive => {
                    let mut end = from.max(target);
                    // An exclusive motion that lands on column 0 stops at the previous line end.
                    if matches!(motion, Motion::WordStart { .. })
                        && buffer.offset_to_point(end).1 == 0
                        && buffer.offset_to_line(end) > buffer.offset_to_line(from)
                    {
                        end = buffer.line_end(buffer.offset_to_line(end) - 1);
                    }
                    (from.min(target)..end, false)
                }
            };
            self.apply_operator(operator, range, linewise, state, effects);
        } else {
            self.place(target, state);
        }
    }

    fn resolve(
        &mut self,
        motion: Motion,
        count: Option<usize>,
        from: usize,
        in_operator: bool,
        state: &EditorState,
        input: &Input,
    ) -> Option<(usize, Reach)> {
        let buffer = state.buffer();
        let times = count.unwrap_or(1).max(1);
        let line = buffer.offset_to_line(from);
        let vertical = matches!(motion, Motion::Up | Motion::Down | Motion::HalfUp | Motion::HalfDown | Motion::LineEnd);
        if !vertical {
            self.goal_column = None;
        }
        Some(match motion {
            Motion::Left => (self.step_left(buffer, from, times, in_operator), Reach::Exclusive),
            Motion::Right => (self.step_right(buffer, from, times, in_operator), Reach::Exclusive),
            Motion::Up => (self.vertical(buffer, from, -(times as isize)), Reach::Linewise),
            Motion::Down => (self.vertical(buffer, from, times as isize), Reach::Linewise),
            Motion::HalfDown => (self.vertical(buffer, from, (input.half_page.max(1) * times) as isize), Reach::Linewise),
            Motion::HalfUp => (self.vertical(buffer, from, -((input.half_page.max(1) * times) as isize)), Reach::Linewise),
            Motion::WordStart { big } => (repeat(from, times, |at| next_word_start(buffer, at, big)), Reach::Exclusive),
            Motion::WordEnd { big } => (repeat(from, times, |at| word_end(buffer, at, big)), Reach::Inclusive),
            Motion::WordBack { big } => (repeat(from, times, |at| word_back(buffer, at, big)), Reach::Exclusive),
            Motion::LineStart => (buffer.line_start(line), Reach::Exclusive),
            Motion::FirstNonBlank => (buffer.first_non_whitespace(line), Reach::Exclusive),
            Motion::LineEnd => {
                let target = (line + times - 1).min(buffer.line_count() - 1);
                self.goal_column = Some(usize::MAX);
                let end = buffer.line_end(target);
                let offset = if end == buffer.line_start(target) { end } else { buffer.previous_boundary(end) };
                (offset, Reach::Inclusive)
            }
            Motion::DocumentStart => {
                let target = count.map_or(0, |count| count - 1).min(buffer.line_count() - 1);
                (buffer.first_non_whitespace(target), Reach::Linewise)
            }
            Motion::DocumentEnd => {
                let target = count.map_or(buffer.line_count() - 1, |count| count.saturating_sub(1).min(buffer.line_count() - 1));
                (buffer.first_non_whitespace(target), Reach::Linewise)
            }
            Motion::ParagraphNext => (repeat(from, times, |at| paragraph(buffer, at, true)), Reach::Exclusive),
            Motion::ParagraphPrevious => (repeat(from, times, |at| paragraph(buffer, at, false)), Reach::Exclusive),
            Motion::MatchBracket => (match_bracket(buffer, from)?, Reach::Inclusive),
            Motion::Find { kind, ch, repeat } => find_on_line(buffer, from, kind, ch, times, repeat)?,
            Motion::RepeatFind { reverse } => {
                let (kind, ch) = self.last_find?;
                find_on_line(buffer, from, if reverse { kind.reversed() } else { kind }, ch, times, true)?
            }
            Motion::SearchNext { reverse } => {
                let search = self.search.clone()?;
                self.highlight = true;
                let forward = search.forward != reverse;
                let mut at = from;
                // The hits repeat in a circle, so a huge count takes no more steps.
                let times = if times > 1 { search.steps_for(buffer, times) } else { times };
                for _ in 0..times {
                    match search.next_after(buffer, at, forward) {
                        Some(hit) => at = hit,
                        None => {
                            self.message = Some(format!("E486: Pattern not found: {}", search.query));
                            return None;
                        }
                    }
                }
                (at, Reach::Exclusive)
            }
        })
    }

    /// Follows `whichwrap=hl` from NvChad, except inside an operator.
    fn step_left(&self, buffer: &Buffer, from: usize, times: usize, in_operator: bool) -> usize {
        let mut at = from;
        for _ in 0..times {
            let (line, column) = buffer.offset_to_point(at);
            if column > 0 {
                at = buffer.previous_boundary(at);
            } else if !in_operator && line > 0 {
                at = last_char(buffer, line - 1);
            } else {
                break;
            }
        }
        at
    }

    fn step_right(&self, buffer: &Buffer, from: usize, times: usize, in_operator: bool) -> usize {
        let mut at = from;
        for _ in 0..times {
            let line = buffer.offset_to_line(at);
            let end = buffer.line_end(line);
            if in_operator {
                if at >= end {
                    break;
                }
                at = buffer.next_boundary(at);
            } else if at < last_char(buffer, line) {
                at = buffer.next_boundary(at);
            } else if line + 1 < buffer.line_count() {
                at = buffer.line_start(line + 1);
            } else {
                break;
            }
        }
        at
    }

    fn vertical(&mut self, buffer: &Buffer, from: usize, delta: isize) -> usize {
        let (line, column) = buffer.offset_to_point(from);
        let goal = self.goal_column.unwrap_or_else(|| buffer.line_text(line)[..column].chars().count());
        self.goal_column = Some(goal);
        let target = (line as isize + delta).clamp(0, buffer.line_count() as isize - 1) as usize;
        let text = buffer.line_text(target);
        let index = text.char_indices().nth(goal).map(|(index, _)| index);
        match index {
            Some(index) => buffer.line_start(target) + index,
            None => last_char(buffer, target),
        }
    }

    fn apply_operator(
        &mut self,
        operator: Operator,
        range: Range<usize>,
        linewise: bool,
        state: &mut EditorState,
        effects: &mut Vec<Effect>,
    ) {
        self.reset_pending();
        let buffer = state.buffer();
        if linewise {
            let first = buffer.offset_to_line(range.start);
            let last = buffer.offset_to_line(range.end);
            match operator {
                Operator::Yank => {
                    effects.push(Effect::Yank(lines_text(buffer, first, last)));
                    let cursor = self.cursor(state);
                    let keep = !self.is_visual() && buffer.offset_to_line(cursor) == first;
                    let target = if keep { cursor } else { buffer.line_start(first) };
                    self.mode = Mode::Normal;
                    set_cursor(state, target);
                }
                Operator::Delete => {
                    effects.push(Effect::Yank(lines_text(buffer, first, last)));
                    let has_next = last + 1 < buffer.line_count();
                    let (start, end) = if has_next {
                        (buffer.line_start(first), buffer.line_start(last + 1))
                    } else if first > 0 {
                        (buffer.line_end(first - 1), buffer.line_end(last))
                    } else {
                        (0, buffer.line_end(last))
                    };
                    replace(state, start..end, "", start);
                    let line = first.min(state.buffer().line_count() - 1);
                    let target = state.buffer().first_non_whitespace(line);
                    set_cursor(state, target);
                }
                Operator::Change => {
                    effects.push(Effect::Yank(lines_text(buffer, first, last)));
                    let text = buffer.line_text(first);
                    let indent = text[..text.len() - text.trim_start().len()].to_string();
                    let start = buffer.line_start(first);
                    let end = buffer.line_end(last);
                    let cursor = start + indent.len();
                    replace(state, start..end, &indent, cursor);
                    self.mode = Mode::Insert;
                }
                Operator::Indent | Operator::Outdent => {
                    state.indent_lines(first..last + 1, operator == Operator::Outdent);
                    let target = state.buffer().first_non_whitespace(first);
                    set_cursor(state, target);
                }
            }
            return;
        }
        let text = buffer.slice(range.clone());
        match operator {
            Operator::Yank => {
                effects.push(Effect::Yank(text));
                set_cursor(state, range.start);
            }
            Operator::Delete => {
                if !text.is_empty() {
                    effects.push(Effect::Yank(text));
                }
                replace(state, range.clone(), "", range.start);
            }
            Operator::Change => {
                if !text.is_empty() {
                    effects.push(Effect::Yank(text));
                }
                replace(state, range.clone(), "", range.start);
                self.mode = Mode::Insert;
            }
            Operator::Indent | Operator::Outdent => {
                let first = buffer.offset_to_line(range.start);
                let last = buffer.offset_to_line(range.end);
                state.indent_lines(first..last + 1, operator == Operator::Outdent);
                let target = state.buffer().first_non_whitespace(first);
                set_cursor(state, target);
            }
        }
    }

    fn replace_chars(&mut self, ch: char, state: &mut EditorState) {
        let count = self.count.take().unwrap_or(1);
        let head = state.selections().primary().head;
        let buffer = state.buffer();
        let end_of_line = buffer.line_end(buffer.offset_to_line(head));
        let mut end = head;
        for _ in 0..count {
            if end >= end_of_line {
                return;
            }
            end = buffer.next_boundary(end);
        }
        let text = ch.to_string().repeat(count);
        let cursor = head + text.len() - ch.len_utf8();
        replace(state, head..end, &text, cursor);
    }

    fn join_lines(&mut self, joins: usize, state: &mut EditorState) {
        for _ in 0..joins {
            let head = state.selections().primary().head;
            let buffer = state.buffer();
            let line = buffer.offset_to_line(head);
            if line + 1 >= buffer.line_count() {
                return;
            }
            let end = buffer.line_end(line);
            let next_start = buffer.first_non_whitespace(line + 1);
            let next_empty = next_start == buffer.line_end(line + 1);
            let separator = if next_empty || end == buffer.line_start(line) { "" } else { " " };
            replace(state, end..next_start, separator, end);
        }
    }

    fn put(&mut self, before: bool, count: usize, state: &mut EditorState, input: &Input) {
        let Some(text) = input.clipboard.as_deref().filter(|text| !text.is_empty()) else {
            return;
        };
        let head = state.selections().primary().head;
        let buffer = state.buffer();
        let line = buffer.offset_to_line(head);
        let repeated = text.repeat(count.max(1));
        if text.ends_with('\n') {
            if before || line + 1 < buffer.line_count() {
                let target = if before { line } else { line + 1 };
                let at = buffer.line_start(target);
                replace(state, at..at, &repeated, at);
                let cursor = state.buffer().first_non_whitespace(target);
                set_cursor(state, cursor);
            } else {
                let at = buffer.line_end(line);
                let body = format!("\n{}", repeated.trim_end_matches('\n'));
                replace(state, at..at, &body, at + 1);
                let cursor = state.buffer().first_non_whitespace(line + 1);
                set_cursor(state, cursor);
            }
            return;
        }
        let at = if before { head } else { buffer.next_boundary(head).min(buffer.line_end(line)) };
        let cursor = if repeated.contains('\n') {
            at
        } else {
            at + repeated.len() - repeated.chars().next_back().map_or(0, char::len_utf8)
        };
        replace(state, at..at, &repeated, cursor);
    }

    fn put_over_selection(&mut self, state: &mut EditorState, input: &Input, effects: &mut Vec<Effect>) {
        let Some(text) = input.clipboard.clone() else {
            return;
        };
        let (range, linewise) = self.visual_range(state);
        self.mode = Mode::Normal;
        let buffer = state.buffer();
        if linewise {
            let first = buffer.offset_to_line(range.start);
            let last = buffer.offset_to_line(range.end);
            effects.push(Effect::Yank(lines_text(buffer, first, last)));
            let start = buffer.line_start(first);
            let end = buffer.line_end(last);
            replace(state, start..end, text.trim_end_matches('\n'), start);
        } else {
            effects.push(Effect::Yank(buffer.slice(range.clone())));
            let body = text.trim_end_matches('\n');
            let cursor = range.start + body.len().saturating_sub(body.chars().next_back().map_or(0, char::len_utf8));
            replace(state, range, body, cursor);
        }
    }

    fn undo_or_redo(&mut self, state: &mut EditorState, undo: bool) {
        let count = self.count.take().unwrap_or(1);
        for _ in 0..count {
            let done = if undo { state.undo() } else { state.redo() };
            if !done {
                break;
            }
        }
        // Later steps must not be merged with an undo that has already happened.
        self.undo_checkpoint = None;
        self.discard_recording = true;
        let start = state.selections().primary().start();
        set_cursor(state, start);
    }

    fn repeat_change(&mut self, count: usize, state: &mut EditorState, input: &Input, effects: &mut Vec<Effect>) {
        self.discard_recording = true;
        if self.replaying {
            return;
        }
        let keys = self.last_change.clone();
        self.replaying = true;
        for _ in 0..count {
            for key in &keys {
                self.dispatch(*key, state, input, effects);
            }
            clamp_to_last_char(state);
        }
        self.replaying = false;
    }

    fn search_word(&mut self, forward: bool, state: &mut EditorState) {
        self.count = None;
        let cursor = self.cursor(state);
        let range = state.buffer().word_range(cursor);
        if range.is_empty() {
            self.message = Some("E348: No string under cursor".to_string());
            return;
        }
        let search = Search { query: state.buffer().slice(range.clone()), whole_word: true, forward };
        let hit = search.next_after(state.buffer(), range.start, forward);
        self.search = Some(search);
        self.highlight = true;
        if let Some(hit) = hit {
            self.place(hit, state);
        }
    }

    // Helpers

    fn is_visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }

    fn reset_pending(&mut self) {
        self.pending = Pending::None;
        self.count = None;
        self.operator = None;
    }

    fn place(&mut self, offset: usize, state: &mut EditorState) {
        if self.is_visual() {
            self.visual_cursor = offset;
            self.sync_visual(state);
        } else {
            set_cursor(state, offset);
        }
    }

    fn leave_visual(&mut self, state: &mut EditorState) {
        let cursor = self.visual_cursor;
        self.mode = Mode::Normal;
        set_cursor(state, cursor);
    }

    fn enter_visual_from_selection(&mut self, state: &mut EditorState) {
        let selection = *state.selections().primary();
        let buffer = state.buffer();
        if selection.is_reversed() {
            self.visual_anchor = buffer.previous_boundary(selection.anchor);
            self.visual_cursor = selection.head;
        } else {
            self.visual_anchor = selection.anchor;
            self.visual_cursor = buffer.previous_boundary(selection.head);
        }
        self.mode = Mode::Visual;
        self.sync_visual(state);
    }

    /// The text range of the selection, and whether it covers whole lines.
    fn visual_range(&self, state: &EditorState) -> (Range<usize>, bool) {
        let buffer = state.buffer();
        let start = self.visual_anchor.min(self.visual_cursor);
        let end = self.visual_anchor.max(self.visual_cursor);
        if self.mode == Mode::VisualLine {
            let first = buffer.offset_to_line(start);
            let last = buffer.offset_to_line(end);
            (buffer.line_start(first)..buffer.line_end(last), true)
        } else {
            (start..buffer.next_boundary(end), false)
        }
    }

    fn sync_visual(&self, state: &mut EditorState) {
        let buffer = state.buffer();
        let selection = if self.mode == Mode::VisualLine {
            let first = buffer.offset_to_line(self.visual_anchor.min(self.visual_cursor));
            let last = buffer.offset_to_line(self.visual_anchor.max(self.visual_cursor));
            let end = if last + 1 < buffer.line_count() { buffer.line_start(last + 1) } else { buffer.line_end(last) };
            Selection::new(buffer.line_start(first), end)
        } else if self.visual_cursor >= self.visual_anchor {
            Selection::new(self.visual_anchor, buffer.next_boundary(self.visual_cursor))
        } else {
            Selection::new(buffer.next_boundary(self.visual_anchor), self.visual_cursor)
        };
        state.set_selections(vec![selection]);
    }
}

impl FindKind {
    fn reversed(self) -> Self {
        match self {
            FindKind::To => FindKind::BackTo,
            FindKind::Till => FindKind::BackTill,
            FindKind::BackTo => FindKind::To,
            FindKind::BackTill => FindKind::Till,
        }
    }
}

impl Search {
    fn matches_in_line(&self, text: &str) -> Vec<Range<usize>> {
        find_literal(text, &self.query, self.whole_word)
    }

    /// Rules out most lines without a copy of the text. Only ASCII is that simple:
    /// other characters can have a lowercase form in ASCII, like the Kelvin sign.
    fn may_match(&self, line: RopeSlice) -> bool {
        let Some(first) = self.query.chars().next().filter(char::is_ascii) else {
            return true;
        };
        let (lower, upper) = (first.to_ascii_lowercase() as u8, first.to_ascii_uppercase() as u8);
        line.chunks().any(|chunk| !chunk.is_ascii() || chunk.bytes().any(|byte| byte == lower || byte == upper))
    }

    fn steps_for(&self, buffer: &Buffer, times: usize) -> usize {
        let hits: usize = (0..buffer.line_count())
            .filter(|line| self.may_match(buffer.rope().line(*line)))
            .map(|line| self.matches_in_line(&buffer.line_text(line)).len())
            .sum();
        (times - 1) % hits.max(1) + 1
    }

    /// The next hit after `from`, wrapping around the document.
    fn next_after(&self, buffer: &Buffer, from: usize, forward: bool) -> Option<usize> {
        let lines = buffer.line_count();
        let (line, column) = buffer.offset_to_point(from);
        let start = |line: usize, range: &Range<usize>| buffer.line_start(line) + range.start;
        for step in 0..=lines {
            let index = if forward { (line + step) % lines } else { (line + lines - step % lines) % lines };
            if !self.may_match(buffer.rope().line(index)) {
                continue;
            }
            let hits = self.matches_in_line(&buffer.line_text(index));
            let hit = if forward {
                hits.iter().find(|hit| step > 0 || hit.start > column)
            } else {
                hits.iter().rev().find(|hit| step > 0 || hit.start < column)
            };
            if let Some(hit) = hit {
                return Some(start(index, hit));
            }
        }
        None
    }
}

/// What a key sequence after the leader does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Leader {
    Effect(Effect),
    Prefix,
    Unknown,
}

/// The `Space` mappings from the Neovim config.
pub fn leader_lookup(keys: &str) -> Leader {
    Leader::Effect(match keys {
        "w" => Effect::Save,
        "x" => Effect::Close,
        "f" => Effect::FindFiles,
        "g" => Effect::FindText,
        "p" => Effect::TogglePreview,
        "d" => Effect::ShowError,
        "r" => Effect::LookupDoi,
        "." => Effect::QuickFix,
        "ee" => Effect::ToggleTree,
        "ef" => Effect::RevealInTree,
        "e" => return Leader::Prefix,
        _ => return Leader::Unknown,
    })
}

/// Literal hits in one line, ignoring case unless the query has an uppercase letter.
pub fn find_literal(text: &str, query: &str, whole_word: bool) -> Vec<Range<usize>> {
    let query: Vec<char> = query.chars().collect();
    if query.is_empty() {
        return Vec::new();
    }
    let ignore_case = !query.iter().any(|ch| ch.is_uppercase());
    let same = |left: char, right: char| {
        left == right || (ignore_case && left.to_lowercase().eq(right.to_lowercase()))
    };
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut hits = Vec::new();
    let mut index = 0;
    while index + query.len() <= chars.len() {
        let found = query.iter().enumerate().all(|(offset, ch)| same(chars[index + offset].1, *ch));
        let bounded = !whole_word
            || ((index == 0 || !is_word_char(chars[index - 1].1))
                && chars.get(index + query.len()).is_none_or(|(_, ch)| !is_word_char(*ch)));
        if found && bounded {
            let end = chars.get(index + query.len()).map_or(text.len(), |(offset, _)| *offset);
            hits.push(chars[index].0..end);
            index += query.len();
        } else {
            index += 1;
        }
    }
    hits
}

fn motion_for(ch: char) -> Option<Motion> {
    Some(match ch {
        'h' => Motion::Left,
        'l' => Motion::Right,
        'k' => Motion::Up,
        'j' => Motion::Down,
        'w' => Motion::WordStart { big: false },
        'W' => Motion::WordStart { big: true },
        'e' => Motion::WordEnd { big: false },
        'E' => Motion::WordEnd { big: true },
        'b' => Motion::WordBack { big: false },
        'B' => Motion::WordBack { big: true },
        '0' => Motion::LineStart,
        '^' => Motion::FirstNonBlank,
        '$' => Motion::LineEnd,
        'G' => Motion::DocumentEnd,
        '}' => Motion::ParagraphNext,
        '{' => Motion::ParagraphPrevious,
        '%' => Motion::MatchBracket,
        ';' => Motion::RepeatFind { reverse: false },
        ',' => Motion::RepeatFind { reverse: true },
        'n' => Motion::SearchNext { reverse: false },
        'N' => Motion::SearchNext { reverse: true },
        _ => return None,
    })
}

fn set_cursor(state: &mut EditorState, offset: usize) {
    state.set_selections(vec![Selection::cursor(offset)]);
}

fn replace(state: &mut EditorState, range: Range<usize>, text: &str, cursor: usize) {
    state.replace_range(range, text, Selection::cursor(cursor));
}

fn last_char(buffer: &Buffer, line: usize) -> usize {
    let end = buffer.line_end(line);
    if end == buffer.line_start(line) { end } else { buffer.previous_boundary(end) }
}

fn clamp_to_last_char(state: &mut EditorState) {
    let selection = *state.selections().primary();
    let buffer = state.buffer();
    let line = buffer.offset_to_line(selection.head);
    let target = selection.head.min(last_char(buffer, line));
    if target != selection.head || state.selections().len() > 1 || !selection.is_empty() {
        set_cursor(state, target);
    }
}

fn lines_text(buffer: &Buffer, first: usize, last: usize) -> String {
    let mut text = buffer.slice(buffer.line_start(first)..buffer.line_end(last));
    text.push('\n');
    text
}

fn repeat(from: usize, times: usize, mut step: impl FnMut(usize) -> usize) -> usize {
    (0..times).fold(from, |at, _| step(at))
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punctuation,
}

fn class(ch: char, big: bool) -> Class {
    if ch.is_whitespace() {
        Class::Space
    } else if big || is_word_char(ch) {
        Class::Word
    } else {
        Class::Punctuation
    }
}

fn is_empty_line_at(buffer: &Buffer, offset: usize) -> bool {
    let line = buffer.offset_to_line(offset);
    buffer.line_start(line) == buffer.line_end(line)
}

fn next_word_start(buffer: &Buffer, from: usize, big: bool) -> usize {
    let mut at = from;
    if let Some(first) = buffer.char_at(at) {
        let kind = class(first, big);
        if kind != Class::Space {
            while let Some(ch) = buffer.char_at(at).filter(|ch| class(*ch, big) == kind) {
                at += ch.len_utf8();
            }
        }
    }
    while let Some(ch) = buffer.char_at(at) {
        if !ch.is_whitespace() {
            break;
        }
        at += ch.len_utf8();
        // An empty line counts as a word.
        if ch == '\n' && at < buffer.len() && is_empty_line_at(buffer, at) {
            break;
        }
    }
    at
}

fn word_end(buffer: &Buffer, from: usize, big: bool) -> usize {
    let mut at = from;
    if let Some(ch) = buffer.char_at(at) {
        at += ch.len_utf8();
    }
    while let Some(ch) = buffer.char_at(at).filter(|ch| ch.is_whitespace()) {
        at += ch.len_utf8();
    }
    let Some(first) = buffer.char_at(at) else {
        return from;
    };
    let kind = class(first, big);
    let mut last = at;
    while let Some(ch) = buffer.char_at(at).filter(|ch| class(*ch, big) == kind) {
        last = at;
        at += ch.len_utf8();
    }
    last
}

fn word_back(buffer: &Buffer, from: usize, big: bool) -> usize {
    let mut at = from;
    while let Some(ch) = buffer.char_before(at).filter(|ch| ch.is_whitespace()) {
        at -= ch.len_utf8();
        if ch == '\n' && is_empty_line_at(buffer, at) && at != from {
            return buffer.clip(at);
        }
    }
    let Some(first) = buffer.char_before(at) else {
        return at;
    };
    let kind = class(first, big);
    while let Some(ch) = buffer.char_before(at).filter(|ch| class(*ch, big) == kind) {
        at -= ch.len_utf8();
    }
    at
}

fn paragraph(buffer: &Buffer, from: usize, forward: bool) -> usize {
    let line = buffer.offset_to_line(from);
    let is_blank = |line: usize| buffer.line_text(line).trim().is_empty();
    if forward {
        let mut index = line;
        while index + 1 < buffer.line_count() && is_blank(index) {
            index += 1;
        }
        while index + 1 < buffer.line_count() {
            index += 1;
            if is_blank(index) {
                return buffer.line_start(index);
            }
        }
        buffer.line_end(buffer.line_count() - 1)
    } else {
        let mut index = line;
        while index > 0 && is_blank(index) {
            index -= 1;
        }
        while index > 0 {
            index -= 1;
            if is_blank(index) {
                return buffer.line_start(index);
            }
        }
        0
    }
}

fn pair_of(ch: u8) -> Option<(u8, u8, bool)> {
    Some(match ch {
        b'(' => (b'(', b')', true),
        b')' => (b'(', b')', false),
        b'[' => (b'[', b']', true),
        b']' => (b'[', b']', false),
        b'{' => (b'{', b'}', true),
        b'}' => (b'{', b'}', false),
        _ => return None,
    })
}

fn match_bracket(buffer: &Buffer, from: usize) -> Option<usize> {
    let line = buffer.offset_to_line(from);
    let end = buffer.line_end(line);
    let position = (from..end).find(|index| pair_of(buffer.rope().byte(*index)).is_some())?;
    let (open, close, opening) = pair_of(buffer.rope().byte(position))?;
    if opening {
        closing_after(buffer, position, open, close)
    } else {
        opening_before(buffer, position, open, close)
    }
}

/// The bracket that closes the one at `start`, without looking further than it is.
fn closing_after(buffer: &Buffer, start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (index, byte) in buffer.rope().bytes_at(start).enumerate() {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(start + index);
            }
        }
    }
    None
}

/// The bracket around `end` that is still open there, going backwards from it.
fn opening_before(buffer: &Buffer, end: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 1usize;
    let mut index = end;
    for byte in buffer.rope().bytes_at(end).reversed() {
        index -= 1;
        if byte == close {
            depth += 1;
        } else if byte == open {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn find_on_line(
    buffer: &Buffer,
    from: usize,
    kind: FindKind,
    target: char,
    times: usize,
    repeat: bool,
) -> Option<(usize, Reach)> {
    let line = buffer.offset_to_line(from);
    let start = buffer.line_start(line);
    let text = buffer.line_text(line);
    let column = (from - start).min(text.len());
    let forward = matches!(kind, FindKind::To | FindKind::Till);
    let till = matches!(kind, FindKind::Till | FindKind::BackTill);
    let hits: Vec<usize> = text.char_indices().filter(|(_, ch)| *ch == target).map(|(index, _)| index).collect();
    let next_char = text[column..].chars().next().map_or(column, |ch| column + ch.len_utf8());
    let previous_char = text[..column].char_indices().next_back().map_or(0, |(index, _)| index);
    // A repeated `t` must not stick to the target it already stands next to.
    let skip_adjacent = till && repeat;
    let candidates: Vec<usize> = if forward {
        hits.into_iter().filter(|hit| *hit > column && !(skip_adjacent && *hit == next_char)).collect()
    } else {
        hits.into_iter().rev().filter(|hit| *hit < column && !(skip_adjacent && *hit == previous_char)).collect()
    };
    let hit = *candidates.get(times - 1)?;
    let offset = if !till {
        hit
    } else if forward {
        text[..hit].char_indices().next_back().map_or(0, |(index, _)| index)
    } else {
        text[hit..].chars().next().map_or(hit, |ch| hit + ch.len_utf8())
    };
    let reach = if forward { Reach::Inclusive } else { Reach::Exclusive };
    Some((start + offset, reach))
}

fn object_range(buffer: &Buffer, cursor: usize, object: char, around: bool) -> Option<Range<usize>> {
    match object {
        'w' | 'W' => word_object(buffer, cursor, object == 'W', around),
        '(' | ')' | 'b' => pair_object(buffer, cursor, b'(', b')', around),
        '{' | '}' | 'B' => pair_object(buffer, cursor, b'{', b'}', around),
        '[' | ']' => pair_object(buffer, cursor, b'[', b']', around),
        '"' | '\'' | '`' | '$' => quote_object(buffer, cursor, object, around),
        _ => None,
    }
}

fn word_object(buffer: &Buffer, cursor: usize, big: bool, around: bool) -> Option<Range<usize>> {
    let line = buffer.offset_to_line(cursor);
    let base = buffer.line_start(line);
    let text = buffer.line_text(line);
    let column = (cursor - base).min(text.len());
    let first = text[column..].chars().next()?;
    let kind = class(first, big);
    let same = |ch: char| class(ch, big) == kind;
    let mut start = column;
    for (index, ch) in text[..column].char_indices().rev() {
        if !same(ch) {
            break;
        }
        start = index;
    }
    let mut end = column;
    for (index, ch) in text[column..].char_indices() {
        if !same(ch) {
            break;
        }
        end = column + index + ch.len_utf8();
    }
    if around {
        let trailing = text[end..].len() - text[end..].trim_start().len();
        if trailing > 0 && kind != Class::Space {
            end += trailing;
        } else {
            let before = &text[..start];
            start = before.trim_end().len();
        }
    }
    Some(base + start..base + end)
}

fn pair_object(buffer: &Buffer, cursor: usize, open: u8, close: u8, around: bool) -> Option<Range<usize>> {
    let start = if buffer.rope().get_byte(cursor) == Some(open) {
        cursor
    } else {
        opening_before(buffer, cursor, open, close)?
    };
    let end = closing_after(buffer, start, open, close)?;
    Some(if around { start..end + 1 } else { start + 1..end })
}

fn quote_object(buffer: &Buffer, cursor: usize, quote: char, around: bool) -> Option<Range<usize>> {
    let line = buffer.offset_to_line(cursor);
    let base = buffer.line_start(line);
    let text = buffer.line_text(line);
    let column = cursor - base;
    let quotes: Vec<usize> = text
        .char_indices()
        .filter(|(index, ch)| *ch == quote && (*index == 0 || !text[..*index].ends_with('\\')))
        .map(|(index, _)| index)
        .collect();
    let (start, end) = quotes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (pair[0], pair[1]))
        .find(|(_, end)| column <= *end)?;
    Some(if around { base + start..base + end + quote.len_utf8() } else { base + start + quote.len_utf8()..base + end })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Language;

    struct Fixture {
        vim: Vim,
        state: EditorState,
        clipboard: Option<String>,
        effects: Vec<Effect>,
    }

    /// `|` in `text` marks the cursor.
    fn fixture(text: &str) -> Fixture {
        let cursor = text.find('|').expect("text marks the cursor");
        let mut state = EditorState::new(&text.replacen('|', "", 1), Language::Plain);
        state.set_selections(vec![Selection::cursor(cursor)]);
        Fixture { vim: Vim::default(), state, clipboard: None, effects: Vec::new() }
    }

    #[test]
    fn snippet_from_outside_starts_insert_and_stays_out_of_repeat() {
        let mut fixture = fixture("|x");
        fixture.keys("ia<esc>");
        fixture.vim.enter_insert(&fixture.state);
        fixture.state.insert_snippet("{${}}{${}}", false);
        assert_eq!(fixture.vim.mode(), Mode::Insert);
        fixture.state.type_text("p");
        fixture.vim.record_insert(Key::Char('p'));
        fixture.keys("<esc>");
        assert_eq!(fixture.vim.mode(), Mode::Normal);
        assert!(!fixture.state.next_snippet_slot());
        fixture.keys("0.");
        assert_eq!(fixture.state.buffer().to_string(), "a{p}{}ax");
    }

    #[test]
    fn repeat_types_an_accepted_suggestion() {
        let mut fixture = fixture("|x");
        fixture.keys("i");
        for ch in ['\\', 'f', 'r'] {
            fixture.state.type_text(ch.encode_utf8(&mut [0; 4]));
            fixture.vim.record_insert(Key::Char(ch));
        }
        fixture.vim.record_completion("\\fr", "\\frac{}{}");
        fixture.state.complete_prefix(0..3, "\\frac{${}}{${}}");
        assert_eq!(fixture.state.buffer().to_string(), "\\frac{}{}x");
        fixture.keys("<esc>0.");
        assert_eq!(fixture.state.buffer().to_string(), "\\frac{}{}\\frac{}{}x");
    }

    #[test]
    fn tab_in_normal_mode_still_goes_to_the_tree() {
        let mut fixture = fixture("|x");
        let effects = fixture.vim.key(Key::Tab, &mut fixture.state, &Input::default());
        assert!(effects.contains(&Effect::FocusTree));
    }

    #[test]
    fn tab_in_insert_mode_walks_the_snippet() {
        let mut fixture = fixture("|");
        fixture.vim.enter_insert(&fixture.state);
        fixture.state.insert_snippet("{${}}[${}]", false);
        fixture.keys("<tab>");
        assert_eq!(fixture.state.selections().primary().head, "{}[".len());
    }

    impl Fixture {
        /// Plain characters are keys. `<esc>`, `<cr>`, `<tab>`, `<bs>` and `<c-x>` name others.
        fn keys(&mut self, keys: &str) -> &mut Self {
            let mut rest = keys;
            while let Some(ch) = rest.chars().next() {
                let (key, used) = if let (true, Some(end)) = (ch == '<', rest.find('>')) {
                    let name = &rest[1..end];
                    let key = match name {
                        "esc" => Key::Esc,
                        "cr" => Key::Enter,
                        "tab" => Key::Tab,
                        "bs" => Key::Backspace,
                        _ if name.starts_with("c-") => Key::Ctrl(name.chars().nth(2).unwrap()),
                        _ => Key::Char('<'),
                    };
                    match key {
                        Key::Char(_) => (key, 1),
                        _ => (key, end + 1),
                    }
                } else {
                    (Key::Char(ch), ch.len_utf8())
                };
                rest = &rest[used..];
                let input = Input { clipboard: self.clipboard.clone(), half_page: 2 };
                let effects = self.vim.key(key, &mut self.state, &input);
                for effect in effects {
                    if let Effect::Yank(text) = &effect {
                        self.clipboard = Some(text.clone());
                    }
                    self.effects.push(effect);
                }
            }
            self
        }

        fn text(&self) -> String {
            self.state.buffer().to_string()
        }

        /// The text with `|` at the block cursor.
        fn shown(&self) -> String {
            let mut text = self.text();
            text.insert(self.vim.cursor(&self.state), '|');
            text
        }
    }

    fn check(before: &str, keys: &str, after: &str) {
        let mut fixture = fixture(before);
        fixture.keys(keys);
        assert_eq!(fixture.shown(), after, "keys: {keys}");
    }

    #[test]
    fn moves_with_hjkl_and_keeps_the_column() {
        check("|abc\ndef", "l", "a|bc\ndef");
        check("a|bc\ndef", "h", "|abc\ndef");
        check("ab|c\nd\nefg", "jj", "abc\nd\nef|g");
        check("abc\nd|ef", "k", "a|bc\ndef");
    }

    #[test]
    fn h_and_l_wrap_like_the_nvchad_config() {
        check("ab|c\ndef", "l", "abc\n|def");
        check("abc\n|def", "h", "ab|c\ndef");
        check("ab|c", "l", "ab|c");
    }

    #[test]
    fn dollar_keeps_the_line_end_when_moving_down() {
        check("|ab\nabcd\na", "$j", "ab\nabc|d\na");
    }

    #[test]
    fn counts_repeat_motions() {
        check("|one two three four", "3w", "one two three |four");
        check("|a\nb\nc\nd", "2j", "a\nb\n|c\nd");
        check("|a\nb\nc\nd", "3G", "a\nb\n|c\nd");
        check("a\nb\nc\n|d", "gg", "|a\nb\nc\nd");
        check("|a\nb", "G", "a\n|b");
    }

    #[test]
    fn word_motions_split_punctuation_and_cross_lines() {
        check("|foo.bar baz", "w", "foo|.bar baz");
        check("|foo.bar baz", "W", "foo.bar |baz");
        check("|foo bar", "e", "fo|o bar");
        check("foo ba|r", "b", "foo |bar");
        check("fo|o\n\nbar", "w", "foo\n|\nbar");
        check("|\\frac{a}", "w", "\\|frac{a}");
    }

    #[test]
    fn motions_handle_umlauts() {
        check("|äöü x", "l", "ä|öü x");
        check("|äöü x", "$", "äöü |x");
        check("|äöü x", "w", "äöü |x");
        check("|äb\näb", "lj", "äb\nä|b");
        check("|äb", "x", "|b");
        check("|ä", "rü", "|ü");
    }

    #[test]
    fn stays_on_the_last_char_and_handles_empty_lines() {
        check("|", "x", "|");
        check("|", "dd", "|");
        check("|", "$", "|");
        check("|\n", "j", "\n|");
        check("|a", "$l", "|a");
        check("|", "ihi<esc>", "h|i");
    }

    #[test]
    fn find_and_repeat_on_a_line() {
        check("|a,b,c", "f,", "a|,b,c");
        check("|a,b,c", "f,;", "a,b|,c");
        check("|a,b,c", "2f,", "a,b|,c");
        check("|a,b,c", "t,;", "a,|b,c");
        check("a,b,|c", "F,", "a,b|,c");
        check("a,b,|c", "F,,", "a,b|,c");
    }

    #[test]
    fn paragraph_and_bracket_motions() {
        check("|a\nb\n\nc", "}", "a\nb\n|\nc");
        check("a\n\nb\n|c", "{", "a\n|\nb\nc");
        check("|f(a, (b))", "%", "f(a, (b)|)");
        check("f(a, (b)|)", "%", "f|(a, (b))");
    }

    #[test]
    fn dd_and_counted_operators() {
        check("a\n|b\nc", "dd", "a\n|c");
        check("a\nb\n|c", "dd", "a\n|b");
        check("|a\nb\nc\nd", "2dd", "|c\nd");
        check("|a\nb\nc\nd", "d2j", "|d");
        check("|a b c d", "2dw", "|c d");
        check("|a b c d", "d2w", "|c d");
        check("|a b c", "2d2w", "|");
        check("|only", "dd", "|");
    }

    #[test]
    fn dw_stays_on_the_line_and_d_dollar_keeps_the_newline() {
        check("foo |bar\nbaz", "dw", "foo| \nbaz");
        check("foo |bar\nbaz", "d$", "foo| \nbaz");
        check("foo |bar\nbaz", "D", "foo| \nbaz");
        check("|foo bar", "de", "| bar");
        check("|foo,bar", "dt,", "|,bar");
        check("|foo,bar", "df,", "|bar");
    }

    #[test]
    fn x_stays_on_the_line() {
        check("a|b\nc", "x", "|a\nc");
        check("|abc", "2x", "|c");
        check("ab|c", "X", "a|c");
    }

    #[test]
    fn change_commands_enter_insert_mode() {
        check("|foo bar", "cwx<esc>", "|x bar");
        check("  |foo\nbar", "ccx<esc>", "  |x\nbar");
        check("|foo", "sx<esc>", "|xoo");
        check("foo |bar", "Cx<esc>", "foo |x");
        check("  |foo", "Sx<esc>", "  |x");
    }

    #[test]
    fn insert_commands_place_the_cursor() {
        check("a|b", "iX<esc>", "a|Xb");
        check("a|b", "aX<esc>", "ab|X");
        check("  a|b", "IX<esc>", "  |Xab");
        check("|ab", "AX<esc>", "ab|X");
        check("|a\nb", "oX<esc>", "a\n|X\nb");
        check("a\n|b", "OX<esc>", "a\n|X\nb");
        check("  |a", "oX<esc>", "  a\n  |X");
        check("  |a", "OX<esc>", "  |X\n  a");
    }

    #[test]
    fn text_objects() {
        check("a f|oo b", "diw", "a | b");
        check("a f|oo b", "daw", "a |b");
        check("a b f|oo", "daw", "a |b");
        check("f(a, |b)", "di(", "f(|)");
        check("f(a, |b)", "da(", "|f");
        check("x{a {b} |c}", "ci{y<esc>", "x{|y}");
        check("a[|b]", "di]", "a[|]");
        check("\"a|b\" c", "di\"", "\"|\" c");
        check("x $a|b$ y", "di$", "x $|$ y");
        check("x $a|b$ y", "da$", "x | y");
    }

    #[test]
    fn missing_text_object_changes_nothing() {
        check("|abc", "di(", "|abc");
        let mut fixture = fixture("|abc");
        fixture.keys("di(");
        assert_eq!(fixture.vim.mode(), Mode::Normal);
        assert_eq!(fixture.vim.pending_keys(), "");
    }

    #[test]
    fn yank_and_put() {
        check("|ab\ncd", "yyp", "ab\n|ab\ncd");
        check("a|b\ncd", "yyjP", "ab\n|ab\ncd");
        check("|ab", "yyp", "ab\n|ab");
        check("|ab cd", "ywP", "ab| ab cd");
        check("|ab", "ylp", "a|ab");
        check("|ab", "yl2p", "aa|ab");
        check("a|b\ncd", "ddp", "cd\n|ab");
    }

    #[test]
    fn yank_keeps_the_cursor_on_the_line_and_goes_to_the_clipboard() {
        let mut fixture = fixture("a|b\nc");
        fixture.keys("yy");
        assert_eq!(fixture.clipboard.as_deref(), Some("ab\n"));
        assert_eq!(fixture.shown(), "a|b\nc");
    }

    #[test]
    fn put_uses_text_from_other_programs() {
        let mut fixture = fixture("|ab");
        fixture.clipboard = Some("xy".to_string());
        fixture.keys("p");
        assert_eq!(fixture.shown(), "ax|yb");
    }

    #[test]
    fn replace_and_join() {
        check("|abc", "rx", "|xbc");
        check("|abc", "2rx", "x|xc");
        check("|abc", "9rx", "|abc");
        check("|a\n  b\nc", "J", "a| b\nc");
        check("|a\n\nb", "J", "|a\nb");
        check("|a\nb\nc", "3J", "a b| c");
    }

    #[test]
    fn indent_operators() {
        check("|a\nb", ">>", "  |a\nb");
        check("|a\nb", ">j", "  |a\n  b");
        check("  |a", "<<", "|a");
        check("|a\nb\nc", "2>>", "  |a\n  b\nc");
    }

    #[test]
    fn visual_mode_covers_the_cursor_char() {
        check("|abc def", "vld", "|c def");
        check("ab|c", "vhd", "|a");
        check("|abc", "vly", "|abc");
        check("|abc", "vlcX<esc>", "|Xc");
        let mut fixture = fixture("|abc");
        fixture.keys("vl");
        assert_eq!(fixture.vim.mode(), Mode::Visual);
        assert_eq!(fixture.state.selected_text().as_deref(), Some("ab"));
        assert_eq!(fixture.shown(), "a|bc");
    }

    #[test]
    fn visual_line_works_on_whole_lines() {
        check("a|b\ncd\nef", "Vjd", "|ef");
        check("a\n|b\nc", "Vkd", "|c");
        check("|a\nb", "Vj>", "  |a\n  b");
        check("a\n|b", "Vd", "|a");
        let mut fixture = fixture("|a\nb");
        fixture.keys("Vy");
        assert_eq!(fixture.clipboard.as_deref(), Some("a\n"));
    }

    #[test]
    fn visual_text_object_and_swap() {
        let mut fixture = fixture("(a|b)");
        fixture.keys("vi(");
        assert_eq!(fixture.state.selected_text().as_deref(), Some("ab"));
        fixture.keys("<esc>");
        assert_eq!(fixture.vim.mode(), Mode::Normal);
        check("|abc", "vlod", "|c");
    }

    #[test]
    fn visual_put_replaces_and_yanks_the_old_text() {
        let mut fixture = fixture("|ab c");
        fixture.clipboard = Some("xy".to_string());
        fixture.keys("vlp");
        assert_eq!(fixture.text(), "xy c");
        assert_eq!(fixture.clipboard.as_deref(), Some("ab"));
    }

    #[test]
    fn mouse_selection_becomes_visual_mode() {
        let mut fixture = fixture("|abc def");
        fixture.state.set_selections(vec![Selection::new(0, 3)]);
        fixture.vim.after_mouse(&mut fixture.state);
        assert_eq!(fixture.vim.mode(), Mode::Visual);
        fixture.keys("d");
        assert_eq!(fixture.text(), " def");
    }

    #[test]
    fn click_past_the_line_end_lands_on_the_last_char() {
        let mut fixture = fixture("|abc");
        fixture.state.set_selections(vec![Selection::cursor(3)]);
        fixture.vim.after_mouse(&mut fixture.state);
        assert_eq!(fixture.shown(), "ab|c");
    }

    #[test]
    fn one_undo_per_insert_session() {
        let mut fixture = fixture("|");
        fixture.keys("ione<cr>two<cr>three<esc>");
        assert_eq!(fixture.text(), "one\ntwo\nthree");
        fixture.keys("u");
        assert_eq!(fixture.shown(), "|");
        fixture.keys("<c-r>");
        assert_eq!(fixture.text(), "one\ntwo\nthree");
    }

    #[test]
    fn separate_commands_undo_separately_and_leave_no_selection() {
        let mut fixture = fixture("|a\nb\nc");
        fixture.keys("dddd");
        assert_eq!(fixture.text(), "c");
        fixture.keys("u");
        assert_eq!(fixture.text(), "b\nc");
        assert!(fixture.state.selections().primary().is_empty());
        assert_eq!(fixture.vim.mode(), Mode::Normal);
        fixture.keys("u");
        assert_eq!(fixture.text(), "a\nb\nc");
        check("|a\nb\nc", "dddd2u", "|a\nb\nc");
    }

    #[test]
    fn o_with_text_undoes_as_one_step() {
        let mut fixture = fixture("|a");
        fixture.keys("oxy<esc>");
        assert_eq!(fixture.text(), "a\nxy");
        fixture.keys("u");
        assert_eq!(fixture.text(), "a");
    }

    #[test]
    fn dot_repeats_the_last_change() {
        check("|a\nb\nc\nd", "dd.", "|c\nd");
        check("|a b c d", "dw.", "|c d");
        check("|a\nb", "Ax<esc>j.", "ax\nb|x");
        check("|foo foo", "cwbar<esc>w.", "bar ba|r");
        check("|a\nb\nc\nd", "dd2.", "|d");
        check("|ab", "x.", "|");
    }

    #[test]
    fn dot_ignores_motions_undo_and_yank() {
        check("|a b c", "xwyl.", " | c");
        let mut fixture = fixture("|abc");
        fixture.keys("xu.");
        assert_eq!(fixture.text(), "bc");
    }

    #[test]
    fn dot_replays_text_recorded_from_the_input_handler() {
        let mut fixture = fixture("|a\nb");
        fixture.keys("A");
        // In the app, typed text reaches the state through the input handler.
        fixture.state.type_text("x");
        fixture.vim.record_insert(Key::Char('x'));
        fixture.keys("<esc>j.");
        assert_eq!(fixture.text(), "ax\nbx");
    }

    #[test]
    fn brackets_are_found_across_rope_chunks() {
        // Long enough that the rope splits it, with characters of several bytes.
        let filler = "äöü text ".repeat(800);
        let text = format!("start ({filler}(inner){filler}) end {{ x }}");
        let open = text.find('(').unwrap();
        let close = text.rfind(')').unwrap();
        let buffer = Buffer::new(&text);
        assert_eq!(match_bracket(&buffer, open), Some(close));
        assert_eq!(match_bracket(&buffer, close), Some(open));
        assert_eq!(match_bracket(&buffer, 0), Some(close));
        let middle = text.len() / 4;
        let middle = (middle..).find(|index| text.is_char_boundary(*index)).unwrap();
        assert_eq!(pair_object(&buffer, middle, b'(', b')', true), Some(open..close + 1));
        assert_eq!(pair_object(&buffer, middle, b'(', b')', false), Some(open + 1..close));
        assert_eq!(pair_object(&buffer, open, b'(', b')', true), Some(open..close + 1));
        // Outside of the brackets.
        assert_eq!(pair_object(&buffer, 1, b'(', b')', true), None);
        assert_eq!(pair_object(&buffer, text.len(), b'{', b'}', true), None);
        // No partner.
        let buffer = Buffer::new("a ( b");
        assert_eq!(match_bracket(&buffer, 0), None);
        let buffer = Buffer::new("a ) b");
        assert_eq!(match_bracket(&buffer, 0), None);
    }

    #[test]
    fn search_counts_go_round_and_filter_keeps_all_hits() {
        // Three hits, so this count is one step, and it has to return at once.
        check("|Kelvin K\nnothing\nkelvin", "/k<cr>", "Kelvin |K\nnothing\nkelvin");
        check("|Kelvin K\nnothing\nkelvin", "/k<cr>300000001n", "Kelvin K\nnothing\n|kelvin");
        // The lowercase of the Kelvin sign is k, which only the slow path knows.
        check("|abc\nnope\na\u{212a}c", "/k<cr>", "abc\nnope\na|\u{212a}c");
        let search = |query: &str| Search { query: query.to_string(), forward: true, whole_word: false };
        let rope = |text: &str| ropey::Rope::from_str(text);
        assert!(!search("k").may_match(rope("abc").slice(..)));
        assert!(search("k").may_match(rope("aKc").slice(..)));
        assert!(search("\u{212a}").may_match(rope("abc").slice(..)));
    }

    #[test]
    fn search_is_literal_with_smartcase_and_wraps() {
        check("|foo Bar\nbar", "/bar<cr>", "foo |Bar\nbar");
        check("|foo Bar\nbar", "/Bar<cr>", "foo |Bar\nbar");
        check("|foo Bar\nbar", "/bar<cr>n", "foo Bar\n|bar");
        check("|foo Bar\nbar", "/bar<cr>nn", "foo |Bar\nbar");
        check("|foo Bar\nbar", "/bar<cr>N", "foo Bar\n|bar");
        check("foo a\nbar |a", "?foo<cr>", "|foo a\nbar a");
        check("|a.c abc", "/.c<cr>", "a|.c abc");
    }

    #[test]
    fn search_moves_while_typing_and_esc_returns() {
        let mut fixture = fixture("|foo bar");
        fixture.keys("/bar");
        assert_eq!(fixture.vim.mode(), Mode::Command);
        assert_eq!(fixture.vim.command_line().as_deref(), Some("/bar"));
        assert_eq!(fixture.shown(), "foo |bar");
        fixture.keys("<esc>");
        assert_eq!(fixture.shown(), "|foo bar");
        assert_eq!(fixture.vim.mode(), Mode::Normal);
    }

    #[test]
    fn search_reports_a_missing_pattern() {
        let mut fixture = fixture("|foo");
        fixture.keys("/xyz<cr>");
        assert_eq!(fixture.vim.command_line().as_deref(), Some("E486: Pattern not found: xyz"));
        assert_eq!(fixture.shown(), "|foo");
    }

    #[test]
    fn star_finds_the_whole_word_and_esc_clears_highlights() {
        check("|foo foobar foo", "*", "foo foobar |foo");
        let mut fixture = fixture("|foo foo");
        fixture.keys("*");
        assert_eq!(fixture.vim.highlights_in_line("foo foo"), vec![0..3, 4..7]);
        fixture.keys("<esc>");
        assert!(fixture.vim.highlights_in_line("foo foo").is_empty());
    }

    #[test]
    fn highlights_use_byte_offsets_with_umlauts() {
        let mut fixture = fixture("|äfoo");
        fixture.keys("/FOO<esc>");
        fixture.keys("/foo<cr>");
        assert_eq!(fixture.vim.highlights_in_line("äfoo FOO"), vec![2..5, 6..9]);
    }

    #[test]
    fn commands_produce_effects() {
        for (keys, effect) in [
            (":w<cr>", Effect::Save),
            (":q<cr>", Effect::Close),
            (":q!<cr>", Effect::CloseWithoutSaving),
            (":wq<cr>", Effect::SaveAndClose),
            (":x<cr>", Effect::SaveAndClose),
        ] {
            let mut fixture = fixture("|a");
            fixture.keys(keys);
            assert_eq!(fixture.effects, vec![effect], "{keys}");
            assert_eq!(fixture.vim.mode(), Mode::Normal);
        }
        check("|a\nb\nc", ":3<cr>", "a\nb\n|c");
        check("|a\nb\nc", ":99<cr>", "a\nb\n|c");
    }

    #[test]
    fn unknown_command_reports_an_error_and_backspace_leaves_the_line() {
        let mut fixture = fixture("|a");
        fixture.keys(":foo<cr>");
        assert_eq!(fixture.vim.command_line().as_deref(), Some("E492: Not an editor command: foo"));
        fixture.keys(":<bs>");
        assert_eq!(fixture.vim.mode(), Mode::Normal);
        assert_eq!(fixture.vim.command_line(), None);
    }

    #[test]
    fn leader_keys_follow_the_nvim_config() {
        for (keys, effect) in [
            (" w", Effect::Save),
            (" x", Effect::Close),
            (" f", Effect::FindFiles),
            (" g", Effect::FindText),
            (" p", Effect::TogglePreview),
            (" d", Effect::ShowError),
            (" r", Effect::LookupDoi),
            (" .", Effect::QuickFix),
            (" ee", Effect::ToggleTree),
            (" ef", Effect::RevealInTree),
            ("]d", Effect::NextError),
            ("[d", Effect::PreviousError),
            ("H", Effect::PreviousTab),
            ("L", Effect::NextTab),
            ("<tab>", Effect::FocusTree),
        ] {
            let mut fixture = fixture("|a");
            fixture.keys(keys);
            assert_eq!(fixture.effects, vec![effect], "{keys}");
            assert_eq!(fixture.text(), "a", "{keys}");
        }
    }

    #[test]
    fn leader_shows_pending_keys_and_esc_cancels() {
        let mut fixture = fixture("|a");
        fixture.keys(" e");
        assert_eq!(fixture.vim.pending_keys(), "Space e");
        fixture.keys("<esc>");
        assert_eq!(fixture.vim.pending_keys(), "");
        assert!(fixture.effects.is_empty());
        fixture.keys(" q");
        assert!(fixture.effects.is_empty());
        assert_eq!(fixture.vim.pending_keys(), "");
    }

    #[test]
    fn half_page_moves_and_scrolls() {
        let mut fixture = fixture("|a\nb\nc\nd\ne");
        fixture.keys("<c-d>");
        assert_eq!(fixture.effects, vec![Effect::Scroll(2)]);
        assert_eq!(fixture.shown(), "a\nb\n|c\nd\ne");
        fixture.keys("<c-u>");
        assert_eq!(fixture.shown(), "|a\nb\nc\nd\ne");
    }

    #[test]
    fn normal_mode_refuses_text() {
        let fixture = fixture("|a");
        assert!(!fixture.vim.accepts_text());
    }

    #[test]
    fn unknown_keys_do_nothing() {
        check("|a", "Qqz", "|a");
    }
}
