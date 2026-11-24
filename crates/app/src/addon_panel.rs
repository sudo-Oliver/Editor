//! Draws the panel of an add-on and passes messages between it and the workspace.
//! The program starts when the panel first shows, or with the project if the
//! manifest asks for that.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use addon_api::{
    API_VERSION, Activation, AddonMessage, CitationEntry, DropdownOption, EventKind, StatusItem, FEATURES, HostMessage, Node, Permission, Reply, Request, Role,
    Tone,
};
use gpui::{
    Anchor, AnyElement, App, ClipboardEntry, ClipboardItem, Context, ExternalPaths, ImageFormat, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, MouseButton, ObjectFit, Pixels, Render, ScrollHandle, SharedString, Size, Task, Window, anchored,
    deferred, div, img, point, prelude::*, px, size,
};

use crate::{
    actions::{CloseMenu, Newline, Paste, Recompile},
    addons::{self, AddonProcess, Installed, ProcessEvent},
    editor_view::{DragContent, DragItem, DragPreview, EditorView},
    markdown::{self, Block},
    markdown_view::{self, Look},
    theme,
};

const INPUT_HEIGHT: f32 = 62.0;
const CHECK_SIZE: f32 = 12.0;
const CHECK_INDENT: f32 = 14.0;
const SUGGESTION_LIMIT: usize = 300;
const DROPDOWN_WIDTH: f32 = 200.0;
const MAX_APPENDED_BYTES: usize = 8 * 1024 * 1024;
/// Messages handled before the panel draws again, so that a stream of them
/// costs one frame, not one each.
const BATCH: usize = 256;
const MULTILINE_ROWS: usize = 8;

/// The list of a `Node::Dropdown` while it shows.
struct OpenDropdown {
    id: String,
    /// The row that Enter would choose.
    highlighted: usize,
    focus: FocusHandle,
    /// Gets the focus back, so that typing goes on where it was.
    previous: Option<FocusHandle>,
}

pub enum AddonPanelEvent {
    Request { id: u64, request: Request },
    /// A note for the status line.
    Notify(String),
    /// The user asked to put this text at the cursor of the document.
    InsertText(String),
    /// The program ended or was stopped, so what waits on it ends too.
    Stopped,
    /// The dialog opened, closed or changed, and the workspace draws it.
    DialogChanged,
    /// The program ended before it answered the shortcut.
    LookupFailed(String),
}

pub struct AddonPanel {
    pub add_on: Installed,
    root: Option<PathBuf>,
    visible: bool,
    process: Option<AddonProcess>,
    task: Option<Task<()>>,
    /// Why the program does not run. `None` before it first starts.
    stopped: Option<String>,
    /// The last broken message, which the add-on should not have sent.
    invalid: Option<String>,
    view: Node,
    dialog: Option<Node>,
    /// The shortcut went out and no dialog came back yet.
    dialog_asked: bool,
    inputs: HashMap<String, Entity<EditorView>>,
    scrolls: HashMap<String, ScrollHandle>,
    citations: Vec<CitationEntry>,
    open_dropdown: Option<OpenDropdown>,
    viewport: Size<Pixels>,
    /// Where each node with an id lies in the view, as child indexes.
    index: HashMap<String, Vec<usize>>,
    /// The parsed form of each `Markdown` node with an id.
    documents: HashMap<String, markdown::Document>,
    /// Collapsibles the user opened or closed.
    toggled: HashMap<String, bool>,
    /// A message added content, so following scrolls go to the end after the batch.
    scroll_pending: bool,
    /// The events the add-on asked for.
    subscriptions: HashSet<EventKind>,
    /// The line at the top of the panel.
    status: Vec<StatusItem>,
}

impl EventEmitter<AddonPanelEvent> for AddonPanel {}

impl AddonPanel {
    pub fn new(add_on: Installed, root: Option<PathBuf>) -> Self {
        Self {
            add_on,
            root,
            visible: false,
            process: None,
            task: None,
            stopped: None,
            invalid: None,
            view: Node::default(),
            dialog: None,
            dialog_asked: false,
            inputs: HashMap::new(),
            scrolls: HashMap::new(),
            citations: Vec::new(),
            open_dropdown: None,
            viewport: size(px(0.), px(0.)),
            index: HashMap::new(),
            documents: HashMap::new(),
            toggled: HashMap::new(),
            scroll_pending: false,
            subscriptions: HashSet::new(),
            status: Vec::new(),
        }
    }

    fn starts_with_project(&self) -> bool {
        self.add_on.manifest.activation == Activation::Project && self.root.is_some()
    }

    /// Starts the program now if it should not wait for the panel to show.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if self.starts_with_project() && self.process.is_none() && self.stopped.is_none() {
            self.start(cx);
        }
    }

    pub fn citations(&self) -> &[CitationEntry] {
        &self.citations
    }

    /// Tells the add-on about a saved file, if it runs with the project or asked.
    pub fn notify_saved(&self, path: &str) {
        if self.add_on.manifest.activation == Activation::Project || self.wants(EventKind::Saved) {
            self.send(HostMessage::Saved { path: path.to_string() });
        }
    }

    /// Runs a command of the manifest: starts the program if needed and tells it.
    pub fn run_command(&mut self, id: &str, context: Option<addon_api::ActiveFile>, cx: &mut Context<Self>) {
        if self.process.is_none() {
            self.start(cx);
        }
        self.send(HostMessage::Command { id: id.to_string(), context });
    }

    /// Whether the add-on runs and asked for these events.
    pub fn wants(&self, kind: EventKind) -> bool {
        self.process.is_some() && self.subscriptions.contains(&kind)
    }

    pub fn send_event(&self, message: HostMessage) {
        self.send(message);
    }

    /// Whether the add-on speaks version 2, which has events and drops.
    fn speaks_two(&self) -> bool {
        self.add_on.manifest.api >= 2
    }

    /// A picture on the clipboard goes to the add-on as a file instead of
    /// text. Anything else pastes as usual.
    fn paste_image(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(image) = cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(image.clone()),
                _ => None,
            })
        }) else {
            return false;
        };
        let extension = match image.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Gif => "gif",
            ImageFormat::Svg => "svg",
            _ => "png",
        };
        match addons::private_file("pasted", extension).and_then(|path| std::fs::write(&path, &image.bytes).map(|()| path).map_err(|error| error.to_string())) {
            Ok(path) => self.send(HostMessage::ImagePasted { path: path.display().to_string() }),
            Err(error) => self.invalid = Some(format!("The picture could not be passed on: {error}")),
        }
        true
    }

    pub fn notify_citation_picked(&self, key: &str) {
        if self.citations.iter().any(|entry| entry.key == key) {
            self.send(HostMessage::CitationPicked { key: key.to_string() });
        }
    }

    pub fn looks_up_dois(&self) -> bool {
        self.add_on.manifest.permissions.contains(&Permission::LookupDoi)
    }

    /// Asks the add-on to open its dialog. Fails with the reason when it does not run.
    pub fn lookup_doi(&mut self, suggestion: String, cx: &mut Context<Self>) -> Result<(), String> {
        if self.process.is_none() {
            self.start(cx);
        }
        if self.process.is_none() {
            return Err(self.stopped.clone().unwrap_or_else(|| format!("{} does not run.", self.add_on.manifest.name)));
        }
        self.dialog_asked = true;
        self.send(HostMessage::LookupDoi { suggestion });
        Ok(())
    }

    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    pub fn close_dialog(&mut self, cx: &mut Context<Self>) {
        if self.dialog.take().is_some() {
            self.send(HostMessage::DialogClosed);
            self.sync_view(cx);
            cx.notify();
        }
    }

    /// `Enter` in the dialog. An input method that composes keeps it.
    pub fn confirm_dialog(&mut self, cx: &mut Context<Self>) {
        if self.inputs.values().any(|input| input.read(cx).is_composing()) {
            return;
        }
        let button = self.dialog.as_ref().and_then(default_button).map(|(id, submits)| (id.to_string(), submits.map(str::to_string)));
        if let Some((id, submits)) = button {
            self.click(&id, submits.as_deref(), cx);
        }
    }

    /// What should hold the focus while the dialog shows.
    pub fn dialog_input(&self, cx: &App) -> Option<gpui::FocusHandle> {
        let mut inputs = Vec::new();
        collect_ids(self.dialog.as_ref()?, &mut inputs, &mut Vec::new());
        inputs.first().and_then(|id| self.inputs.get(id)).map(|input| input.read(cx).focus_handle(cx))
    }

    pub fn render_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.take()?;
        let element = self.render_node(&dialog, cx);
        self.dialog = Some(dialog);
        Some(element)
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if visible && self.process.is_none() && self.stopped.is_none() {
            self.start(cx);
        }
    }

    /// An add-on works on one project, so another one means a new start.
    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.root = root;
        self.stop();
        cx.emit(AddonPanelEvent::Stopped);
        self.stopped = None;
        self.view = Node::default();
        self.dialog = None;
        cx.emit(AddonPanelEvent::DialogChanged);
        self.citations.clear();
        self.toggled.clear();
        self.sync_view(cx);
        if self.visible || self.starts_with_project() {
            self.start(cx);
        }
        cx.notify();
    }

    fn stop(&mut self) {
        self.task = None;
        self.process = None;
        self.subscriptions.clear();
        self.status.clear();
    }


    fn start(&mut self, cx: &mut Context<Self>) {
        self.invalid = None;
        let (process, mut events) = match AddonProcess::start(&self.add_on) {
            Ok(started) => started,
            Err(error) => {
                self.stopped = Some(error);
                cx.notify();
                return;
            }
        };
        let project = self.root.as_ref().map(|root| root.display().to_string());
        // An add-on gets the version it was written for, and features only from 2 on.
        let api = self.add_on.manifest.api.min(API_VERSION);
        let features = if api >= 2 { FEATURES.iter().map(ToString::to_string).collect() } else { Vec::new() };
        process.send(HostMessage::Init { api, project, host_version: env!("CARGO_PKG_VERSION").to_string(), features });
        self.process = Some(process);
        self.stopped = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                let mut batch = vec![event];
                while batch.len() < BATCH {
                    match events.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                let handled = this.update(cx, |this, cx| {
                    for event in batch {
                        this.handle_event(event, cx);
                    }
                    if std::mem::take(&mut this.scroll_pending) {
                        this.scroll_to_end();
                    }
                    cx.notify();
                });
                if handled.is_err() {
                    return;
                }
            }
        }));
        cx.notify();
    }

    /// Handles one message. The caller draws once after a batch of them.
    fn handle_event(&mut self, event: ProcessEvent, cx: &mut Context<Self>) {
        match event {
            ProcessEvent::Message(AddonMessage::SetView { view }) => {
                self.view = view;
                self.view_changed(cx);
            }
            ProcessEvent::Message(AddonMessage::Append { id, text }) => {
                let Some(node) = self.index.get(&id).and_then(|path| node_at_mut(&mut self.view, path)) else {
                    return;
                };
                if let Some(content) = append(node, &text) {
                    if let Some(document) = self.documents.get_mut(&id) {
                        document.update(content);
                    }
                    self.scroll_pending = true;
                }
            }
            ProcessEvent::Message(AddonMessage::Insert { parent, index, node }) => {
                let Some(children) = self.index.get(&parent).and_then(|path| node_at_mut(&mut self.view, path)).and_then(Node::children_mut) else {
                    return;
                };
                let at = index.unwrap_or(children.len()).min(children.len());
                children.insert(at, node);
                self.view_changed(cx);
            }
            ProcessEvent::Message(AddonMessage::Replace { id, node }) => {
                if let Some(target) = self.index.get(&id).and_then(|path| node_at_mut(&mut self.view, path)) {
                    *target = node;
                    self.view_changed(cx);
                }
            }
            ProcessEvent::Message(AddonMessage::Remove { id }) => {
                let Some(path) = self.index.get(&id).cloned() else {
                    return;
                };
                if let Some((last, parent)) = path.split_last() {
                    if let Some(children) = node_at_mut(&mut self.view, parent).and_then(Node::children_mut) {
                        if *last < children.len() {
                            children.remove(*last);
                        }
                    }
                    self.view_changed(cx);
                }
            }
            ProcessEvent::Message(AddonMessage::Notify { text, .. }) => cx.emit(AddonPanelEvent::Notify(text)),
            ProcessEvent::Message(AddonMessage::SetStatus { mut items }) => {
                if self.speaks_two() {
                    // A line, not a page: a few short items.
                    items.truncate(8);
                    self.status = items;
                }
            }
            ProcessEvent::Message(AddonMessage::Subscribe { events }) => {
                if self.speaks_two() {
                    let had_project = self.subscriptions.contains(&EventKind::Project);
                    self.subscriptions = events.into_iter().collect();
                    // The program starts again with each project, so this is the one it works on.
                    if !had_project
                        && self.subscriptions.contains(&EventKind::Project)
                        && let Some(root) = &self.root
                    {
                        self.send(HostMessage::ProjectOpened { root: root.display().to_string() });
                    }
                }
            }
            ProcessEvent::Message(AddonMessage::Request { id, request }) => {
                let permission = request.permission();
                if self.add_on.manifest.permissions.contains(&permission) {
                    cx.emit(AddonPanelEvent::Request { id, request });
                } else {
                    self.send(HostMessage::Reply {
                        id,
                        result: Err(format!("The add-on did not ask for the permission to: {}.", permission.description())),
                    });
                }
            }
            ProcessEvent::Message(AddonMessage::SetCitations { entries }) => {
                if self.add_on.manifest.permissions.contains(&Permission::Citations) {
                    self.citations = entries;
                }
            }
            ProcessEvent::Message(AddonMessage::SetDialog { view }) => {
                if self.looks_up_dois() {
                    self.dialog_asked = false;
                    self.dialog = view;
                    self.view_changed(cx);
                    cx.emit(AddonPanelEvent::DialogChanged);
                }
            }
            ProcessEvent::Invalid(error) => self.invalid = Some(error),
            ProcessEvent::Exited(reason) => {
                if std::mem::take(&mut self.dialog_asked) {
                    cx.emit(AddonPanelEvent::LookupFailed(format!("{} stopped: {reason}", self.add_on.manifest.name)));
                }
                self.process = None;
                self.subscriptions.clear();
                self.status.clear();
                cx.emit(AddonPanelEvent::Stopped);
                self.citations.clear();
                self.dialog = None;
                cx.emit(AddonPanelEvent::DialogChanged);
                self.stopped = Some(reason);
            }
        }
    }

    /// After the shape of the view changed: the ids, the inputs and the
    /// parsed Markdown follow it.
    fn view_changed(&mut self, cx: &mut Context<Self>) {
        self.index.clear();
        index_ids(&self.view, &mut Vec::new(), &mut self.index);
        let mut texts = Vec::new();
        collect_markdown(&self.view, &mut texts);
        if let Some(dialog) = &self.dialog {
            collect_markdown(dialog, &mut texts);
        }
        self.documents.retain(|id, _| texts.iter().any(|(text_id, _)| text_id == id));
        for (id, text) in texts {
            self.documents.entry(id.to_string()).or_default().update(text);
        }
        self.sync_view(cx);
        self.scroll_pending = true;
    }

    pub fn reply(&self, id: u64, result: Result<Reply, String>) {
        self.send(HostMessage::Reply { id, result });
    }

    fn send(&self, message: HostMessage) {
        if let Some(process) = &self.process {
            process.send(message);
        }
    }

    fn scroll_to_end(&self) {
        let mut following = Vec::new();
        collect_following(&self.view, &mut following);
        for scroll in following.iter().filter_map(|id| self.scrolls.get(*id)) {
            scroll.scroll_to_bottom();
        }
    }

    /// Keeps inputs and their text by id, so that a new view does not lose what
    /// the user is typing.
    fn sync_view(&mut self, cx: &mut Context<Self>) {
        let (mut inputs, mut scrolls) = (Vec::new(), Vec::new());
        collect_ids(&self.view, &mut inputs, &mut scrolls);
        if let Some(dialog) = &self.dialog {
            collect_ids(dialog, &mut inputs, &mut scrolls);
        }
        self.inputs.retain(|id, _| inputs.contains(id));
        self.scrolls.retain(|id, _| scrolls.contains(id));
        for id in inputs {
            let value = self.dialog.iter().chain([&self.view]).find_map(|node| input_value(node, &id)).unwrap_or_default().to_string();
            self.inputs.entry(id).or_insert_with(|| {
                let input = cx.new(|cx| {
                    let mut view = EditorView::new(&value, None, cx);
                    view.set_plain();
                    view
                });
                cx.observe(&input, |_, _, cx| cx.notify()).detach();
                input
            });
        }
        for id in scrolls {
            self.scrolls.entry(id).or_default();
        }
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.inputs.values().any(|input| input.read(cx).focus_handle(cx).is_focused(window))
            || self.open_dropdown.as_ref().is_some_and(|open| open.focus.is_focused(window))
    }

    pub fn shut_down(&mut self) {
        if let Some(process) = &mut self.process {
            process.shut_down();
        }
    }

    fn close_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = self.open_dropdown.take().and_then(|open| open.previous) {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn choose_dropdown_option(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.open_dropdown.as_ref().map(|open| open.id.clone()) {
            self.send(HostMessage::Select { id, selected: index });
        }
        self.close_dropdown(window, cx);
    }

    fn dropdown_key(&mut self, event: &KeyDownEvent, count: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open_dropdown.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "down" | "j" => open.highlighted = (open.highlighted + 1) % count,
            "up" | "k" => open.highlighted = (open.highlighted + count - 1) % count,
            "enter" => {
                let index = open.highlighted;
                return self.choose_dropdown_option(index, window, cx);
            }
            "escape" => return self.close_dropdown(window, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn render_dropdown(&self, id: &str, options: &[DropdownOption], selected: usize, cx: &mut Context<Self>) -> AnyElement {
        self.render_dropdown_toward(id, options, selected, true, cx)
    }

    /// `upward` for a dropdown low in the panel, downward for one at its top.
    fn render_dropdown_toward(&self, id: &str, options: &[DropdownOption], selected: usize, upward: bool, cx: &mut Context<Self>) -> AnyElement {
        let dropdown = id.to_string();
        let open = self.open_dropdown.as_ref().filter(|open| open.id == id);
        let label = options.get(selected).map_or_else(String::new, |option| option.label.clone());
        div()
            .id(SharedString::from(format!("dropdown-{id}")))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(2.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(open.is_some(), |style| style.bg(theme::color(theme::palette().panel)))
            .hover(|style| style.bg(theme::color(theme::palette().panel)))
            .on_click(cx.listener(move |this, _, window, cx| {
                let focus = cx.focus_handle();
                let previous = window.focused(cx);
                window.focus(&focus, cx);
                this.open_dropdown = Some(OpenDropdown { id: dropdown.clone(), highlighted: selected, focus, previous });
                cx.notify();
            }))
            .child(label)
            .child(
                gpui::svg()
                    .path("icons/chevron-down.svg")
                    .size(px(12.))
                    .flex_none()
                    .text_color(theme::color(theme::palette().ui_muted)),
            )
            .children(open.map(|open| self.render_dropdown_list(open, options, selected, upward, cx)))
            .into_any_element()
    }

    /// The list opens upward from the button, since dropdowns sit low in a panel.
    /// The backdrop takes a click elsewhere, so that it only closes the list.
    fn render_dropdown_list(&self, open: &OpenDropdown, options: &[DropdownOption], selected: usize, upward: bool, cx: &mut Context<Self>) -> AnyElement {
        let count = options.len().max(1);
        let backdrop = anchored().position(point(px(0.), px(0.))).child(
            div()
                .id("dropdown-backdrop")
                .w(self.viewport.width)
                .h(self.viewport.height)
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_dropdown(window, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.close_dropdown(window, cx))),
        );
        let list = anchored().anchor(if upward { Anchor::BottomLeft } else { Anchor::TopLeft }).snap_to_window_with_margin(px(8.)).child(
            div()
                .id("dropdown-list")
                .track_focus(&open.focus)
                .flex()
                .flex_col()
                .w(px(DROPDOWN_WIDTH))
                .when(upward, |style| style.mb(px(4.)))
                .when(!upward, |style| style.mt(px(22.)))
                .p(px(4.))
                .gap(px(2.))
                .rounded(px(8.))
                .border_1()
                .border_color(theme::color(theme::palette().divider))
                .bg(theme::color(theme::palette().panel))
                .shadow_lg()
                .occlude()
                .on_action(cx.listener(|this, _: &CloseMenu, window, cx| this.close_dropdown(window, cx)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| this.dropdown_key(event, count, window, cx)))
                .children(options.iter().enumerate().map(|(index, option)| {
                    div()
                        .id(("dropdown-option", index))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(index == open.highlighted, |style| style.bg(theme::color(theme::palette().divider)))
                        .hover(|style| style.bg(theme::color(theme::palette().divider)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.choose_dropdown_option(index, window, cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(option.label.clone())
                                .when(!option.description.is_empty(), |element| {
                                    element.child(div().text_color(theme::color(theme::palette().ui_muted)).child(option.description.clone()))
                                }),
                        )
                        .when(index == selected, |element| element.child(div().flex_none().text_color(theme::color(theme::palette().accent)).child("✓")))
                })),
        );
        div().child(deferred(backdrop).with_priority(1)).child(deferred(list).with_priority(2)).into_any_element()
    }

    fn click(&mut self, id: &str, submits: Option<&str>, cx: &mut Context<Self>) {
        let Some(input) = submits.and_then(|input| self.inputs.get(input)).cloned() else {
            self.send(HostMessage::Click { id: id.to_string() });
            return;
        };
        let text = input.read(cx).state.buffer().to_string();
        input.update(cx, |input, cx| input.clear(cx));
        self.send(HostMessage::Submit { id: id.to_string(), text });
    }

    /// `⌘↩` in an input acts like the button that submits it.
    fn submit_focused(&mut self, window: &Window, cx: &mut Context<Self>) {
        let focused = self.inputs.iter().find(|(_, input)| input.read(cx).focus_handle(cx).is_focused(window));
        let Some(input) = focused.map(|(id, _)| id.clone()) else {
            return;
        };
        if let Some(button) = button_submitting(&self.view, &input).map(str::to_string) {
            self.click(&button, Some(&input), cx);
        }
    }

    fn render_node(&self, node: &Node, cx: &mut Context<Self>) -> AnyElement {
        match node {
            Node::Column { children, .. } => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .when(contains_scroll(node), |style| style.flex_1().min_h_0())
                .children(children.iter().map(|child| self.render_node(child, cx)).collect::<Vec<_>>())
                .into_any_element(),
            Node::Row { children, .. } => div()
                .flex()
                .flex_none()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .children(children.iter().map(|child| self.render_node(child, cx)).collect::<Vec<_>>())
                .into_any_element(),
            Node::Scroll { id, children, .. } => {
                let mut scroll = div()
                    .id(SharedString::from(id.clone()))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(6.))
                    .overflow_y_scroll();
                if let Some(handle) = self.scrolls.get(id) {
                    scroll = scroll.track_scroll(handle);
                }
                scroll.children(children.iter().map(|child| self.render_node(child, cx)).collect::<Vec<_>>()).into_any_element()
            }
            Node::Text { text, muted, .. } => div()
                .flex_none()
                .when(*muted, |style| style.text_color(theme::color(theme::palette().ui_muted)))
                .child(text.clone())
                .into_any_element(),
            Node::Code { text, .. } => div()
                .flex_none()
                .p(px(6.))
                .rounded(px(4.))
                .bg(theme::color(theme::palette().panel))
                .font_family(theme::code_font())
                .text_size(px(theme::code_font_size()))
                .child(text.clone())
                .into_any_element(),
            Node::Button { id, label, primary, enabled, submits } => {
                let (button, submits, enabled) = (id.clone(), submits.clone(), *enabled);
                div()
                    .id(SharedString::from(id.clone()))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .px(px(10.))
                    .h(px(26.))
                    .rounded(px(4.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(theme::color(if *primary && enabled { theme::palette().accent } else { theme::palette().divider }))
                    .when(*primary && enabled, |style| style.text_color(theme::color(theme::palette().on_accent)))
                    .when(enabled, |style| style.cursor_pointer())
                    .when(!enabled, |style| style.text_color(theme::color(theme::palette().ui_muted)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if enabled {
                            this.click(&button, submits.as_deref(), cx);
                        }
                    }))
                    .child(label.clone())
                    .into_any_element()
            }
            Node::Input { id, placeholder, multiline, .. } => {
                let input = self.inputs.get(id);
                let empty = input.is_none_or(|input| input.read(cx).state.buffer().is_empty());
                let height = if *multiline {
                    // Grows with its lines, up to a few, then scrolls.
                    let lines = input.map_or(1, |input| input.read(cx).state.buffer().to_string().lines().count().max(1));
                    lines.clamp(2, MULTILINE_ROWS) as f32 * theme::code_line_height() + 8.
                } else {
                    INPUT_HEIGHT
                };
                let multiline = *multiline;
                div()
                    .relative()
                    .flex_none()
                    .h(px(height))
                    // Enter sends and Shift-Enter starts a new line, as in a chat.
                    .when(multiline, |element| {
                        let line_break = input.cloned();
                        element
                            .capture_action(cx.listener(|this, _: &Newline, window, cx| {
                                if !this.inputs.values().any(|input| input.read(cx).is_composing()) {
                                    this.submit_focused(window, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .capture_key_down(move |event: &KeyDownEvent, _, cx| {
                                let modifiers = event.keystroke.modifiers;
                                if event.keystroke.key == "enter" && modifiers.shift && !modifiers.platform && !modifiers.control {
                                    if let Some(input) = &line_break {
                                        input.update(cx, |input, cx| input.insert_text("\n", cx));
                                        cx.stop_propagation();
                                    }
                                }
                            })
                    })
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::color(theme::palette().divider))
                    .overflow_hidden()
                    .children(input.cloned())
                    .when(empty, |element| {
                        element.child(
                            div()
                                .absolute()
                                .top_0()
                                .left(px(6.))
                                .font_family(theme::code_font())
                                .text_size(px(theme::code_font_size()))
                                .line_height(px(theme::code_line_height()))
                                .text_color(theme::color(theme::palette().ui_muted))
                                .child(placeholder.clone()),
                        )
                    })
                    .into_any_element()
            }
            Node::Select { id, options, selected } => div()
                .flex()
                .flex_none()
                .flex_wrap()
                .p(px(2.))
                .gap(px(2.))
                .rounded(px(6.))
                .bg(theme::color(theme::palette().panel))
                .border_1()
                .border_color(theme::color(theme::palette().divider))
                .children(options.iter().enumerate().map(|(index, option)| {
                    let select = id.clone();
                    let chosen = index == *selected;
                    div()
                        .id(ElementId::NamedInteger(id.clone().into(), index as u64))
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(chosen, |style| {
                            style
                                .bg(theme::color(theme::palette().accent))
                                .text_color(theme::color(theme::palette().on_accent))
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(!chosen, |style| style.hover(|style| style.bg(theme::color(theme::palette().divider))))
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.send(HostMessage::Select { id: select.clone(), selected: index });
                        }))
                        .child(option.clone())
                }))
                .into_any_element(),
            Node::Dropdown { id, options, selected } => self.render_dropdown(id, options, *selected, cx),
            Node::Check { id, label, checked, indent } => {
                let check = id.clone();
                div()
                    .id(SharedString::from(id.clone()))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(*indent as f32 * CHECK_INDENT))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::color(theme::palette().panel)))
                    .on_click(cx.listener(move |this, _, _, _| this.send(HostMessage::Click { id: check.clone() })))
                    .child(
                        div()
                            .flex_none()
                            .size(px(CHECK_SIZE))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::color(if *checked { theme::palette().accent } else { theme::palette().ui_muted }))
                            .when(*checked, |style| style.bg(theme::color(theme::palette().accent))),
                    )
                    .child(label.clone())
                    .into_any_element()
            }
            Node::Alert { text } => {
                div().flex_none().p(px(8.)).rounded(px(4.)).bg(theme::color(theme::palette().error_panel)).font_weight(FontWeight::SEMIBOLD).child(text.clone()).into_any_element()
            }
            Node::Divider => div().flex_none().h(px(1.)).bg(theme::color(theme::palette().panel)).into_any_element(),
            Node::Markdown { id, text } => match id.as_deref().and_then(|id| self.documents.get(id)) {
                Some(document) => self.render_blocks(&document.blocks, cx),
                // Without an id it cannot stream, so it is short and read here.
                None => {
                    let blocks: Vec<Block> = markdown::parse(text).into_iter().map(|(_, block)| block).collect();
                    self.render_blocks(&blocks, cx)
                }
            },
            Node::Message { role, children, .. } => {
                let palette = theme::palette();
                let content = div().flex().flex_col().gap(px(6.)).min_w_0().children(children.iter().map(|child| self.render_node(child, cx)).collect::<Vec<_>>());
                match role {
                    Role::User => div()
                        .flex()
                        .flex_none()
                        .justify_end()
                        .child(div().max_w(relative_width(0.85)).px(px(10.)).py(px(6.)).rounded(px(10.)).bg(theme::color(palette.panel)).child(content))
                        .into_any_element(),
                    Role::Assistant => div().flex_none().child(content).into_any_element(),
                    Role::Tool | Role::System => div().flex_none().text_size(px(12.)).text_color(theme::color(palette.ui_muted)).child(content).into_any_element(),
                }
            }
            Node::Collapsible { id, title, open, children } => {
                let open = self.toggled.get(id).copied().unwrap_or(*open);
                let toggle = id.clone();
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(4.))
                    .child(
                        div()
                            .id(SharedString::from(format!("collapsible-{id}")))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .text_color(theme::color(theme::palette().ui_muted))
                            .hover(|style| style.text_color(theme::color(theme::palette().ui_text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggled.insert(toggle.clone(), !open);
                                cx.notify();
                            }))
                            .child(gpui::svg().path(if open { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" }).size(px(12.)).flex_none().text_color(theme::color(theme::palette().ui_muted)))
                            .child(div().min_w_0().truncate().child(title.clone())),
                    )
                    .when(open, |element| {
                        element.child(div().flex().flex_col().gap(px(6.)).pl(px(16.)).children(children.iter().map(|child| self.render_node(child, cx)).collect::<Vec<_>>()))
                    })
                    .into_any_element()
            }
            Node::Card { tone, children, .. } => {
                let palette = theme::palette();
                let border = match tone {
                    Tone::Info => palette.divider,
                    Tone::Success => palette.accent,
                    Tone::Warning => palette.warning,
                    Tone::Danger => palette.error,
                };
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(6.))
                    .p(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme::color(border))
                    .bg(theme::color(palette.panel))
                    .children(children.iter().map(|child| self.render_node(child, cx)).collect::<Vec<_>>())
                    .into_any_element()
            }
            Node::Chips { items, .. } => div()
                .flex()
                .flex_none()
                .flex_wrap()
                .gap(px(4.))
                .children(items.iter().map(|chip| {
                    let remove = chip.id.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(10.))
                        .text_size(px(12.))
                        .bg(theme::color(theme::palette().panel))
                        .border_1()
                        .border_color(theme::color(theme::palette().divider))
                        .child(chip.label.clone())
                        .when(chip.removable, |element| {
                            element.child(
                                div()
                                    .id(SharedString::from(format!("chip-{}", chip.id)))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, _| this.send(HostMessage::Click { id: remove.clone() })))
                                    .child(gpui::svg().path("icons/close.svg").size(px(10.)).text_color(theme::color(theme::palette().ui_muted))),
                            )
                        })
                }))
                .into_any_element(),
            Node::Image { path, max_height } => div()
                .flex_none()
                .w_full()
                .h(px(max_height.unwrap_or(240) as f32))
                .child(img(PathBuf::from(path)).size_full().object_fit(ObjectFit::Contain))
                .into_any_element(),
            Node::Draggable { id, payload, children } => {
                let (content, label) = match payload {
                    addon_api::DragPayload::Text { text } => (DragContent::Text(text.clone()), text.chars().take(40).collect::<String>()),
                    addon_api::DragPayload::File { path } => {
                        let path = PathBuf::from(path);
                        let label = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                        (DragContent::File(path), label)
                    }
                };
                let item = DragItem { content, origin: Some((self.add_on.manifest.id.clone(), id.clone())), label: label.into() };
                div()
                    .id(SharedString::from(format!("draggable-{id}")))
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(6.))
                    .cursor_grab()
                    .on_drag(item, |item, _, _, cx| cx.new(|_| DragPreview(item.label.clone())))
                    .children(children.iter().map(|child| self.render_node(child, cx)).collect::<Vec<_>>())
                    .into_any_element()
            }
            Node::Unknown => div().into_any_element(),
        }
    }

    /// Facts in one line, with the choices among them as small dropdowns.
    fn render_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = theme::palette();
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .pb(px(4.))
            .text_size(px(12.))
            .border_b_1()
            .border_color(theme::color(palette.divider))
            .children(self.status.iter().map(|item| {
                let color = match item.tone {
                    Tone::Info | Tone::Success => palette.ui_muted,
                    Tone::Warning => palette.warning,
                    Tone::Danger => palette.error,
                };
                if item.options.is_empty() {
                    let id = item.id.clone();
                    div()
                        .id(SharedString::from(format!("status-{}", item.id)))
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .text_color(theme::color(color))
                        .hover(|style| style.bg(theme::color(palette.panel)))
                        .on_click(cx.listener(move |this, _, _, _| this.send(HostMessage::Click { id: id.clone() })))
                        .child(item.text.clone())
                        .into_any_element()
                } else {
                    self.render_dropdown_toward(&item.id, &item.options, item.selected, false, cx)
                }
            }))
            .into_any_element()
    }

    fn render_blocks(&self, blocks: &[Block], cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(6.))
            .min_w_0()
            .children(blocks.iter().enumerate().map(|(index, block)| self.render_block(index, block, cx)).collect::<Vec<_>>())
            .into_any_element()
    }

    fn render_block(&self, index: usize, block: &Block, cx: &mut Context<Self>) -> AnyElement {
        let palette = theme::palette();
        markdown_view::block(index, block, Look::PANEL, None, |language, text| {
            let (copy, insert) = (text.clone(), text.clone());
            let action = |id: String, label: &'static str| {
                div()
                    .id(SharedString::from(id))
                    .px(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(theme::color(palette.ui_muted))
                    .hover(|style| style.bg(theme::color(palette.divider)).text_color(theme::color(palette.ui_text)))
                    .child(label)
            };
            Some(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(6.))
                    .py(px(2.))
                    .text_size(px(11.))
                    .child(div().text_color(theme::color(palette.ui_muted)).child(language.clone()))
                    .child(
                        div()
                            .flex()
                            .gap(px(2.))
                            .child(action(format!("code-copy-{index}-{}", text.len()), "Copy").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()))))
                            .child(action(format!("code-insert-{index}-{}", text.len()), "Insert at cursor").on_click(cx.listener(move |_, _, _, cx| cx.emit(AddonPanelEvent::InsertText(insert.to_string()))))),
                    )
                    .into_any_element(),
            )
        })
    }
}

fn relative_width(fraction: f32) -> gpui::DefiniteLength {
    gpui::relative(fraction)
}

/// Adds to a `Text`, `Code` or `Markdown` node and returns its whole text.
fn append<'a>(node: &'a mut Node, addition: &str) -> Option<&'a str> {
    let (Node::Text { text, .. } | Node::Code { text, .. } | Node::Markdown { text, .. }) = node else {
        return None;
    };
    text.push_str(addition);
    // An add-on can stream for as long as it runs, and only the end stays useful.
    if text.len() > MAX_APPENDED_BYTES * 2 {
        let cut = text.ceil_char_boundary(text.len() - MAX_APPENDED_BYTES);
        text.drain(..cut);
    }
    Some(text)
}

fn node_at_mut<'a>(node: &'a mut Node, path: &[usize]) -> Option<&'a mut Node> {
    let Some((first, rest)) = path.split_first() else {
        return Some(node);
    };
    node_at_mut(node.children_mut()?.get_mut(*first)?, rest)
}

/// Every node with an id, by its path of child indexes. The first of a
/// repeated id wins.
fn index_ids(node: &Node, path: &mut Vec<usize>, index: &mut HashMap<String, Vec<usize>>) {
    if let Some(id) = node.id() {
        index.entry(id.to_string()).or_insert_with(|| path.clone());
    }
    for (position, child) in node.children().iter().enumerate() {
        path.push(position);
        index_ids(child, path, index);
        path.pop();
    }
}

fn collect_markdown<'a>(node: &'a Node, texts: &mut Vec<(&'a str, &'a str)>) {
    if let Node::Markdown { id: Some(id), text } = node {
        texts.push((id, text));
    }
    node.children().iter().for_each(|child| collect_markdown(child, texts));
}

fn collect_ids(node: &Node, inputs: &mut Vec<String>, scrolls: &mut Vec<String>) {
    match node {
        Node::Input { id, .. } => inputs.push(id.clone()),
        Node::Scroll { id, .. } => scrolls.push(id.clone()),
        _ => {}
    }
    node.children().iter().for_each(|child| collect_ids(child, inputs, scrolls));
}

fn contains_dropdown(node: &Node, target: &str) -> bool {
    match node {
        Node::Dropdown { id, .. } => id == target,
        _ => node.children().iter().any(|child| contains_dropdown(child, target)),
    }
}

fn input_value<'a>(node: &'a Node, target: &str) -> Option<&'a str> {
    match node {
        Node::Input { id, value, .. } if id == target => Some(value),
        _ => node.children().iter().find_map(|child| input_value(child, target)),
    }
}

fn default_button(node: &Node) -> Option<(&str, Option<&str>)> {
    match node {
        Node::Button { id, primary: true, enabled: true, submits, .. } => Some((id, submits.as_deref())),
        _ => node.children().iter().find_map(default_button),
    }
}

/// The selection or clipboard, if an add-on may see it as a DOI or arXiv id. Other
/// text on the clipboard stays with the editor.
pub fn doi_suggestion(text: &str) -> Option<String> {
    let text = text.trim();
    let lower = text.to_lowercase();
    let is_arxiv_id = |word: &str| {
        word.split_once('.').is_some_and(|(left, right)| {
            left.len() == 4 && left.chars().all(|ch| ch.is_ascii_digit()) && right.len() >= 4 && right.chars().take(4).all(|ch| ch.is_ascii_digit())
        })
    };
    let fits = text.len() <= SUGGESTION_LIMIT && !text.contains(char::is_whitespace);
    (fits && (lower.contains("10.") && lower.contains('/') || lower.contains("arxiv") || is_arxiv_id(&lower))).then(|| text.to_string())
}

fn collect_following<'a>(node: &'a Node, following: &mut Vec<&'a str>) {
    if let Node::Scroll { id, follow: true, .. } = node {
        following.push(id);
    }
    node.children().iter().for_each(|child| collect_following(child, following));
}

fn contains_scroll(node: &Node) -> bool {
    matches!(node, Node::Scroll { .. }) || node.children().iter().any(contains_scroll)
}

fn button_submitting<'a>(node: &'a Node, input: &str) -> Option<&'a str> {
    match node {
        Node::Button { id, submits: Some(submits), enabled: true, .. } if submits == input => Some(id),
        _ => node.children().iter().find_map(|child| button_submitting(child, input)),
    }
}

impl Render for AddonPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport = window.viewport_size();
        // The add-on can take the dropdown away while its list shows.
        if self.open_dropdown.as_ref().is_some_and(|open| !contains_dropdown(&self.view, &open.id) && !self.status.iter().any(|item| item.id == open.id && !item.options.is_empty())) {
            self.close_dropdown(window, cx);
        }
        // Taken out and put back, so that drawing never copies the whole view.
        let tree = std::mem::take(&mut self.view);
        let view = self.render_node(&tree, cx);
        self.view = tree;
        let failure = self.stopped.clone().map(|reason| format!("{} stopped. {reason}", self.add_on.manifest.name));
        let invalid = self.invalid.clone().map(|error| format!("The add-on sent a broken message: {error}"));
        let restart = failure.is_some();
        let banner = |text: String| div().flex_none().p(px(8.)).rounded(px(4.)).bg(theme::color(theme::palette().error_panel)).text_size(px(12.)).child(text);
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(6.))
            .p(px(8.))
            .on_action(cx.listener(|this, _: &Recompile, window, cx| this.submit_focused(window, cx)))
            .when(self.speaks_two(), |element| {
                element
                    .capture_action(cx.listener(|this, _: &Paste, _, cx| {
                        if this.paste_image(cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(theme::color(theme::palette().panel)))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, _| {
                        let paths = paths.paths().iter().map(|path| path.display().to_string()).collect();
                        this.send(HostMessage::FilesDropped { paths });
                    }))
            })
            .when(!self.status.is_empty(), |element| element.child(self.render_status(cx)))
            .children(failure.map(banner))
            .children(invalid.map(banner))
            .when(restart, |element| {
                element.child(
                    div()
                        .id("addon-restart")
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .h(px(26.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .bg(theme::color(theme::palette().divider))
                        .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                        .child("Restart"),
                )
            })
            .child(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(id: &str, value: &str) -> Node {
        Node::Text { id: Some(id.to_string()), text: value.to_string(), muted: false }
    }

    #[test]
    fn only_what_looks_like_an_id_reaches_the_add_on() {
        for text in ["10.1145/3442188.3445922", " https://doi.org/10.1038/x\n", "1706.03762v5", "arXiv:hep-th/9901001"] {
            assert_eq!(doi_suggestion(text).as_deref(), Some(text.trim()));
        }
        for text in ["", "password123", "sk-ant-abc", "a sentence with 10.1/x in it", &"10.1/".repeat(100)] {
            assert_eq!(doi_suggestion(text), None, "{text}");
        }
    }

    #[test]
    fn enter_finds_the_first_enabled_primary_button() {
        let button = |id: &str, primary, enabled| Node::Button { id: id.to_string(), label: id.to_string(), primary, enabled, submits: None };
        let view = Node::Column {
            id: None,
            children: vec![Node::Row { id: None, children: vec![button("a", false, true), button("b", true, false), button("c", true, true)] }],
        };
        assert_eq!(default_button(&view), Some(("c", None)));
        assert_eq!(default_button(&button("a", false, true)), None);
    }

    #[test]
    fn nodes_are_found_by_id_and_appended_to() {
        let mut view = Node::Column {
            id: Some("root".to_string()),
            children: vec![Node::Scroll { id: "s".to_string(), children: vec![text("a", "x"), Node::Markdown { id: Some("b".to_string()), text: "y".to_string() }], follow: true }],
        };
        let mut index = HashMap::new();
        index_ids(&view, &mut Vec::new(), &mut index);
        assert_eq!(index["root"], Vec::<usize>::new());
        assert_eq!(index["b"], vec![0, 1]);
        let node = node_at_mut(&mut view, &index["b"]).unwrap();
        assert_eq!(append(node, "z"), Some("yz"));
        assert_eq!(append(node_at_mut(&mut view, &index["s"]).unwrap(), "z"), None);
        assert!(node_at_mut(&mut view, &[0, 9]).is_none());
    }

    #[test]
    fn submit_finds_the_button_of_an_input() {
        let button = |id: &str, submits: &str, enabled: bool| Node::Button {
            id: id.to_string(),
            label: String::new(),
            primary: false,
            enabled,
            submits: Some(submits.to_string()),
        };
        let view = Node::Row { id: None, children: vec![button("off", "a", false), button("other", "b", true), button("send", "a", true)] };
        assert_eq!(button_submitting(&view, "a"), Some("send"));
        assert_eq!(button_submitting(&view, "c"), None);
    }

    #[test]
    fn ids_are_collected_at_any_depth() {
        let view = Node::Column {
            id: None,
            children: vec![
                Node::Scroll {
                    id: "s".to_string(),
                    children: vec![Node::Input { id: "i".to_string(), placeholder: String::new(), value: String::new(), multiline: false }],
                    follow: true,
                },
            ],
        };
        let (mut inputs, mut scrolls) = (Vec::new(), Vec::new());
        collect_ids(&view, &mut inputs, &mut scrolls);
        assert_eq!((inputs, scrolls), (vec!["i".to_string()], vec!["s".to_string()]));
        assert!(contains_scroll(&view));
    }

    #[test]
    fn only_following_scrolls_jump_to_the_end() {
        let scroll = |id: &str, follow| Node::Scroll { id: id.to_string(), children: Vec::new(), follow };
        let view = Node::Column { id: None, children: vec![scroll("chat", true), scroll("list", false)] };
        let mut following = Vec::new();
        collect_following(&view, &mut following);
        assert_eq!(following, ["chat"]);
    }
}
