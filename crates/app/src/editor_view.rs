use std::{ops::Range, path::PathBuf, sync::Arc, time::Instant};

use build::Loaded;
use editor_core::{
    EditorState, HighlightKind, Language, Mark, MarkSeverity, Motion, Selection, Symbol, SymbolKind,
    snippet::Snippet,
    vim::{Effect, Input, Key, Mode, Vim},
};
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, FontStyle, FontWeight,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, LineFragment, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render,
    ScrollEvent, ShapedLine, SharedString, Size, Style, Task, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, font, point, prelude::*, px, relative, size,
};

use crate::{
    actions::*,
    completion::{self, Entry, Extra},
    smear::{self, Smear},
    theme,
};

const GUTTER_WIDTH: f32 = 55.0;
const PLAIN_GUTTER_WIDTH: f32 = 6.0;
const LINE_NUMBER_RIGHT: f32 = 40.0;
const TEXT_RIGHT_PADDING: f32 = 8.0;
/// `scrolloff=8` from the Neovim config.
const VIM_SCROLL_OFF: f32 = 8.0;
const FALLBACK_CHAR_WIDTH: f32 = 7.2;

/// The open suggestion popup.
struct Completion {
    /// Where the text begins that accepting replaces.
    start: usize,
    /// The cursor it was made for. Any other cursor closes the popup.
    head: usize,
    items: Vec<Entry>,
    selected: usize,
}

pub enum EditorEvent {
    Edited,
    /// A plain click placed the cursor on this zero-based line.
    CursorClicked { line: usize },
}

/// Soft-wrap table: byte columns where each buffer line continues on a new row.
#[derive(Default)]
struct Rows {
    width: Pixels,
    breaks: Vec<Option<Vec<u32>>>,
    starts: Vec<u32>,
}

impl Rows {
    fn count(&self) -> usize {
        self.starts.last().copied().unwrap_or(1) as usize
    }

    fn rebuild_starts(&mut self) {
        self.starts.clear();
        self.starts.reserve(self.breaks.len() + 1);
        let mut row = 0u32;
        for breaks in &self.breaks {
            self.starts.push(row);
            row += 1 + breaks.as_ref().map_or(0, Vec::len) as u32;
        }
        self.starts.push(row);
    }

    /// Returns the buffer line and the wrapped segment that `row` shows.
    fn locate(&self, row: usize) -> (usize, usize) {
        let row = row.min(self.count().saturating_sub(1)) as u32;
        let line = self.starts.partition_point(|start| *start <= row) - 1;
        (line, (row - self.starts[line]) as usize)
    }

    fn row_of(&self, line: usize, column: usize) -> usize {
        let breaks = self.breaks[line].as_deref().unwrap_or(&[]);
        let segment = breaks.partition_point(|start| *start as usize <= column);
        self.starts[line] as usize + segment
    }

    fn segment(&self, line: usize, segment: usize, line_len: usize) -> Range<usize> {
        let breaks = self.breaks[line].as_deref().unwrap_or(&[]);
        let start = if segment == 0 {
            0
        } else {
            breaks[segment - 1] as usize
        };
        let end = breaks.get(segment).map_or(line_len, |end| *end as usize);
        start..end
    }
}

pub struct EditorView {
    pub state: EditorState,
    pub path: Option<PathBuf>,
    focus_handle: FocusHandle,
    scroll_y: Pixels,
    rows: Rows,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    autoscroll: bool,
    center_on_scroll: bool,
    bounds: Option<Bounds<Pixels>>,
    parse_task: Option<Task<()>>,
    reparse_needed: bool,
    vim: Option<Vim>,
    smooth_cursor: bool,
    /// A text field without line numbers, such as the commit message.
    plain: bool,
    smear: Option<Smear>,
    /// Where the cursor was last painted, in document coordinates.
    last_cursor: Option<(f32, f32)>,
    completion: Option<Completion>,
    auto_suggestions: bool,
    /// What the other open documents define.
    project_symbols: Arc<Vec<Symbol>>,
    /// The packages the project loads, as of the last package check.
    loaded_packages: Arc<Loaded>,
    /// What those packages define and what the project already uses.
    package_extra: Arc<Extra>,
}

impl EventEmitter<EditorEvent> for EditorView {}

impl Focusable for EditorView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EditorView {
    pub fn new(text: &str, path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let language = path
            .as_deref()
            .map_or(Language::Plain, Language::from_path);
        let mut view = Self {
            state: EditorState::new(text, language),
            path,
            focus_handle: cx.focus_handle(),
            scroll_y: px(0.),
            rows: Rows::default(),
            marked_range: None,
            is_selecting: false,
            autoscroll: false,
            center_on_scroll: false,
            bounds: None,
            parse_task: None,
            reparse_needed: false,
            vim: None,
            smooth_cursor: true,
            plain: false,
            smear: None,
            last_cursor: None,
            completion: None,
            auto_suggestions: false,
            project_symbols: Arc::default(),
            loaded_packages: Arc::default(),
            package_extra: Arc::default(),
        };
        view.schedule_parse(cx);
        view
    }

    pub fn set_plain(&mut self) {
        self.plain = true;
    }

    fn gutter_width(&self) -> Pixels {
        px(if self.plain { PLAIN_GUTTER_WIDTH } else { GUTTER_WIDTH })
    }

    /// Changes the text from outside, leaving the cursors where they are.
    pub fn edit_from_outside(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.state.edit_keeping_selections(range, text);
        self.schedule_parse(cx);
        cx.emit(EditorEvent::Edited);
        cx.notify();
    }

    /// Empties a text field.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.state.reload("");
        self.schedule_parse(cx);
        cx.notify();
    }

    pub fn title(&self) -> String {
        self.path
            .as_deref()
            .and_then(|path| path.file_name())
            .map_or_else(|| "untitled".to_string(), |name| name.to_string_lossy().into_owned())
    }

    pub fn save(&mut self, cx: &mut Context<Self>) -> std::io::Result<bool> {
        let Some(path) = &self.path else {
            return Ok(false);
        };
        if !self.state.is_dirty() {
            return Ok(false);
        }
        std::fs::write(path, self.state.buffer().to_string())?;
        self.state.mark_saved();
        cx.notify();
        Ok(true)
    }

    /// Reloads the text after an external change, unless local edits would be lost.
    pub fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        let Some(path) = &self.path else {
            return;
        };
        if self.state.is_dirty() {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        if text != self.state.buffer().to_string() {
            self.state.reload(&text);
            self.schedule_parse(cx);
            cx.notify();
        }
    }

    pub fn go_to_line(&mut self, line: usize, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.state.buffer().point_to_offset(line, column);
        self.state.set_selections(vec![Selection::cursor(offset)]);
        self.vim_after_mouse();
        self.autoscroll = true;
        self.center_on_scroll = true;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub fn set_vim_mode(&mut self, enabled: bool, smooth_cursor: bool, cx: &mut Context<Self>) {
        if enabled != self.vim.is_some() {
            self.vim = enabled.then(Vim::default);
        }
        self.smooth_cursor = smooth_cursor;
        self.smear = None;
        cx.notify();
    }

    /// Replaces the editor's own marks. Compiler marks stay.
    pub fn set_hint_marks(&mut self, hints: Vec<Mark>, cx: &mut Context<Self>) {
        // Hints come first, so that a compiler error on the same text draws on top.
        let mut marks = hints;
        marks.extend(self.state.marks().iter().filter(|mark| mark.severity != MarkSeverity::Hint).cloned());
        self.set_marks(marks, cx);
    }

    /// The message of an editor mark on the line of the cursor.
    pub fn hint_at_cursor(&self) -> Option<String> {
        let buffer = self.state.buffer();
        let line = buffer.offset_to_line(self.state.selections().primary().head);
        self.state
            .marks()
            .iter()
            .find(|mark| mark.severity == MarkSeverity::Hint && buffer.offset_to_line(mark.range.start) == line)
            .map(|mark| mark.message.clone())
    }

    pub fn set_auto_suggestions(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.auto_suggestions = enabled;
        if !enabled {
            self.completion = None;
        }
        cx.notify();
    }

    pub fn set_project_symbols(&mut self, symbols: Arc<Vec<Symbol>>) {
        self.project_symbols = symbols;
    }

    pub fn set_loaded_packages(&mut self, loaded: Arc<Loaded>, extra: Arc<Extra>) {
        self.loaded_packages = loaded;
        self.package_extra = extra;
    }

    pub fn symbols(&self) -> &[Symbol] {
        self.state.syntax().symbols()
    }

    pub fn completion_open(&self) -> bool {
        self.completion.is_some()
    }

    /// Whether the vim layer takes every key, as opposed to only `Esc` while inserting.
    pub fn vim_takes_keys(&self) -> bool {
        self.vim.as_ref().is_some_and(|vim| !vim.accepts_text())
    }

    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    pub fn vim_mode(&self) -> Option<Mode> {
        self.vim.as_ref().map(Vim::mode)
    }

    pub fn vim_pending_keys(&self) -> String {
        self.vim.as_ref().map_or_else(String::new, Vim::pending_keys)
    }

    pub fn vim_command_line(&self) -> Option<String> {
        self.vim.as_ref().and_then(Vim::command_line)
    }

    /// One-based line and character column of the cursor.
    pub fn cursor_position(&self) -> (usize, usize) {
        let buffer = self.state.buffer();
        let (line, column) = buffer.offset_to_point(self.cursor_offset());
        (line + 1, buffer.line_text(line)[..column].chars().count() + 1)
    }

    pub fn vim_record_insert(&mut self, key: Key) {
        if let Some(vim) = &mut self.vim {
            vim.record_insert(key);
        }
    }

    /// Runs one key and returns what the workspace has to do.
    pub fn vim_key(&mut self, key: Key, clipboard: Option<String>, cx: &mut Context<Self>) -> Vec<Effect> {
        let half_page = self.bounds.map_or(15, |bounds| (bounds.size.height / Self::line_height()) as usize / 2);
        let Some(vim) = &mut self.vim else {
            return Vec::new();
        };
        let version = self.state.version();
        let effects = vim.key(key, &mut self.state, &Input { clipboard, half_page });
        let mut rest = Vec::new();
        for effect in effects {
            match effect {
                Effect::Yank(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Effect::Scroll(rows) => self.scroll_y += Self::line_height() * rows as f32,
                other => rest.push(other),
            }
        }
        if self.state.version() != version {
            self.changed(cx);
        } else {
            self.selection_changed(cx);
        }
        rest
    }

    fn cursor_offset(&self) -> usize {
        match &self.vim {
            Some(vim) => vim.cursor(&self.state),
            None => self.state.selections().primary().head,
        }
    }

    fn vim_after_mouse(&mut self) {
        if let Some(vim) = &mut self.vim {
            vim.after_mouse(&mut self.state);
        }
    }

    pub fn set_marks(&mut self, marks: Vec<Mark>, cx: &mut Context<Self>) {
        self.state.set_marks(marks);
        cx.notify();
    }

    pub fn format(&mut self, kind: FormatKind, cx: &mut Context<Self>) {
        use FormatKind::*;
        let language = self.state.language();
        if matches!(kind, Math) && self.state.in_math(self.state.selections().primary().start()) {
            return;
        }
        let (prefix, suffix) = match (language, kind) {
            (Language::Latex, Bold) => ("\\textbf{", "}"),
            (Language::Latex, Italic) => ("\\textit{", "}"),
            (Language::Latex, Link) => ("\\href{}{", "}"),
            (Language::Typst, Bold) => ("*", "*"),
            (Language::Typst, Italic) => ("_", "_"),
            (Language::Typst, Link) => ("#link(\"\")[", "]"),
            (Language::Markdown, Bold) => ("**", "**"),
            (Language::Markdown, Italic) => ("*", "*"),
            (Language::Markdown, Link) => ("[", "]()"),
            (_, Math) => ("$", "$"),
            (Language::Plain, _) => return,
        };
        self.state.wrap_selections(prefix, suffix);
        self.changed(cx);
    }

    /// Inserts a toolbar snippet. With `math`, it stays inside a formula.
    pub fn insert_snippet(&mut self, template: &str, math: bool, cx: &mut Context<Self>) {
        if let Some(vim) = &mut self.vim {
            vim.enter_insert(&self.state);
        }
        self.state.insert_snippet(template, math);
        self.changed(cx);
    }

    /// Moves the cursor one character on, past a closing brace it stands before.
    pub fn step_forward(&mut self, cx: &mut Context<Self>) {
        let head = self.state.selections().primary().head;
        self.state.set_selections(vec![Selection::cursor(head + 1)]);
        self.selection_changed(cx);
    }

    pub fn undo_edit(&mut self, cx: &mut Context<Self>) {
        if self.state.undo() {
            self.changed(cx);
        }
    }

    pub fn redo_edit(&mut self, cx: &mut Context<Self>) {
        if self.state.redo() {
            self.changed(cx);
        }
    }

    /// Looks for suggestions at the cursor, after typing.
    fn update_completion(&mut self) {
        self.completion = None;
        let selections = self.state.selections();
        let head = selections.primary().head;
        if !self.auto_suggestions
            || self.plain
            || self.marked_range.is_some()
            || selections.len() != 1
            || !selections.primary().is_empty()
        {
            return;
        }
        let buffer = self.state.buffer();
        let line_start = buffer.line_start(buffer.offset_to_line(head));
        let before = buffer.slice(line_start..head);
        let language = self.state.language();
        let Some(trigger) = completion::trigger(language, &before) else {
            return;
        };
        let symbols = || self.state.syntax().symbols().iter().chain(self.project_symbols.iter());
        // A `\usepackage` typed since the last check counts at once.
        let packages = symbols().filter(|symbol| symbol.kind == SymbolKind::Package).flat_map(|symbol| symbol.name.split(','));
        let loaded = self.loaded_packages.with(packages);
        let items = completion::suggestions(language, trigger.kind, &before[trigger.start..], symbols(), &loaded, &self.package_extra);
        if !items.is_empty() {
            self.completion = Some(Completion { start: line_start + trigger.start, head, items, selected: 0 });
        }
    }

    fn accept_completion(&mut self, cx: &mut Context<Self>) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let entry = &completion.items[completion.selected];
        let prefix = completion.start..completion.head;
        if let Some(vim) = &mut self.vim {
            let typed = self.state.buffer().slice(prefix.clone());
            vim.record_completion(&typed, &Snippet::parse(&entry.template, "").text);
        }
        self.state.complete_prefix(prefix, &entry.template);
        self.changed(cx);
    }

    fn step_completion(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(completion) = &mut self.completion {
            let count = completion.items.len();
            completion.selected = (completion.selected + if forward { 1 } else { count - 1 }) % count;
            cx.notify();
        }
    }

    fn render_completion(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let completion = self.completion.as_ref()?;
        let rows = completion.items.iter().enumerate().map(|(index, entry)| {
            div()
                .id(index)
                .flex()
                .justify_between()
                .gap(px(24.))
                .px(px(8.))
                .py(px(2.))
                .cursor_pointer()
                .when(index == completion.selected, |row| row.bg(theme::color(theme::palette().accent_soft)))
                .hover(|style| style.bg(theme::color(theme::palette().accent_soft)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(completion) = &mut this.completion {
                        completion.selected = index;
                    }
                    this.accept_completion(cx);
                }))
                .child(entry.label.clone())
                .child(div().text_color(theme::color(theme::palette().ui_muted)).child(entry.detail.clone()))
        });
        Some(
            div()
                .id("completion")
                .occlude()
                .min_w(px(180.))
                .py(px(4.))
                .rounded(px(4.))
                .font(Self::font(FontStyle::Normal, FontWeight::NORMAL))
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::color(theme::palette().code_text))
                .bg(theme::color(theme::palette().panel))
                .border_1()
                .border_color(theme::color(theme::palette().divider))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(rows)
                .into_any_element(),
        )
    }

    /// Below the start of the replaced text. Needs the rows of this frame.
    fn completion_origin(&self, bounds: Bounds<Pixels>, window: &mut Window) -> Option<Point<Pixels>> {
        let completion = self.completion.as_ref()?;
        let (row, x) = self.row_and_x(completion.start, window);
        let y = Self::line_height() * (row + 1) as f32 - self.scroll_y;
        Some(point(bounds.left() + self.gutter_width() + x, bounds.top() + y))
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.completion = None;
        self.autoscroll = true;
        self.schedule_parse(cx);
        cx.emit(EditorEvent::Edited);
        cx.notify();
    }

    fn selection_changed(&mut self, cx: &mut Context<Self>) {
        self.autoscroll = true;
        cx.notify();
    }

    fn schedule_parse(&mut self, cx: &mut Context<Self>) {
        if self.parse_task.is_some() {
            self.reparse_needed = true;
            return;
        }
        let Some(job) = self.state.start_parse() else {
            return;
        };
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { job.run() }).await;
            this.update(cx, |this, cx| {
                this.state.finish_parse(result);
                this.parse_task = None;
                if std::mem::take(&mut this.reparse_needed) {
                    this.schedule_parse(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // Layout helpers

    fn font(style: FontStyle, weight: FontWeight) -> Font {
        let mut code_font = font(theme::CODE_FONT);
        code_font.style = style;
        code_font.weight = weight;
        code_font
    }

    fn plain_font() -> Font {
        Self::font(FontStyle::Normal, FontWeight::NORMAL)
    }

    fn line_height() -> Pixels {
        px(theme::CODE_LINE_HEIGHT)
    }

    fn sync_rows(&mut self, width: Pixels, window: &mut Window) {
        let changes = self.state.take_line_changes();
        let width_changed = (self.rows.width - width).abs() > px(0.5);
        let line_count = self.state.buffer().line_count();
        if width_changed || self.rows.breaks.is_empty() {
            self.rows.width = width;
            self.rows.breaks = vec![None; line_count];
        } else if !changes.is_empty() {
            for change in changes {
                let start = change.start_line.min(self.rows.breaks.len());
                let end = (start + change.removed_lines).min(self.rows.breaks.len());
                self.rows
                    .breaks
                    .splice(start..end, std::iter::repeat_n(None, change.inserted_lines));
            }
            if self.rows.breaks.len() != line_count {
                self.rows.breaks = vec![None; line_count];
            }
        } else if self.rows.starts.len() == line_count + 1 {
            return;
        }

        let font_size = px(theme::CODE_FONT_SIZE);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&Self::plain_font());
        let char_width = text_system
            .ch_advance(font_id, font_size)
            .unwrap_or(px(7.2));
        let mut wrapper = text_system.line_wrapper(Self::plain_font(), font_size);
        let buffer = self.state.buffer();
        for (line, breaks) in self.rows.breaks.iter_mut().enumerate() {
            if breaks.is_some() {
                continue;
            }
            let text = buffer.line_text(line);
            // Monospaced ASCII that fits needs no shaping.
            if text.is_ascii() && char_width * text.len() as f32 <= width {
                *breaks = Some(Vec::new());
                continue;
            }
            let display = text.replace('\t', " ");
            let fragments = [LineFragment::text(&display)];
            let boundaries = wrapper
                .wrap_line(&fragments, width, gpui::IndentAdjustment::NoIndent)
                .map(|boundary| boundary.ix as u32)
                .collect();
            *breaks = Some(boundaries);
        }
        drop(wrapper);
        self.rows.rebuild_starts();
    }

    fn shape_plain(&self, text: &str, window: &mut Window) -> ShapedLine {
        let display: SharedString = text.replace('\t', " ").into();
        let run = TextRun {
            len: display.len(),
            font: Self::plain_font(),
            color: theme::color(theme::palette().code_text),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(display, px(theme::CODE_FONT_SIZE), &[run], None)
    }

    fn row_segment(&self, row: usize) -> (usize, Range<usize>) {
        let (line, segment) = self.rows.locate(row);
        let buffer = self.state.buffer();
        let line_len = buffer.line_end(line) - buffer.line_start(line);
        (line, self.rows.segment(line, segment, line_len))
    }

    fn row_and_x(&self, offset: usize, window: &mut Window) -> (usize, Pixels) {
        let buffer = self.state.buffer();
        let (line, column) = buffer.offset_to_point(offset);
        let row = self.rows.row_of(line, column);
        let (_, range) = self.row_segment(row);
        let text = buffer.line_text(line);
        let shaped = self.shape_plain(&text[range.clone()], window);
        (row, shaped.x_for_index(column.saturating_sub(range.start)))
    }

    fn offset_at(&self, row: usize, x: Pixels, window: &mut Window) -> usize {
        let (line, range) = self.row_segment(row);
        let buffer = self.state.buffer();
        let text = buffer.line_text(line);
        let shaped = self.shape_plain(&text[range.clone()], window);
        let mut index = shaped.closest_index_for_x(x);
        // The last character of a wrapped segment belongs to the next row.
        if index == range.len() && range.end < text.len() && index > 0 {
            index = text[range.clone()]
                .char_indices()
                .next_back()
                .map_or(0, |(last, _)| last);
        }
        buffer.line_start(line) + range.start + index
    }

    fn position_to_offset(&self, position: Point<Pixels>, window: &mut Window) -> usize {
        let Some(bounds) = self.bounds else {
            return 0;
        };
        if self.rows.starts.is_empty() {
            return 0;
        }
        let y = position.y - bounds.top() + self.scroll_y;
        if y < px(0.) {
            return 0;
        }
        let row = (y / Self::line_height()) as usize;
        if row >= self.rows.count() {
            return self.state.buffer().len();
        }
        let x = (position.x - bounds.left() - self.gutter_width()).max(px(0.));
        self.offset_at(row, x, window)
    }

    fn clamp_scroll(&mut self, viewport: Pixels) {
        let content = Self::line_height() * self.rows.count() as f32;
        let max = (content - viewport * 0.25).max(px(0.));
        self.scroll_y = self.scroll_y.clamp(px(0.), max);
    }

    fn visible_page_rows(&self) -> usize {
        self.bounds
            .map_or(30, |bounds| (bounds.size.height / Self::line_height()) as usize)
            .saturating_sub(2)
            .max(1)
    }

    // Motion

    fn apply_motion(&mut self, motion: Motion, extend: bool, cx: &mut Context<Self>) {
        self.state.move_selections(motion, extend);
        self.selection_changed(cx);
    }

    fn move_vertical(&mut self, delta: isize, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        let max_row = self.rows.count().saturating_sub(1) as isize;
        let len = self.state.buffer().len();
        let selections = self
            .state
            .selections()
            .to_vec_primary_last()
            .into_iter()
            .map(|selection| {
                let (row, x) = self.row_and_x(selection.head, window);
                let goal = selection.goal_x.map_or(x, px);
                let target = row as isize + delta;
                let head = if target < 0 {
                    0
                } else if target > max_row {
                    len
                } else {
                    self.offset_at(target as usize, goal, window)
                };
                Selection {
                    anchor: if extend { selection.anchor } else { head },
                    head,
                    goal_x: Some(goal.into()),
                }
            })
            .collect();
        self.state.set_selections(selections);
        self.selection_changed(cx);
    }

    fn add_cursor_vertical(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let mut selections = self.state.selections().to_vec_primary_last();
        let primary = *selections.last().expect("selection set is never empty");
        let (row, x) = self.row_and_x(primary.head, window);
        let target = row as isize + delta;
        if target < 0 || target as usize >= self.rows.count() {
            return;
        }
        selections.push(Selection::cursor(self.offset_at(target as usize, x, window)));
        self.state.set_selections(selections);
        self.selection_changed(cx);
    }

    // Mouse

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let offset = self.position_to_offset(event.position, window);
        if event.modifiers.secondary() {
            self.state.toggle_cursor(offset);
        } else if event.modifiers.shift {
            let anchor = self.state.selections().primary().anchor;
            self.state.set_selections(vec![Selection::new(anchor, offset)]);
        } else {
            match event.click_count {
                2 => self.state.select_word_at(offset),
                3 => self.state.select_line_at(offset),
                _ => {
                    self.state.set_selections(vec![Selection::cursor(offset)]);
                    let line = self.state.buffer().offset_to_line(offset);
                    cx.emit(EditorEvent::CursorClicked { line });
                }
            }
        }
        self.vim_after_mouse();
        self.is_selecting = true;
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.is_selecting) {
            self.vim_after_mouse();
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let offset = self.position_to_offset(event.position, window);
        let mut selections = self.state.selections().to_vec_primary_last();
        let primary = selections.last_mut().expect("selection set is never empty");
        if primary.head != offset {
            primary.head = offset;
            self.state.set_selections(selections);
            self.selection_changed(cx);
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(Self::line_height());
        self.scroll_y -= delta.y;
        if let Some(bounds) = self.bounds {
            self.clamp_scroll(bounds.size.height);
        }
        self.autoscroll = false;
        cx.notify();
    }

    // Styled shaping

    fn line_kinds(&self, line: usize, spans: &[editor_core::HighlightSpan]) -> Vec<Option<HighlightKind>> {
        let buffer = self.state.buffer();
        let start = buffer.line_start(line);
        let end = buffer.line_end(line);
        let mut kinds = vec![None; end - start];
        for span in spans {
            let from = span.range.start.max(start);
            let to = span.range.end.min(end);
            if from < to {
                kinds[from - start..to - start].fill(Some(span.kind));
            }
        }
        kinds
    }

    fn shape_styled(
        &self,
        text: &str,
        kinds: &[Option<HighlightKind>],
        window: &mut Window,
    ) -> ShapedLine {
        let display: SharedString = text.replace('\t', " ").into();
        let mut runs: Vec<TextRun> = Vec::new();
        let mut last: Option<Option<HighlightKind>> = None;
        for (index, ch) in text.char_indices() {
            let kind = kinds.get(index).copied().flatten();
            if last == Some(kind) {
                runs.last_mut().expect("run exists after first char").len += ch.len_utf8();
                continue;
            }
            last = Some(kind);
            let (color, italic) = theme::highlight_style(kind);
            let style = if italic { FontStyle::Italic } else { FontStyle::Normal };
            let weight = if kind == Some(HighlightKind::Strong) {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            };
            runs.push(TextRun {
                len: ch.len_utf8(),
                font: Self::font(style, weight),
                color: theme::color(color),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
        window
            .text_system()
            .shape_line(display, px(theme::CODE_FONT_SIZE), &runs, None)
    }
}

#[derive(Clone, Copy)]
pub enum FormatKind {
    Bold,
    Italic,
    Math,
    Link,
}

macro_rules! motion_actions {
    ($this:ident, $div:ident, $cx:ident; $($action:ident => $motion:ident, $extend:literal;)*) => {
        $(
            let $div = $div.on_action($cx.listener(|this: &mut Self, _: &$action, _, cx| {
                this.apply_motion(Motion::$motion, $extend, cx);
            }));
        )*
    };
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let head = self.state.selections().primary().head;
        if self.completion.as_ref().is_some_and(|completion| completion.head != head)
            || !self.focus_handle.is_focused(window)
        {
            self.completion = None;
        }
        let root = div()
            .id("editor")
            .key_context(if self.completion.is_some() { "Editor Completion" } else { "Editor" })
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme::color(theme::palette().bg))
            .cursor(CursorStyle::IBeam);
        motion_actions!(self, root, cx;
            MoveLeft => Left, false;
            MoveRight => Right, false;
            MoveWordLeft => WordLeft, false;
            MoveWordRight => WordRight, false;
            MoveLineStart => LineStart, false;
            MoveLineEnd => LineEnd, false;
            MoveDocumentStart => DocumentStart, false;
            MoveDocumentEnd => DocumentEnd, false;
            SelectLeft => Left, true;
            SelectRight => Right, true;
            SelectWordLeft => WordLeft, true;
            SelectWordRight => WordRight, true;
            SelectLineStart => LineStart, true;
            SelectLineEnd => LineEnd, true;
            SelectDocumentStart => DocumentStart, true;
            SelectDocumentEnd => DocumentEnd, true;
        );
        root.on_action(cx.listener(|this, _: &MoveUp, window, cx| this.move_vertical(-1, false, window, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, window, cx| this.move_vertical(1, false, window, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, window, cx| this.move_vertical(-1, true, window, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, window, cx| this.move_vertical(1, true, window, cx)))
            .on_action(cx.listener(|this, _: &PageUp, window, cx| {
                let rows = this.visible_page_rows() as isize;
                this.move_vertical(-rows, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PageDown, window, cx| {
                let rows = this.visible_page_rows() as isize;
                this.move_vertical(rows, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &AddCursorAbove, window, cx| this.add_cursor_vertical(-1, window, cx)))
            .on_action(cx.listener(|this, _: &AddCursorBelow, window, cx| this.add_cursor_vertical(1, window, cx)))
            .on_action(cx.listener(|this, _: &CollapseSelections, _, cx| {
                this.state.end_snippet();
                let head = this.state.selections().primary().head;
                this.state.set_selections(vec![Selection::cursor(head)]);
                this.selection_changed(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.state.select_all();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SelectNextOccurrence, _, cx| {
                this.state.select_next_occurrence();
                this.selection_changed(cx);
            }))
            .on_action(cx.listener(|this, _: &Backspace, _, cx| {
                let open = this.completion.is_some();
                this.state.backspace();
                this.changed(cx);
                if open {
                    this.update_completion();
                }
            }))
            .on_action(cx.listener(|this, _: &AcceptCompletion, _, cx| this.accept_completion(cx)))
            .on_action(cx.listener(|this, _: &NextCompletion, _, cx| this.step_completion(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousCompletion, _, cx| this.step_completion(false, cx)))
            .on_action(cx.listener(|this, _: &CloseCompletion, _, cx| {
                this.completion = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DeleteForward, _, cx| {
                this.state.delete_forward();
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteWordLeft, _, cx| {
                this.state.delete_motion(Motion::WordLeft);
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteWordRight, _, cx| {
                this.state.delete_motion(Motion::WordRight);
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteToLineStart, _, cx| {
                this.state.delete_motion(Motion::LineStart);
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &Newline, _, cx| {
                this.state.newline();
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &Indent, _, cx| {
                if this.state.next_snippet_slot() {
                    return this.selection_changed(cx);
                }
                this.state.indent();
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &Outdent, _, cx| {
                this.state.outdent();
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo_edit(cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo_edit(cx)))
            .on_action(cx.listener(|this, _: &Bold, _, cx| this.format(FormatKind::Bold, cx)))
            .on_action(cx.listener(|this, _: &Italic, _, cx| this.format(FormatKind::Italic, cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if let Some(text) = this.state.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }))
            .on_action(cx.listener(|this, _: &Cut, _, cx| {
                if let Some(text) = this.state.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    this.state.insert("");
                    this.changed(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    this.state.paste(&text);
                    this.changed(cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(EditorElement { view: cx.entity(), popup: self.render_completion(cx) })
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let buffer = self.state.buffer();
        let range = buffer.utf16_to_offset(range_utf16.start)..buffer.utf16_to_offset(range_utf16.end);
        actual_range.replace(buffer.offset_to_utf16(range.start)..buffer.offset_to_utf16(range.end));
        Some(buffer.slice(range))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.state.selections().primary();
        let buffer = self.state.buffer();
        Some(UTF16Selection {
            range: buffer.offset_to_utf16(selection.start())..buffer.offset_to_utf16(selection.end()),
            reversed: selection.is_reversed(),
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let buffer = self.state.buffer();
        let start = buffer.utf16_to_offset(range_utf16.start);
        let end = buffer.utf16_to_offset(range_utf16.end);
        self.state.set_selections(vec![Selection::new(start, end)]);
        cx.notify();
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let buffer = self.state.buffer();
        Some(buffer.offset_to_utf16(buffer.len()))
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let buffer = self.state.buffer();
        self.marked_range
            .as_ref()
            .map(|range| buffer.offset_to_utf16(range.start)..buffer.offset_to_utf16(range.end))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.vim.as_ref().is_some_and(|vim| !vim.accepts_text()) {
            return;
        }
        let marked = self.marked_range.take();
        let buffer = self.state.buffer();
        let range = range_utf16
            .map(|range| buffer.utf16_to_offset(range.start)..buffer.utf16_to_offset(range.end))
            .or(marked);
        match range {
            Some(range) => {
                let cursor = range.start + new_text.len();
                self.state.replace_range(range, new_text, Selection::cursor(cursor));
            }
            None => self.state.type_text(new_text),
        }
        self.changed(cx);
        self.update_completion();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.vim.as_ref().is_some_and(|vim| !vim.accepts_text()) {
            return;
        }
        let buffer = self.state.buffer();
        let range = range_utf16
            .map(|range| buffer.utf16_to_offset(range.start)..buffer.utf16_to_offset(range.end))
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.state.selections().primary().range());
        let start_utf16 = buffer.offset_to_utf16(range.start);
        let start = range.start;
        self.state
            .replace_range(range, new_text, Selection::cursor(start + new_text.len()));
        self.marked_range = (!new_text.is_empty()).then(|| start..start + new_text.len());
        if let Some(selected) = new_selected_range_utf16 {
            let buffer = self.state.buffer();
            let anchor = buffer.utf16_to_offset(start_utf16 + selected.start);
            let head = buffer.utf16_to_offset(start_utf16 + selected.end);
            self.state.set_selections(vec![Selection::new(anchor, head)]);
        }
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if self.rows.starts.is_empty() {
            return None;
        }
        let offset = self.state.buffer().utf16_to_offset(range_utf16.start);
        let (row, x) = self.row_and_x(offset, window);
        let y = Self::line_height() * row as f32 - self.scroll_y;
        Some(Bounds::new(
            point(element_bounds.left() + self.gutter_width() + x, element_bounds.top() + y),
            size(px(2.), Self::line_height()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.position_to_offset(position, window);
        Some(self.state.buffer().offset_to_utf16(offset))
    }
}

struct EditorElement {
    view: Entity<EditorView>,
    popup: Option<AnyElement>,
}

struct PaintedRow {
    y: Pixels,
    number: Option<ShapedLine>,
    active: bool,
    text: ShapedLine,
}

struct EditorPrepaint {
    rows: Vec<PaintedRow>,
    search_hits: Vec<PaintQuad>,
    selections: Vec<PaintQuad>,
    cursors: Vec<PaintQuad>,
    block: Option<PaintQuad>,
    trail: Option<Vec<Point<Pixels>>>,
    squiggles: Vec<(Point<Pixels>, Pixels, u32)>,
    marked: Vec<(Point<Pixels>, Pixels)>,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = EditorPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> EditorPrepaint {
        let prepaint = self.view.update(cx, |view, _cx| view.prepaint_rows(bounds, window));
        if let Some(mut popup) = self.popup.take()
            && let Some(mut origin) = self.view.read(cx).completion_origin(bounds, window)
        {
            let popup_size = popup.layout_as_root(Size::<AvailableSpace>::default(), window, cx);
            let viewport = window.viewport_size();
            // Above the line when there is no room below, and always inside the window.
            if origin.y + popup_size.height > viewport.height {
                origin.y -= popup_size.height + EditorView::line_height();
            }
            origin.x = origin.x.min(viewport.width - popup_size.width).max(px(0.));
            window.defer_draw(popup, origin, 1, None);
        }
        prepaint
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut EditorPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.view.read(cx).focus_handle.clone();
        let gutter = self.view.read(cx).gutter_width();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            let line_height = EditorView::line_height();
            for row in &prepaint.rows {
                if row.active {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.left(), row.y),
                            size(bounds.size.width, line_height),
                        ),
                        theme::color(theme::palette().active_line),
                    ));
                }
            }
            for quad in prepaint.search_hits.drain(..) {
                window.paint_quad(quad);
            }
            for quad in prepaint.selections.drain(..) {
                window.paint_quad(quad);
            }
            // Below the text, so that the character under the cursor stays readable.
            if focus_handle.is_focused(window) {
                if let Some(points) = prepaint.trail.take() {
                    let mut path = gpui::Path::new(points[0]);
                    for point in &points[1..] {
                        path.line_to(*point);
                    }
                    window.paint_path(path, theme::color(theme::palette().caret).opacity(0.45));
                    // Frames are only requested while a trail is on screen.
                    window.request_animation_frame();
                }
                if let Some(block) = prepaint.block.take() {
                    window.paint_quad(block);
                }
            }
            for row in &prepaint.rows {
                if let Some(number) = &row.number {
                    let x = bounds.left() + px(LINE_NUMBER_RIGHT) - number.width;
                    number
                        .paint(point(x, row.y), line_height, TextAlign::Left, None, window, cx)
                        .ok();
                }
                row.text
                    .paint(
                        point(bounds.left() + gutter, row.y),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            for (origin, width, color) in &prepaint.squiggles {
                window.paint_underline(
                    *origin,
                    *width,
                    &UnderlineStyle {
                        thickness: px(1.),
                        color: Some(theme::color(*color)),
                        wavy: true,
                    },
                );
            }
            for (origin, width) in &prepaint.marked {
                window.paint_underline(
                    *origin,
                    *width,
                    &UnderlineStyle {
                        thickness: px(1.),
                        color: Some(theme::color(theme::palette().code_text)),
                        wavy: false,
                    },
                );
            }
            if focus_handle.is_focused(window) {
                for quad in prepaint.cursors.drain(..) {
                    window.paint_quad(quad);
                }
            }
        });
    }
}

impl EditorView {
    fn prepaint_rows(&mut self, bounds: Bounds<Pixels>, window: &mut Window) -> EditorPrepaint {
        self.bounds = Some(bounds);
        let line_height = Self::line_height();
        let text_width = (bounds.size.width - self.gutter_width() - px(TEXT_RIGHT_PADDING)).max(px(40.));
        self.sync_rows(text_width, window);

        if std::mem::take(&mut self.autoscroll) {
            let (line, column) = self.state.buffer().offset_to_point(self.cursor_offset());
            let row = self.rows.row_of(line, column);
            let top = line_height * row as f32;
            let bottom = top + line_height;
            let viewport = bounds.size.height;
            let center = std::mem::take(&mut self.center_on_scroll);
            let margin = if self.vim.is_some() {
                (line_height * VIM_SCROLL_OFF).min((viewport - line_height).max(px(0.)) / 2.)
            } else {
                px(0.)
            };
            if center && (top < self.scroll_y || bottom > self.scroll_y + viewport) {
                self.scroll_y = top - viewport / 2.;
            } else if top < self.scroll_y + margin {
                self.scroll_y = top - margin;
            } else if bottom > self.scroll_y + viewport - margin {
                self.scroll_y = bottom - viewport + margin;
            }
        }
        self.clamp_scroll(bounds.size.height);

        let first_row = (self.scroll_y / line_height) as usize;
        let visible = (bounds.size.height / line_height) as usize + 2;
        let last_row = (first_row + visible).min(self.rows.count());

        let buffer = self.state.buffer().clone();
        let (first_line, _) = self.rows.locate(first_row);
        let (last_line, _) = self.rows.locate(last_row.saturating_sub(1));
        let byte_range = buffer.line_start(first_line)..buffer.line_end(last_line);

        // Wider spans paint first, so that nested nodes keep their own color.
        let mut spans = self.state.syntax().highlights(buffer.rope(), byte_range);
        spans.sort_by_key(|span| std::cmp::Reverse(span.range.len()));

        let selections: Vec<Selection> = self.state.selections().as_slice().to_vec();
        let active_lines: Vec<usize> = selections
            .iter()
            .filter(|selection| selection.is_empty())
            .map(|selection| buffer.offset_to_line(selection.head))
            .collect();

        let vim_cursor = self.vim.as_ref().map(|vim| (vim.cursor(&self.state), vim.mode()));
        let cursor_line = vim_cursor.map(|(offset, _)| buffer.offset_to_line(offset));
        let shows_block = vim_cursor.is_some_and(|(_, mode)| mode != Mode::Insert);
        let mut cursor_cell: Option<(Pixels, f32, Pixels)> = None;
        let mut search_hits = Vec::new();
        let mut hits_in_line: Vec<Range<usize>> = Vec::new();

        let code_font = Self::plain_font();
        let number_color = theme::color(theme::palette().line_number);
        let active_number_color = theme::color(theme::palette().line_number_active);
        let error_lines: std::collections::HashSet<usize> = self
            .state
            .marks()
            .iter()
            .filter(|mark| mark.severity == MarkSeverity::Error)
            .map(|mark| buffer.offset_to_line(mark.range.start))
            .collect();
        let mut rows = Vec::with_capacity(last_row.saturating_sub(first_row));
        let mut selection_quads = Vec::new();
        let mut cursors = Vec::new();
        let mut squiggles = Vec::new();
        let mut marked = Vec::new();
        let mut cached_line: Option<(usize, String, Vec<Option<HighlightKind>>)> = None;
        let text_left = bounds.left() + self.gutter_width();

        for row in first_row..last_row {
            let (line, segment) = self.rows.locate(row);
            if cached_line.as_ref().is_none_or(|(cached, ..)| *cached != line) {
                let text = buffer.line_text(line);
                hits_in_line = self.vim.as_ref().map_or_else(Vec::new, |vim| vim.highlights_in_line(&text));
                cached_line = Some((line, text, self.line_kinds(line, &spans)));
            }
            let (_, text, kinds) = cached_line.as_ref().expect("line cache was just filled");
            let range = self.rows.segment(line, segment, text.len());
            let shaped = self.shape_styled(&text[range.clone()], &kinds[range.clone()], window);
            let y = bounds.top() + line_height * row as f32 - self.scroll_y;

            let number = (segment == 0 && !self.plain).then(|| {
                // `relativenumber` with `number`, as in the Neovim config.
                let shown = match cursor_line {
                    Some(cursor_line) if cursor_line != line => cursor_line.abs_diff(line),
                    _ => line + 1,
                };
                let label: SharedString = shown.to_string().into();
                let run = TextRun {
                    len: label.len(),
                    font: code_font.clone(),
                    color: if error_lines.contains(&line) {
                        theme::color(theme::palette().error)
                    } else if cursor_line == Some(line) {
                        active_number_color
                    } else {
                        number_color
                    },
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                window
                    .text_system()
                    .shape_line(label, px(theme::CODE_FONT_SIZE), &[run], None)
            });

            let line_start = buffer.line_start(line);
            let row_start = line_start + range.start;
            let row_end = line_start + range.end;
            let is_last_segment = range.end == text.len();

            for hit in &hits_in_line {
                let from = (line_start + hit.start).max(row_start);
                let to = (line_start + hit.end).min(row_end);
                if from < to {
                    search_hits.push(fill(
                        Bounds::from_corners(
                            point(text_left + shaped.x_for_index(from - row_start), y),
                            point(text_left + shaped.x_for_index(to - row_start), y + line_height),
                        ),
                        theme::color(theme::palette().search_match),
                    ));
                }
            }
            if let Some((offset, _)) = vim_cursor
                && offset >= row_start
                && (offset < row_end || (offset == row_end && is_last_segment))
            {
                let left = shaped.x_for_index(offset - row_start);
                let next = buffer.next_boundary(offset).min(row_end);
                let right = if next > offset {
                    shaped.x_for_index(next - row_start)
                } else {
                    left + px(FALLBACK_CHAR_WIDTH)
                };
                cursor_cell = Some((left, row as f32 * theme::CODE_LINE_HEIGHT, (right - left).max(px(1.))));
            }

            for selection in &selections {
                if !selection.is_empty() && selection.start() <= row_end && selection.end() >= row_start {
                    let from = selection.start().max(row_start) - row_start;
                    let to = selection.end().min(row_end) - row_start;
                    let mut right = shaped.x_for_index(to);
                    if selection.end() > row_end {
                        right += px(6.);
                    }
                    selection_quads.push(fill(
                        Bounds::from_corners(
                            point(text_left + shaped.x_for_index(from), y),
                            point(text_left + right, y + line_height),
                        ),
                        theme::color(theme::palette().selection),
                    ));
                }
                let head = selection.head;
                let head_here = head >= row_start
                    && (head < row_end || (head == row_end && is_last_segment));
                if head_here && !shows_block {
                    cursors.push(fill(
                        Bounds::new(
                            point(text_left + shaped.x_for_index(head - row_start), y),
                            size(px(1.5), line_height),
                        ),
                        theme::color(theme::palette().caret),
                    ));
                }
            }

            let baseline = y + line_height - px(3.);
            for mark in self.state.marks() {
                let from = mark.range.start.max(row_start);
                let to = mark.range.end.min(row_end);
                if from < to {
                    let left = shaped.x_for_index(from - row_start);
                    let right = shaped.x_for_index(to - row_start);
                    let color = match mark.severity {
                        MarkSeverity::Error => theme::palette().error,
                        MarkSeverity::Warning => theme::palette().warning,
                        MarkSeverity::Hint => theme::palette().ui_muted,
                    };
                    squiggles.push((point(text_left + left, baseline), right - left, color));
                }
            }
            if let Some(range) = &self.marked_range {
                let from = range.start.max(row_start);
                let to = range.end.min(row_end);
                if from < to {
                    let left = shaped.x_for_index(from - row_start);
                    let right = shaped.x_for_index(to - row_start);
                    marked.push((point(text_left + left, baseline), right - left));
                }
            }

            rows.push(PaintedRow {
                y,
                number,
                active: !self.plain && active_lines.contains(&line),
                text: shaped,
            });
        }

        let mut block = None;
        let mut trail = None;
        if let Some((x, document_y, width)) = cursor_cell {
            let target = (f32::from(x), document_y);
            let height = theme::CODE_LINE_HEIGHT;
            let width_value = f32::from(width);
            let to_screen = |(x, y): (f32, f32)| point(text_left + px(x), bounds.top() + px(y) - self.scroll_y);
            if self.last_cursor != Some(target) {
                let far = self.last_cursor.is_some_and(|from| from.1 != target.1 || (from.0 - target.0).abs() > width_value * 1.5);
                self.smear = match self.last_cursor {
                    Some(from) if far && shows_block && self.smooth_cursor => {
                        Some(Smear { from, start: Instant::now() })
                    }
                    _ => None,
                };
                self.last_cursor = Some(target);
            }
            if let Some(active) = self.smear {
                let progress = active.start.elapsed().as_secs_f32() / smear::DURATION.as_secs_f32();
                if progress >= 1. {
                    self.smear = None;
                } else {
                    let polygon = smear::trail(active.from, target, (width_value, height), progress);
                    trail = Some(polygon.into_iter().map(to_screen).collect());
                }
            }
            if shows_block {
                block = Some(fill(
                    Bounds::new(to_screen(target), size(width, line_height)),
                    theme::color(theme::palette().caret),
                ));
            }
        }

        EditorPrepaint {
            rows,
            search_hits,
            selections: selection_quads,
            cursors,
            block,
            trail,
            squiggles,
            marked,
        }
    }
}
