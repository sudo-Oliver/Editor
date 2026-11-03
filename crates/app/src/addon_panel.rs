//! Draws the panel of an add-on and passes messages between it and the workspace.
//! The program starts when the panel first shows, not before.

use std::{collections::HashMap, path::PathBuf};

use addon_api::{API_VERSION, AddonMessage, HostMessage, Node, Reply, Request};
use gpui::{
    AnyElement, App, Context, ElementId, Entity, EventEmitter, Focusable, FontWeight, Render, ScrollHandle, SharedString,
    Task, Window, div, prelude::*, px,
};

use crate::{
    actions::Recompile,
    addons::{AddonProcess, Installed, ProcessEvent},
    editor_view::EditorView,
    theme,
};

const INPUT_HEIGHT: f32 = 62.0;

pub enum AddonPanelEvent {
    Request { id: u64, request: Request },
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
    inputs: HashMap<String, Entity<EditorView>>,
    scrolls: HashMap<String, ScrollHandle>,
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
            inputs: HashMap::new(),
            scrolls: HashMap::new(),
        }
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
        self.sync_view(cx);
        if self.visible {
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
            ProcessEvent::Invalid(error) => self.invalid = Some(error),
            ProcessEvent::Exited(reason) => {
                self.process = None;
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
        for scroll in self.scrolls.values() {
            scroll.scroll_to_bottom();
        }
    }

    /// Keeps inputs and their text by id, so that a new view does not lose what
    /// the user is typing.
    fn sync_view(&mut self, cx: &mut Context<Self>) {
        let (mut inputs, mut scrolls) = (Vec::new(), Vec::new());
        collect_ids(&self.view, &mut inputs, &mut scrolls);
        self.inputs.retain(|id, _| inputs.contains(id));
        self.scrolls.retain(|id, _| scrolls.contains(id));
        for id in inputs {
            self.inputs.entry(id).or_insert_with(|| {
                let input = cx.new(|cx| {
                    let mut view = EditorView::new("", None, cx);
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
            Node::Scroll { id, children } => {
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
                .when(*muted, |style| style.text_color(theme::color(theme::UI_MUTED)))
                .child(text.clone())
                .into_any_element(),
            Node::Code { text, .. } => div()
                .flex_none()
                .p(px(6.))
                .rounded(px(4.))
                .bg(theme::color(theme::PANEL))
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
                    .bg(theme::color(if *primary && enabled { theme::GREEN } else { theme::DIVIDER }))
                    .when(enabled, |style| style.cursor_pointer())
                    .when(!enabled, |style| style.text_color(theme::color(theme::UI_MUTED)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if enabled {
                            this.click(&button, submits.as_deref(), cx);
                        }
                    }))
                    .child(label.clone())
                    .into_any_element()
            }
            Node::Input { id, placeholder } => {
                let input = self.inputs.get(id);
                let empty = input.is_none_or(|input| input.read(cx).state.buffer().is_empty());
                div()
                    .relative()
                    .flex_none()
                    .h(px(INPUT_HEIGHT))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::color(theme::DIVIDER))
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
                                .text_color(theme::color(theme::UI_MUTED))
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
                .bg(theme::color(theme::PANEL))
                .children(options.iter().enumerate().map(|(index, option)| {
                    let select = id.clone();
                    let chosen = index == *selected;
                    div()
                        .id(ElementId::NamedInteger(id.clone().into(), index as u64))
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(chosen, |style| style.bg(theme::color(theme::GREEN_DARK)).font_weight(FontWeight::SEMIBOLD))
                        .when(!chosen, |style| style.hover(|style| style.bg(theme::color(theme::DIVIDER))))
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.send(HostMessage::Select { id: select.clone(), selected: index });
                        }))
                        .child(option.clone())
                }))
                .into_any_element(),
            Node::Divider => div().flex_none().h(px(1.)).bg(theme::color(theme::PANEL)).into_any_element(),
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
        Node::Scroll { id, children } => {
            scrolls.push(id.clone());
            children.iter().for_each(|child| collect_ids(child, inputs, scrolls));
        }
        Node::Column { children } | Node::Row { children } => {
            children.iter().for_each(|child| collect_ids(child, inputs, scrolls));
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
        let _ = window;
        let view = self.render_node(&self.view.clone(), cx);
        let failure = self.stopped.clone().map(|reason| format!("{} stopped. {reason}", self.add_on.manifest.name));
        let invalid = self.invalid.clone().map(|error| format!("The add-on sent a broken message: {error}"));
        let restart = failure.is_some();
        let banner = |text: String| div().flex_none().p(px(8.)).rounded(px(4.)).bg(theme::color(theme::ERROR_PANEL)).text_size(px(12.)).child(text);
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
                        .bg(theme::color(theme::DIVIDER))
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
    fn append_finds_nested_text() {
        let mut view = Node::Column {
            children: vec![Node::Scroll { id: "s".to_string(), children: vec![text("a", "x"), text("b", "y")] }],
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
                Node::Scroll { id: "s".to_string(), children: vec![Node::Input { id: "i".to_string(), placeholder: String::new() }] },
            ],
        };
        let (mut inputs, mut scrolls) = (Vec::new(), Vec::new());
        collect_ids(&view, &mut inputs, &mut scrolls);
        assert_eq!((inputs, scrolls), (vec!["i".to_string()], vec!["s".to_string()]));
        assert!(contains_scroll(&view));
    }
}
