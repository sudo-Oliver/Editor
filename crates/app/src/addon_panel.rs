//! Draws the panel of an add-on and passes messages between it and the workspace.
//! The program starts when the panel first shows, or with the project if the
//! manifest asks for that.

use std::{collections::HashMap, path::PathBuf};

use addon_api::{API_VERSION, Activation, AddonMessage, CitationEntry, DropdownOption, HostMessage, Node, Permission, Reply, Request};
use gpui::{
    Anchor, AnyElement, App, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseButton,
    Pixels, Render, ScrollHandle, SharedString, Size, Task, Window, anchored, deferred, div, point, prelude::*, px, size,
};

use crate::{
    actions::{CloseMenu, Recompile},
    addons::{AddonProcess, Installed, ProcessEvent},
    editor_view::EditorView,
    theme,
};

const INPUT_HEIGHT: f32 = 62.0;
const CHECK_SIZE: f32 = 12.0;
const CHECK_INDENT: f32 = 14.0;
const SUGGESTION_LIMIT: usize = 300;
const DROPDOWN_WIDTH: f32 = 200.0;

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

    /// Tells the add-on about a saved file, if it runs with the project.
    pub fn notify_saved(&self, path: &str) {
        if self.add_on.manifest.activation == Activation::Project {
            self.send(HostMessage::Saved { path: path.to_string() });
        }
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
        let dialog = self.dialog.clone()?;
        Some(self.render_node(&dialog, cx))
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
        self.stopped = None;
        self.view = Node::default();
        self.dialog = None;
        cx.emit(AddonPanelEvent::DialogChanged);
        self.citations.clear();
        self.sync_view(cx);
        if self.visible || self.starts_with_project() {
            self.start(cx);
        }
        cx.notify();
    }

    fn stop(&mut self) {
        self.task = None;
        self.process = None;
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
        process.send(HostMessage::Init { api: API_VERSION, project });
        self.process = Some(process);
        self.stopped = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this.update(cx, |this, cx| this.handle_event(event, cx)).is_err() {
                    return;
                }
            }
        }));
        cx.notify();
    }

    fn handle_event(&mut self, event: ProcessEvent, cx: &mut Context<Self>) {
        match event {
            ProcessEvent::Message(AddonMessage::SetView { view }) => {
                self.view = view;
                self.sync_view(cx);
                self.scroll_to_end();
            }
            ProcessEvent::Message(AddonMessage::Append { id, text }) => {
                append(&mut self.view, &id, &text);
                self.scroll_to_end();
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
                return;
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
                    self.sync_view(cx);
                    cx.emit(AddonPanelEvent::DialogChanged);
                }
            }
            ProcessEvent::Invalid(error) => self.invalid = Some(error),
            ProcessEvent::Exited(reason) => {
                if std::mem::take(&mut self.dialog_asked) {
                    cx.emit(AddonPanelEvent::LookupFailed(format!("{} stopped: {reason}", self.add_on.manifest.name)));
                }
                self.process = None;
                self.citations.clear();
                self.dialog = None;
                cx.emit(AddonPanelEvent::DialogChanged);
                self.stopped = Some(reason);
            }
        }
        cx.notify();
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
            .children(open.map(|open| self.render_dropdown_list(open, options, selected, cx)))
            .into_any_element()
    }

    /// The list opens upward from the button, since dropdowns sit low in a panel.
    /// The backdrop takes a click elsewhere, so that it only closes the list.
    fn render_dropdown_list(&self, open: &OpenDropdown, options: &[DropdownOption], selected: usize, cx: &mut Context<Self>) -> AnyElement {
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
        let list = anchored().anchor(Anchor::BottomLeft).snap_to_window_with_margin(px(8.)).child(
            div()
                .id("dropdown-list")
                .track_focus(&open.focus)
                .flex()
                .flex_col()
                .w(px(DROPDOWN_WIDTH))
                .mb(px(4.))
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
            Node::Column { children } => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .when(contains_scroll(node), |style| style.flex_1().min_h_0())
                .children(children.iter().map(|child| self.render_node(child, cx)).collect::<Vec<_>>())
                .into_any_element(),
            Node::Row { children } => div()
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
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::CODE_FONT_SIZE))
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
            Node::Input { id, placeholder, .. } => {
                let input = self.inputs.get(id);
                let empty = input.is_none_or(|input| input.read(cx).state.buffer().is_empty());
                div()
                    .relative()
                    .flex_none()
                    .h(px(INPUT_HEIGHT))
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
                                .font_family(theme::CODE_FONT)
                                .text_size(px(theme::CODE_FONT_SIZE))
                                .line_height(px(theme::CODE_LINE_HEIGHT))
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
        }
    }
}

fn append(node: &mut Node, target: &str, addition: &str) -> bool {
    match node {
        Node::Column { children } | Node::Row { children } | Node::Scroll { children, .. } => {
            children.iter_mut().any(|child| append(child, target, addition))
        }
        Node::Text { id: Some(id), text, .. } | Node::Code { id: Some(id), text } if id == target => {
            text.push_str(addition);
            true
        }
        _ => false,
    }
}

fn collect_ids(node: &Node, inputs: &mut Vec<String>, scrolls: &mut Vec<String>) {
    match node {
        Node::Input { id, .. } => inputs.push(id.clone()),
        Node::Scroll { id, children, .. } => {
            scrolls.push(id.clone());
            children.iter().for_each(|child| collect_ids(child, inputs, scrolls));
        }
        Node::Column { children } | Node::Row { children } => {
            children.iter().for_each(|child| collect_ids(child, inputs, scrolls));
        }
        _ => {}
    }
}

fn contains_dropdown(node: &Node, target: &str) -> bool {
    match node {
        Node::Dropdown { id, .. } => id == target,
        Node::Column { children } | Node::Row { children } | Node::Scroll { children, .. } => {
            children.iter().any(|child| contains_dropdown(child, target))
        }
        _ => false,
    }
}

fn input_value<'a>(node: &'a Node, target: &str) -> Option<&'a str> {
    match node {
        Node::Input { id, value, .. } if id == target => Some(value),
        Node::Column { children } | Node::Row { children } | Node::Scroll { children, .. } => {
            children.iter().find_map(|child| input_value(child, target))
        }
        _ => None,
    }
}

fn default_button(node: &Node) -> Option<(&str, Option<&str>)> {
    match node {
        Node::Button { id, primary: true, enabled: true, submits, .. } => Some((id, submits.as_deref())),
        Node::Column { children } | Node::Row { children } | Node::Scroll { children, .. } => {
            children.iter().find_map(default_button)
        }
        _ => None,
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
    match node {
        Node::Scroll { id, children, follow } => {
            if *follow {
                following.push(id);
            }
            children.iter().for_each(|child| collect_following(child, following));
        }
        Node::Column { children } | Node::Row { children } => {
            children.iter().for_each(|child| collect_following(child, following));
        }
        _ => {}
    }
}

fn contains_scroll(node: &Node) -> bool {
    match node {
        Node::Scroll { .. } => true,
        Node::Column { children } | Node::Row { children } => children.iter().any(contains_scroll),
        _ => false,
    }
}

fn button_submitting<'a>(node: &'a Node, input: &str) -> Option<&'a str> {
    match node {
        Node::Button { id, submits: Some(submits), enabled: true, .. } if submits == input => Some(id),
        Node::Column { children } | Node::Row { children } | Node::Scroll { children, .. } => {
            children.iter().find_map(|child| button_submitting(child, input))
        }
        _ => None,
    }
}

impl Render for AddonPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport = window.viewport_size();
        // The add-on can take the dropdown away while its list shows.
        if self.open_dropdown.as_ref().is_some_and(|open| !contains_dropdown(&self.view, &open.id)) {
            self.close_dropdown(window, cx);
        }
        let view = self.render_node(&self.view.clone(), cx);
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
            children: vec![Node::Row { children: vec![button("a", false, true), button("b", true, false), button("c", true, true)] }],
        };
        assert_eq!(default_button(&view), Some(("c", None)));
        assert_eq!(default_button(&button("a", false, true)), None);
    }

    #[test]
    fn append_finds_nested_text() {
        let mut view = Node::Column {
            children: vec![Node::Scroll { id: "s".to_string(), children: vec![text("a", "x"), text("b", "y")], follow: true }],
        };
        assert!(append(&mut view, "b", "z"));
        assert!(!append(&mut view, "missing", "z"));
        let Node::Column { children } = &view else { panic!() };
        let Node::Scroll { children, .. } = &children[0] else { panic!() };
        assert_eq!(children[1], text("b", "yz"));
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
        let view = Node::Row { children: vec![button("off", "a", false), button("other", "b", true), button("send", "a", true)] };
        assert_eq!(button_submitting(&view, "a"), Some("send"));
        assert_eq!(button_submitting(&view, "c"), None);
    }

    #[test]
    fn ids_are_collected_at_any_depth() {
        let view = Node::Column {
            children: vec![
                Node::Scroll {
                    id: "s".to_string(),
                    children: vec![Node::Input { id: "i".to_string(), placeholder: String::new(), value: String::new() }],
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
        let view = Node::Column { children: vec![scroll("chat", true), scroll("list", false)] };
        let mut following = Vec::new();
        collect_following(&view, &mut following);
        assert_eq!(following, ["chat"]);
    }
}
