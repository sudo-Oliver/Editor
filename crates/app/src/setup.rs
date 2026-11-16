//! The setup on the first start. It asks what the user writes, installs the
//! compiler for it, and says at every step why, so that nobody installs
//! something they do not understand. People who know their way around skip it.

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, SharedString, Subscription, Window, div, prelude::*, px,
    svg,
};

use crate::{
    projects::TemplateLanguage,
    theme,
    toolchain::{self, Found, Tool, Toolchain},
};

const WIDTH: f32 = 640.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    Choose,
    Install,
    Done,
}

impl Step {
    const ALL: [Step; 4] = [Step::Welcome, Step::Choose, Step::Install, Step::Done];

    fn index(self) -> usize {
        Self::ALL.iter().position(|step| *step == self).unwrap_or(0)
    }
}

pub enum SetupEvent {
    /// `example` is the language of the welcome project to open, if any.
    /// `guided` is false when the user skipped.
    Finished { example: Option<TemplateLanguage>, guided: bool },
    /// Add the `editor` command for the terminal.
    InstallCommand,
}

pub struct SetupView {
    step: Step,
    latex: bool,
    typst: bool,
    toolchain: Entity<Toolchain>,
    focus: FocusHandle,
    _toolchain_observer: Subscription,
}

impl EventEmitter<SetupEvent> for SetupView {}

impl Focusable for SetupView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SetupView {
    pub fn new(toolchain: Entity<Toolchain>, cx: &mut Context<Self>) -> Self {
        let _toolchain_observer = cx.observe(&toolchain, |_, _, cx| cx.notify());
        // What is there already is the best guess of what the user writes.
        let (has_latex, has_typst) = (toolchain::find(Tool::Tectonic) != Found::Missing, toolchain::find(Tool::Typst) != Found::Missing);
        Self {
            step: Step::Welcome,
            latex: has_latex || !has_typst,
            typst: has_typst,
            toolchain,
            focus: cx.focus_handle(),
            _toolchain_observer,
        }
    }

    fn chosen(&self) -> Vec<Tool> {
        let mut tools = Vec::new();
        if self.latex {
            tools.push(Tool::Tectonic);
        }
        if self.typst {
            tools.push(Tool::Typst);
        }
        tools
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.step = step;
        cx.notify();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        let index = self.step.index();
        if index > 0 {
            self.go(Step::ALL[index - 1], cx);
        }
    }

    fn install_missing(&mut self, cx: &mut Context<Self>) {
        for tool in self.chosen() {
            if toolchain::find(tool) == Found::Missing && tool.installable() {
                self.toolchain.update(cx, |toolchain, cx| toolchain.start(tool, cx));
            }
        }
    }

    fn all_ready(&self) -> bool {
        self.chosen().into_iter().all(|tool| toolchain::find(tool) != Found::Missing)
    }

    fn finish(&mut self, example: bool, cx: &mut Context<Self>) {
        let language = if self.latex || !self.typst { TemplateLanguage::Latex } else { TemplateLanguage::Typst };
        cx.emit(SetupEvent::Finished { example: example.then_some(language), guided: true });
    }

    fn skip(&mut self, cx: &mut Context<Self>) {
        cx.emit(SetupEvent::Finished { example: None, guided: false });
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => match self.step {
                Step::Welcome => self.go(Step::Choose, cx),
                Step::Choose if self.latex || self.typst => self.go(Step::Install, cx),
                Step::Install => self.go(Step::Done, cx),
                Step::Done => self.finish(true, cx),
                _ => {}
            },
            "escape" if self.step == Step::Welcome => self.skip(cx),
            "escape" => self.back(cx),
            _ => {}
        }
    }

    // Rendering

    fn render_welcome(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = theme::palette();
        vec![
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(64.))
                .rounded(px(16.))
                .bg(theme::color(palette.accent))
                .child(svg().path("icons/logo.svg").size(px(38.)).text_color(theme::color(palette.on_accent)))
                .into_any_element(),
            title("Welcome to Editor"),
            paragraph("Write LaTeX or Typst on the left and see the PDF on the right, like on Overleaf. Everything runs on your Mac, also without internet."),
            paragraph("The setup takes about two minutes. It asks what you write and installs the one program that turns your text into a PDF."),
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .pt(px(8.))
                .child(primary("setup-start", "Set me up", cx, |this, cx| this.go(Step::Choose, cx)))
                .child(quiet("setup-skip", "Skip, I have my own setup", cx, |this, cx| this.skip(cx)))
                .into_any_element(),
        ]
    }

    fn choice(&self, id: &'static str, icon: &'static str, name: &'static str, text: &'static str, chosen: bool, cx: &mut Context<Self>, toggle: fn(&mut Self)) -> impl IntoElement {
        let palette = theme::palette();
        div()
            .id(id)
            .flex()
            .flex_col()
            .flex_1()
            .gap(px(8.))
            .p(px(16.))
            .rounded(px(theme::RADIUS_LG))
            .cursor_pointer()
            .bg(theme::color(if chosen { palette.accent_soft } else { palette.panel }))
            .border_2()
            .border_color(theme::color(if chosen { palette.accent } else { palette.divider }))
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(svg().path(format!("icons/{icon}.svg")).size(px(28.)).text_color(theme::color(palette.accent)))
                    .child(
                        svg()
                            .path("icons/check-circle.svg")
                            .size(px(20.))
                            .text_color(theme::color(if chosen { palette.accent } else { palette.divider })),
                    ),
            )
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(name))
            .child(div().text_color(theme::color(palette.ui_muted)).child(text))
    }

    fn render_choose(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let any = self.latex || self.typst;
        vec![
            title("What do you write?"),
            paragraph("Pick one or both. You can add the other one later in Writing tools."),
            div()
                .flex()
                .gap(px(12.))
                .child(self.choice(
                    "setup-latex",
                    "file-tex",
                    "LaTeX",
                    "What Overleaf uses, and what Claude writes when you ask for a paper, a thesis or slides.",
                    self.latex,
                    cx,
                    |this| this.latex = !this.latex,
                ))
                .child(self.choice(
                    "setup-typst",
                    "file-typ",
                    "Typst",
                    "A newer language for the same kind of documents, shorter to write and faster to compile.",
                    self.typst,
                    cx,
                    |this| this.typst = !this.typst,
                ))
                .into_any_element(),
            div()
                .text_color(theme::color(theme::palette().ui_muted))
                .child("Not sure? Choose LaTeX. Text that starts with \\documentclass is LaTeX.")
                .into_any_element(),
            self.nav(
                if any { Some(("Continue", |this: &mut Self, cx: &mut Context<Self>| this.go(Step::Install, cx))) } else { None },
                cx,
            ),
        ]
    }

    fn render_install(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let tools = self.chosen();
        let busy = tools.iter().any(|tool| self.toolchain.read(cx).installing(*tool));
        let missing = tools.iter().any(|tool| toolchain::find(*tool) == Found::Missing && tool.installable());
        let mut elements = vec![
            title("Install what you need"),
            paragraph(
                "Your text becomes a PDF through a program called a compiler. Overleaf runs it on its servers. Here it runs on your Mac, so it has to be installed once.",
            ),
        ];
        if let Some(choice) = toolchain::installer_choice(&self.toolchain, cx) {
            elements.push(div().text_color(theme::color(theme::palette().ui_muted)).child("You have Homebrew. Install with it, or keep the compilers in the app's own folder.").into_any_element());
            elements.push(choice);
        }
        elements.extend([
            div()
                .flex()
                .flex_col()
                .px(px(16.))
                .py(px(4.))
                .rounded(px(theme::RADIUS_LG))
                .bg(theme::color(theme::palette().panel))
                .border_1()
                .border_color(theme::color(theme::palette().divider))
                .children(toolchain::tool_rows(&self.toolchain, &tools, cx))
                .into_any_element(),
            div().flex().gap(px(8.)).items_start().child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::color(theme::palette().ui_muted))
                    .child(toolchain::promise(&self.toolchain, cx)),
            )
            .into_any_element(),
        ]);
        let next: Option<(&'static str, fn(&mut Self, &mut Context<Self>))> = if missing && !busy {
            Some(("Install", |this, cx| this.install_missing(cx)))
        } else if busy {
            None
        } else {
            Some(("Continue", |this, cx| this.go(Step::Done, cx)))
        };
        elements.push(self.nav(next, cx));
        if missing && !busy {
            elements.push(
                quiet("setup-later", "Not now, I will install it later", cx, |this, cx| this.go(Step::Done, cx)).into_any_element(),
            );
        }
        elements
    }

    fn render_done(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ready = self.all_ready();
        vec![
            title(if ready { "You are all set" } else { "Almost there" }),
            paragraph(if ready {
                "Open the welcome document to see how it works. A short tour shows each part of the editor, and you can skip it any time."
            } else {
                "A compiler is still missing, so documents cannot become PDFs yet. Install it any time from Writing tools in the project hub."
            }),
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .pt(px(8.))
                .child(primary("setup-example", "Open the welcome document", cx, |this, cx| this.finish(true, cx)))
                .child(quiet("setup-hub", "Go to my projects", cx, |this, cx| this.finish(false, cx)))
                .into_any_element(),
            // Homebrew is a fair sign of someone at home in the terminal.
            div()
                .when(toolchain::homebrew().is_some() && crate::shell_command::script().is_some(), |element| {
                    element.child(quiet("setup-command", "Also add the editor command for Terminal", cx, |_, cx| cx.emit(SetupEvent::InstallCommand)))
                })
                .into_any_element(),
        ]
    }

    fn nav(&self, next: Option<(&'static str, fn(&mut Self, &mut Context<Self>))>, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .pt(px(8.))
            .child(quiet("setup-back", "Back", cx, |this, cx| this.back(cx)))
            .children(next.map(|(label, handler)| primary("setup-next", label, cx, handler)))
            .into_any_element()
    }

    fn render_steps(&self) -> impl IntoElement {
        let palette = theme::palette();
        let current = self.step.index();
        div().flex().gap(px(6.)).children(Step::ALL.iter().enumerate().map(|(index, _)| {
            div()
                .h(px(4.))
                .w(px(if index == current { 28. } else { 12. }))
                .rounded(px(2.))
                .bg(theme::color(if index <= current { palette.accent } else { palette.divider }))
        }))
    }
}

impl Render for SetupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        let content = match self.step {
            Step::Welcome => self.render_welcome(cx),
            Step::Choose => self.render_choose(cx),
            Step::Install => self.render_install(cx),
            Step::Done => self.render_done(cx),
        };
        div()
            .id("setup")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .flex()
            .flex_col()
            .items_center()
            .size_full()
            .overflow_y_scroll()
            .bg(theme::color(theme::palette().bg))
            .text_color(theme::color(theme::palette().ui_text))
            .text_size(px(theme::UI_FONT_SIZE))
            .when_some(theme::look().ui_font, |root, font| root.font_family(font))
            .child(div().h(px(40.)).flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .w_full()
                    .max_w(px(WIDTH))
                    .px(px(24.))
                    .py(px(40.))
                    .when(self.step != Step::Welcome, |element| element.child(self.render_steps()))
                    .children(content),
            )
    }
}

fn title(text: &'static str) -> AnyElement {
    div().text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).child(text).into_any_element()
}

fn paragraph(text: impl Into<SharedString>) -> AnyElement {
    div().text_size(px(14.)).line_height(px(21.)).text_color(theme::color(theme::palette().ui_muted)).child(text.into()).into_any_element()
}

fn primary(id: &'static str, text: &'static str, cx: &mut Context<SetupView>, handler: fn(&mut SetupView, &mut Context<SetupView>)) -> impl IntoElement {
    let palette = theme::palette();
    div()
        .id(id)
        .flex_none()
        .px(px(18.))
        .py(px(9.))
        .rounded(px(theme::RADIUS_MD))
        .cursor_pointer()
        .bg(theme::color(palette.accent))
        .text_color(theme::color(palette.on_accent))
        .font_weight(FontWeight::SEMIBOLD)
        .hover(|style| style.opacity(0.9))
        .on_click(cx.listener(move |this, _, _, cx| handler(this, cx)))
        .child(text)
}

fn quiet(id: &'static str, text: &'static str, cx: &mut Context<SetupView>, handler: fn(&mut SetupView, &mut Context<SetupView>)) -> impl IntoElement {
    let palette = theme::palette();
    div()
        .id(id)
        .flex_none()
        .px(px(12.))
        .py(px(9.))
        .rounded(px(theme::RADIUS_MD))
        .cursor_pointer()
        .text_color(theme::color(palette.ui_muted))
        .hover(|style| style.bg(theme::color(palette.divider)).text_color(theme::color(palette.ui_text)))
        .on_click(cx.listener(move |this, _, _, cx| handler(this, cx)))
        .child(text)
}
