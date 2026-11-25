//! The setup on the first start. It asks what the user writes, installs
//! exactly that, tests it, and says at every step why and how much, so that
//! nobody installs something they do not understand. It needs nothing on the
//! Mac beforehand: Homebrew, when chosen, is installed first. People who know
//! their way around skip it, and anyone can open it again to add a language.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, SharedString, Subscription, Task, Window, div,
    prelude::*, px, svg,
};

use crate::{
    projects::TemplateLanguage,
    theme,
    toolchain::{self, Found, Tool, Toolchain},
};

const WIDTH: f32 = 640.0;
/// How often the manual way looks into Editor Tools.
const LOOK: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    Choose,
    Way,
    Install,
    Done,
}

impl Step {
    const ALL: [Step; 5] = [Step::Welcome, Step::Choose, Step::Way, Step::Install, Step::Done];

    fn index(self) -> usize {
        Self::ALL.iter().position(|step| *step == self).unwrap_or(0)
    }
}

/// How the programs come onto the Mac.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Way {
    Homebrew,
    AppFolder,
    /// The user downloads each file and puts it where the setup says.
    Manual,
}

#[derive(Clone, PartialEq, Eq)]
enum Probe {
    Running,
    Passed,
    Failed(String),
}

pub enum SetupEvent {
    /// `example` is the language of the welcome project to open, if any.
    /// `guided` is false when the user skipped.
    Finished { example: Option<TemplateLanguage>, guided: bool },
    /// Add the `editor` command for the terminal.
    InstallCommand,
    /// Something did not work: tell the developer.
    ReportProblem,
}

pub struct SetupView {
    step: Step,
    latex: bool,
    typst: bool,
    /// MacTeX instead of Tectonic.
    full_latex: bool,
    way: Way,
    /// Whether the other ways to install are open. People without Homebrew
    /// see only the app's folder unless they ask.
    show_ways: bool,
    /// What the last Install started, to show its rows while it runs.
    started: Vec<Tool>,
    probes: HashMap<Tool, Probe>,
    /// Free space in megabytes, once measured.
    free: Option<u64>,
    /// Whether Apple's developer tools are there, once known.
    apple_tools: Option<bool>,
    toolchain: Entity<Toolchain>,
    focus: FocusHandle,
    _looking: Option<Task<()>>,
    _toolchain_observer: Subscription,
}

impl EventEmitter<SetupEvent> for SetupView {}

impl Focusable for SetupView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SetupView {
    /// `again` starts at the choice, for someone who has been through the setup.
    pub fn new(toolchain: Entity<Toolchain>, again: bool, cx: &mut Context<Self>) -> Self {
        let _toolchain_observer = cx.observe(&toolchain, |this, _, cx| {
            // Once everything is there, the setup moves on by itself.
            if this.step == Step::Install && this.way != Way::Manual && this.all_installed(cx) {
                this.go(Step::Done, cx);
            }
            cx.notify();
        });
        // What is there already is the best guess of what the user writes.
        let (has_latex, has_typst) = { let state = toolchain.read(cx); (state.ready(Tool::Tectonic), state.ready(Tool::Typst)) };
        // Homebrew only for people who have it: everyone else would first get
        // Apple's developer tools, gigabytes they never asked for.
        // winget is part of Windows, so there it is the first choice whenever it is there.
        let way = {
            let state = toolchain.read(cx);
            let found = state.found(Tool::Homebrew) != Found::Missing;
            if found && (state.use_homebrew || cfg!(windows)) { Way::Homebrew } else { Way::AppFolder }
        };
        Self {
            step: if again { Step::Choose } else { Step::Welcome },
            latex: has_latex || !has_typst,
            typst: has_typst,
            full_latex: true,
            way,
            show_ways: false,
            started: Vec::new(),
            probes: HashMap::new(),
            free: None,
            apple_tools: None,
            toolchain,
            focus: cx.focus_handle(),
            _looking: None,
            _toolchain_observer,
        }
    }

    /// The compilers of the chosen languages.
    fn chosen(&self) -> Vec<Tool> {
        let mut tools = Vec::new();
        if self.latex {
            tools.push(if self.full_latex { Tool::MacTex } else { Tool::Tectonic });
        }
        if self.typst {
            tools.push(Tool::Typst);
        }
        tools
    }

    /// What has to be installed, smallest first, so that something works soon.
    /// MacTeX comes alone: one long download, and then everything works,
    /// rather than a second LaTeX that behaves differently in the meantime.
    fn to_install(&self, cx: &App) -> Vec<Tool> {
        let mut tools: Vec<Tool> = self.chosen().into_iter().filter(|tool| !self.has(*tool, cx)).collect();
        tools.sort_by_key(|tool| tool.installed_megabytes());
        tools
    }

    /// Whether the tool itself is there. For MacTeX that is a complete TeX,
    /// while `ready` already counts Tectonic as LaTeX.
    fn has(&self, tool: Tool, cx: &App) -> bool {
        let state = self.toolchain.read(cx);
        if tool == Tool::MacTex { state.has_complete_latex() } else { state.ready(tool) }
    }

    /// Whether every chosen language can make a PDF, maybe with the quick
    /// compiler while the complete one still downloads.
    fn usable(&self, cx: &App) -> bool {
        self.chosen().into_iter().all(|tool| self.ready(tool, cx))
    }

    /// Whether Homebrew itself comes first.
    fn needs_homebrew(&self, cx: &App) -> bool {
        self.way == Way::Homebrew && !self.ready(Tool::Homebrew, cx) && self.to_install(cx).iter().any(|tool| tool.formula().is_some())
    }

    fn ready(&self, tool: Tool, cx: &App) -> bool {
        self.toolchain.read(cx).ready(tool)
    }

    /// Download and disk together, in megabytes, since the downloads are kept until installed.
    fn megabytes_needed(&self, cx: &App) -> u64 {
        let mut tools = self.to_install(cx);
        let homebrew = self.needs_homebrew(cx);
        if homebrew {
            tools.push(Tool::Homebrew);
        }
        let apple = if homebrew && self.apple_tools == Some(false) { 2_000 } else { 0 };
        tools.iter().map(|tool| tool.megabytes().unwrap_or(0) + tool.installed_megabytes()).sum::<u64>() + apple
    }

    fn all_installed(&self, cx: &App) -> bool {
        let state = self.toolchain.read(cx);
        !state.busy() && self.chosen().into_iter().all(|tool| self.has(tool, cx))
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.step = step;
        self._looking = None;
        // The user may have installed something meanwhile.
        self.toolchain.read(cx).forget_found();
        match step {
            Step::Way => self.measure(cx),
            Step::Install if self.way == Way::Manual => self.look_for_downloads(cx),
            Step::Done => self.run_probes(cx),
            _ => {}
        }
        cx.notify();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        let index = self.step.index();
        if index > 0 {
            self.go(Step::ALL[index - 1], cx);
        }
    }

    /// Free space and Apple's tools, looked up off the main thread since
    /// asking for the tools starts a program.
    fn measure(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (free, apple) = cx.background_spawn(async { (free_megabytes(), toolchain::developer_tools()) }).await;
            this.update(cx, |this, cx| {
                this.free = free;
                this.apple_tools = Some(apple);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn choose_way(&mut self, way: Way, cx: &mut Context<Self>) {
        self.way = way;
        if way != Way::Manual {
            self.toolchain.update(cx, |toolchain, cx| toolchain.choose_homebrew(way == Way::Homebrew, cx));
        }
        cx.notify();
    }

    fn install_missing(&mut self, cx: &mut Context<Self>) {
        let tools = self.to_install(cx);
        let mut started = tools.clone();
        if self.needs_homebrew(cx) {
            started.insert(0, Tool::Homebrew);
        }
        self.started = started;
        // The way shown is the way used, also when it was only the default.
        let homebrew = self.way == Way::Homebrew;
        self.toolchain.update(cx, |toolchain, cx| {
            if toolchain.use_homebrew != homebrew {
                toolchain.choose_homebrew(homebrew, cx);
            }
            for tool in tools {
                toolchain.start(tool, cx);
            }
        });
        self.go(Step::Install, cx);
    }

    /// On the manual way, looks into Editor Tools until everything is there,
    /// and makes what it finds work.
    fn look_for_downloads(&mut self, cx: &mut Context<Self>) {
        if let Some(manual) = toolchain::manual_directory() {
            std::fs::create_dir_all(manual).ok();
        }
        self._looking = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(tools) = this.update(cx, |this, cx| {
                    this.toolchain.read(cx).forget_found();
                    this.to_install(cx)
                }) else {
                    return;
                };
                if tools.is_empty() {
                    this.update(cx, |this, cx| this.go(Step::Done, cx)).ok();
                    return;
                }
                let errors = cx
                    .background_spawn(async move { tools.into_iter().filter_map(|tool| toolchain::adopt_manual(tool).err().map(|error| (tool, error))).collect::<Vec<_>>() })
                    .await;
                let toolchain = this.update(cx, |this, _| this.toolchain.clone());
                if let Ok(toolchain) = toolchain {
                    toolchain.update(cx, |toolchain, cx| {
                        toolchain.errors.extend(errors);
                        cx.notify();
                    });
                }
                cx.background_executor().timer(LOOK).await;
            }
        }));
    }

    /// Compiles a test document in each chosen language.
    fn run_probes(&mut self, cx: &mut Context<Self>) {
        for tool in self.chosen() {
            if !self.ready(tool, cx) || self.probes.get(&tool) == Some(&Probe::Running) {
                continue;
            }
            self.probes.insert(tool, Probe::Running);
            let probe = cx.background_spawn(async move { toolchain::probe(tool) });
            cx.spawn(async move |this, cx| {
                let result = probe.await;
                this.update(cx, |this, cx| {
                    this.probes.insert(tool, match result {
                        Ok(()) => Probe::Passed,
                        Err(error) => Probe::Failed(error),
                    });
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn reinstall(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.probes.remove(&tool);
        if self.toolchain.read(cx).found(tool) == Found::Installed {
            self.toolchain.update(cx, |toolchain, cx| toolchain.remove(tool, cx));
        }
        self.toolchain.update(cx, |toolchain, cx| toolchain.start(tool, cx));
        self.started = vec![tool];
        self.go(Step::Install, cx);
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
                Step::Choose if self.latex || self.typst => self.after_choice(cx),
                Step::Done if !self.probes.values().any(|probe| *probe == Probe::Running) => self.finish(true, cx),
                _ => {}
            },
            "escape" if self.step == Step::Welcome => self.skip(cx),
            "escape" if self.step == Step::Install && self.toolchain.read(cx).busy() => {}
            "escape" => self.back(cx),
            _ => {}
        }
    }

    /// Nothing to install skips straight to the test.
    fn after_choice(&mut self, cx: &mut Context<Self>) {
        self.go(if self.to_install(cx).is_empty() { Step::Done } else { Step::Way }, cx);
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
            paragraph(if cfg!(windows) { "Write LaTeX or Typst on the left and see the PDF on the right, like on Overleaf. Everything runs on your PC, also without internet." } else { "Write LaTeX or Typst on the left and see the PDF on the right, like on Overleaf. Everything runs on your Mac, also without internet." }),
            paragraph(if cfg!(windows) { "The setup asks what you write and installs exactly what that needs, nothing else. You need nothing on your PC beforehand." } else { "The setup asks what you write and installs exactly what that needs, nothing else. You need nothing on your Mac beforehand." }),
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

    #[allow(clippy::too_many_arguments)]
    fn choice(&self, id: &'static str, icon: &'static str, name: &'static str, text: &'static str, chosen: bool, ready: bool, cx: &mut Context<Self>, toggle: fn(&mut Self)) -> impl IntoElement {
        let palette = theme::palette();
        div()
            .id(id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(8.))
            .p(px(16.))
            .rounded(px(theme::RADIUS_LG))
            .when(!ready, |element| element.cursor_pointer())
            .bg(theme::color(if chosen { palette.accent_soft } else { palette.panel }))
            .border_2()
            .border_color(theme::color(if chosen { palette.accent } else { palette.divider }))
            .on_click(cx.listener(move |this, _, _, cx| {
                // What is set up stays: removing happens in the settings.
                if !ready {
                    toggle(this);
                    cx.notify();
                }
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
            .when(ready, |element| element.child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::color(palette.accent)).child("Already set up")))
    }

    fn option(&self, id: &'static str, label: impl Into<SharedString>, text: impl Into<SharedString>, chosen: bool, cx: &mut Context<Self>, pick: impl Fn(&mut Self, &mut Context<Self>) + 'static) -> AnyElement {
        let palette = theme::palette();
        div()
            .id(id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(theme::RADIUS_MD))
            .cursor_pointer()
            .border_1()
            .border_color(theme::color(if chosen { palette.accent } else { palette.divider }))
            .when(chosen, |style| style.bg(theme::color(palette.accent_soft)))
            .on_click(cx.listener(move |this, _, _, cx| pick(this, cx)))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(label.into()))
            .child(div().text_size(px(11.)).text_color(theme::color(palette.ui_muted)).child(text.into()))
            .into_any_element()
    }

    fn render_choose(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let any = self.latex || self.typst;
        let (latex_ready, typst_ready) = (self.ready(Tool::Tectonic, cx), self.ready(Tool::Typst, cx));
        let mut elements = vec![
            title("What do you write?"),
            paragraph("Pick one or both. Only what you pick is installed. To add the other one later, open this setup again from the project hub."),
            div()
                .flex()
                .gap(px(12.))
                .child(self.choice(
                    "setup-latex",
                    "file-tex",
                    "LaTeX",
                    "What Overleaf uses, and what Claude writes when you ask for a paper, a thesis or slides.",
                    self.latex || latex_ready,
                    latex_ready,
                    cx,
                    |this| this.latex = !this.latex,
                ))
                .child(self.choice(
                    "setup-typst",
                    "file-typ",
                    "Typst",
                    "A newer language for the same kind of documents, shorter to write and faster to compile.",
                    self.typst || typst_ready,
                    typst_ready,
                    cx,
                    |this| this.typst = !this.typst,
                ))
                .into_any_element(),
        ];
        if self.latex && !latex_ready {
            let mactex = toolchain::size(Tool::MacTex.megabytes().unwrap_or(0));
            elements.push(div().pt(px(4.)).font_weight(FontWeight::SEMIBOLD).child("Which LaTeX?").into_any_element());
            elements.push(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(self.option(
                        "setup-mactex",
                        "Complete (recommended)",
                        format!("Every package at once. One {mactex} download, then everything works, also offline."),
                        self.full_latex,
                        cx,
                        |this, cx| {
                            this.full_latex = true;
                            cx.notify();
                        },
                    ))
                    .child(self.option(
                        "setup-tectonic",
                        "Small",
                        "About 20 MB, ready in a minute. Fetches each package from the internet the first time a document uses it.",
                        !self.full_latex,
                        cx,
                        |this, cx| {
                            this.full_latex = false;
                            cx.notify();
                        },
                    ))
                    .into_any_element(),
            );
        } else {
            elements.push(
                div()
                    .text_color(theme::color(theme::palette().ui_muted))
                    .child("Not sure? Choose LaTeX. Text that starts with \\documentclass is LaTeX.")
                    .into_any_element(),
            );
        }
        elements.push(self.nav(if any { Some(("Continue", |this: &mut Self, cx: &mut Context<Self>| this.after_choice(cx))) } else { None }, cx));
        elements
    }

    /// The ways to install, for people who have Homebrew or ask for them.
    fn render_ways(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_homebrew = self.ready(Tool::Homebrew, cx);
        let (homebrew_label, homebrew_text) = if cfg!(windows) {
            ("With winget (recommended)", "winget comes with Windows. It installs and later updates everything, for you alone and without a password.".to_string())
        } else if has_homebrew {
            ("With Homebrew (recommended)", "You have Homebrew. It installs and later updates everything.".to_string())
        } else {
            (
                "Install Homebrew first",
                format!(
                    "For people who use the Terminal. Adds Homebrew ({}) and Apple's developer tools (about 2 GB), then installs everything through it.",
                    toolchain::size(Tool::Homebrew.megabytes().unwrap_or(0))
                ),
            )
        };
        let app_label = if has_homebrew { "In the app's folder" } else { "In the app's folder (recommended)" };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            // On Windows winget is there or not: nothing to install first.
            .when(!cfg!(windows) || has_homebrew, |ways| ways.child(self.option("setup-way-brew", homebrew_label, homebrew_text, self.way == Way::Homebrew, cx, |this, cx| this.choose_way(Way::Homebrew, cx))))
            .child(self.option(
                "setup-way-app",
                app_label,
                "The app downloads the official programs into its own folder. Removed with one click.",
                self.way == Way::AppFolder,
                cx,
                |this, cx| this.choose_way(Way::AppFolder, cx),
            ))
            .child(self.option(
                "setup-way-manual",
                "I download the files myself",
                "The setup shows each file, its download link and exactly where to put it, and checks it.",
                self.way == Way::Manual,
                cx,
                |this, cx| this.choose_way(Way::Manual, cx),
            ))
            .into_any_element()
    }

    /// What will be installed, how big it is and what macOS will ask, before
    /// anything happens. People without Homebrew are not asked how: the app's
    /// folder needs nothing beforehand.
    fn render_way(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = theme::palette();
        let muted = theme::color(palette.ui_muted);
        let has_homebrew = self.ready(Tool::Homebrew, cx);
        let ways_open = has_homebrew || self.show_ways;
        let mut elements = vec![title("Ready to install"), paragraph(if cfg!(windows) { "Here is what your PC gets. Nothing happens until you click Install." } else { "Here is what your Mac gets. Nothing happens until you click Install." })];
        if ways_open {
            elements.push(self.render_ways(cx));
        }
        let mut parts: Vec<(String, String)> = Vec::new();
        let to_install = self.to_install(cx);
        let needs_homebrew = self.needs_homebrew(cx);
        if needs_homebrew && self.apple_tools == Some(false) {
            parts.push(("Apple's developer tools".to_string(), "about 2 GB, Apple's own dialog".to_string()));
        }
        for tool in needs_homebrew.then_some(Tool::Homebrew).into_iter().chain(to_install.iter().copied()) {
            let download = tool.megabytes().unwrap_or(0);
            parts.push((tool.name().to_string(), format!("{} download, {} on the disk", toolchain::size(download), toolchain::size(tool.installed_megabytes()))));
        }
        let needed = self.megabytes_needed(cx);
        let free = self.free;
        let enough = free.is_none_or(|free| free >= needed);
        // TeX Live installs for one user on Windows, without a password.
        let passwords = (self.way != Way::Manual && needs_homebrew) as usize + (to_install.contains(&Tool::MacTex) && cfg!(target_os = "macos")) as usize;
        elements.push(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(px(16.))
                .py(px(12.))
                .rounded(px(theme::RADIUS_LG))
                .bg(theme::color(palette.panel))
                .border_1()
                .border_color(theme::color(palette.divider))
                .child(div().font_weight(FontWeight::SEMIBOLD).pb(px(4.)).child("What will be installed"))
                .children(parts.into_iter().map(|(name, text)| div().flex().justify_between().gap(px(12.)).child(name).child(div().text_color(muted).child(text))))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .pt(px(6.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Space needed")
                        .child(match free {
                            Some(free) => format!("{} of {} free", toolchain::size(needed), toolchain::size(free)),
                            None => toolchain::size(needed),
                        }),
                )
                .into_any_element(),
        );
        if !enough {
            let missing = needed - free.unwrap_or(0);
            elements.push(
                div()
                    .text_color(theme::color(palette.error))
                    .child(format!("Your {} needs {} more free space for this. Free some up, or go back and choose the small LaTeX.", if cfg!(windows) { "PC" } else { "Mac" }, toolchain::size(missing)))
                    .into_any_element(),
            );
        }
        if to_install.contains(&Tool::MacTex) {
            elements.push(paragraph(if cfg!(windows) {
                "The complete LaTeX is one long download of about 2.6 GB: on a fast connection about an hour, on a slow one longer. The setup shows how long it still takes. After that, everything works offline."
            } else {
                "The complete LaTeX is one long download: on a fast connection about 15 minutes, on a slow one longer. The setup shows how long it still takes. After that, everything works offline, and updates come from inside the app."
            }));
        }
        if passwords > 0 && self.way != Way::Manual {
            elements.push(paragraph(if passwords == 1 {
                "macOS asks for your Mac password once, to install into the system. Editor never sees it."
            } else {
                "macOS asks for your Mac password twice: for Homebrew now and for MacTeX after its download. Editor never sees it."
            }));
        } else if self.way == Way::AppFolder {
            elements.push(paragraph(if cfg!(windows) {
                "No password needed. Everything stays in your own user folder, and nothing else on your PC changes."
            } else {
                "No password needed. Everything stays in the app's own folder, and nothing else on your Mac changes."
            }));
        }
        let next: Option<(&'static str, fn(&mut Self, &mut Context<Self>))> = match (enough, self.way) {
            (false, _) => None,
            (true, Way::Manual) => Some(("Show me the files", |this, cx| this.go(Step::Install, cx))),
            (true, _) => Some(("Install", |this, cx| this.install_missing(cx))),
        };
        elements.push(self.nav(next, cx));
        if !ways_open {
            elements.push(
                div()
                    .flex()
                    .justify_center()
                    .child(quiet("setup-ways", "Other ways to install", cx, |this, cx| {
                        this.show_ways = true;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        elements
    }

    fn render_install(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.way == Way::Manual {
            return self.render_manual(cx);
        }
        let mut tools = if self.started.is_empty() { self.chosen() } else { self.started.clone() };
        if !tools.contains(&Tool::Homebrew) && (self.toolchain.read(cx).installing(Tool::Homebrew) || self.toolchain.read(cx).errors.contains_key(&Tool::Homebrew)) {
            tools.insert(0, Tool::Homebrew);
        }
        let busy = self.toolchain.read(cx).busy();
        let failed = tools.iter().any(|tool| self.toolchain.read(cx).errors.contains_key(tool));
        let mut elements = vec![
            title(if busy { "Installing" } else { "Install what you need" }),
            paragraph(if cfg!(windows) { "Your text becomes a PDF through a program called a compiler. Overleaf runs it on its servers. Here it runs on your PC, so it is installed once." } else { "Your text becomes a PDF through a program called a compiler. Overleaf runs it on its servers. Here it runs on your Mac, so it is installed once." }),
            div()
                .flex()
                .flex_col()
                .px(px(16.))
                .py(px(4.))
                .rounded(px(theme::RADIUS_LG))
                .bg(theme::color(theme::palette().panel))
                .border_1()
                .border_color(theme::color(theme::palette().divider))
                .children(toolchain::tool_rows(&self.toolchain, &tools, false, cx))
                .into_any_element(),
        ];
        if failed && self.way == Way::Homebrew {
            elements.push(quiet("setup-no-brew", if cfg!(windows) { "Install without winget instead" } else { "Install without Homebrew instead" }, cx, |this, cx| {
                this.choose_way(Way::AppFolder, cx);
                this.toolchain.update(cx, |toolchain, _| {
                    toolchain.errors.clear();
                });
                this.install_missing(cx);
            })
            .into_any_element());
        }
        if busy && self.usable(cx) {
            // Only the complete LaTeX is left: the small one works already.
            elements.push(paragraph(if cfg!(windows) {
                "You can start now. The complete LaTeX keeps downloading and installing in the background, and the status bar shows how far."
            } else {
                "You can start now. The complete LaTeX keeps downloading in the background, and the status bar shows how far. macOS asks for your password once it is done."
            }));
            elements.push(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .pt(px(8.))
                    .child(primary("setup-start-now", "Start writing now", cx, |this, cx| this.go(Step::Done, cx)))
                    .into_any_element(),
            );
        } else if busy {
            // Waiting here beats a document without a PDF that nobody can explain.
            let left = self.toolchain.read(cx).jobs.values().filter_map(toolchain::Job::remaining).max();
            let once = if self.toolchain.read(cx).installing(Tool::MacTex) { " You wait only this once: afterwards LaTeX works offline." } else { "" };
            elements.push(paragraph(match left {
                Some(left) => format!("Please wait, {} left.{once} The setup then tests everything and tells you when you can start.", toolchain::spoken(left)),
                None => format!("Please wait until this is done.{once} The setup then tests everything and tells you when you can start."),
            }));
        } else {
            let left = self.to_install(cx);
            let next: (&'static str, fn(&mut Self, &mut Context<Self>)) = if left.is_empty() {
                ("Continue", |this, cx| this.go(Step::Done, cx))
            } else if failed {
                ("Try again", |this, cx| this.install_missing(cx))
            } else {
                ("Install", |this, cx| this.install_missing(cx))
            };
            elements.push(self.nav(Some(next), cx));
            if !left.is_empty() {
                elements.push(quiet("setup-later", "Not now, I will install it later", cx, |this, cx| this.go(Step::Done, cx)).into_any_element());
            }
            if failed {
                elements.push(quiet("setup-report-install", "Still not working? Report the problem", cx, |_, cx| cx.emit(SetupEvent::ReportProblem)).into_any_element());
            }
        }
        elements
    }

    /// The manual way: per program the file, its link, and where it goes.
    fn render_manual(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = theme::palette();
        let muted = theme::color(palette.ui_muted);
        let folder = toolchain::manual_directory().unwrap_or_else(|| PathBuf::from("~/Applications/Editor Tools"));
        let shown = crate::projects::tilde(&folder);
        let errors = self.toolchain.read(cx).errors.clone();
        let mut elements = vec![
            title("Download the files"),
            paragraph("For each part: download the file, then put it where it says. The setup notices on its own and checks every file."),
        ];
        for (index, tool) in self.chosen().into_iter().enumerate() {
            let ready = self.ready(tool, cx);
            let mut steps: Vec<AnyElement> = Vec::new();
            if tool == Tool::MacTex && cfg!(windows) {
                let page = tool.download_page();
                steps.push(numbered(1, "Download the TeX Live installer, install-tl-windows.exe, from tug.org.").into_any_element());
                steps.push(link(("manual-page", index), "Open the download page", move |cx| cx.open_url(page)).into_any_element());
                steps.push(numbered(2, format!("Open it from your Downloads folder, choose to install for yourself only and keep the other answers as they are. It fetches about {} and needs no password.", toolchain::size(tool.megabytes().unwrap_or(0)))).into_any_element());
            } else if tool == Tool::MacTex {
                let page = tool.download_page();
                steps.push(numbered(1, format!("Download MacTeX.pkg ({}) from tug.org.", toolchain::size(tool.megabytes().unwrap_or(0)))).into_any_element());
                steps.push(link(("manual-page", index), "Open the download page", move |cx| cx.open_url(page)).into_any_element());
                steps.push(numbered(2, "Open MacTeX.pkg in your Downloads folder and click through the installer. It asks for your Mac password.").into_any_element());
            } else if let (Some(file), Some(url)) = (toolchain::manual_file(tool), toolchain::manual_url(tool)) {
                steps.push(numbered(1, format!("Download {file} ({}).", toolchain::size(tool.megabytes().unwrap_or(0)))).into_any_element());
                steps.push(link(("manual-download", index), "Download the file", move |cx| cx.open_url(url)).into_any_element());
                steps.push(numbered(2, format!("Drag {file} from your Downloads folder into the folder {shown}. Packed or unpacked, both work.")).into_any_element());
                let (open, copy) = (folder.clone(), folder.clone());
                steps.push(
                    div()
                        .flex()
                        .gap(px(12.))
                        .child(link(("manual-folder", index), if cfg!(windows) { "Open the folder in Explorer" } else { "Open the folder in Finder" }, move |cx| {
                            std::fs::create_dir_all(&open).ok();
                            cx.open_with_system(&open);
                        }))
                        .child(link(("manual-copy", index), "Copy the path", move |cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.display().to_string()))))
                        .into_any_element(),
                );
            }
            let status: SharedString = if ready { "Found, ready".into() } else { "Waiting for the file…".into() };
            elements.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(16.))
                    .py(px(12.))
                    .rounded(px(theme::RADIUS_LG))
                    .bg(theme::color(palette.panel))
                    .border_1()
                    .border_color(theme::color(if ready { palette.accent } else { palette.divider }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(tool.name()))
                            .child(div().text_size(px(11.)).text_color(if ready { theme::color(palette.accent) } else { muted }).child(status)),
                    )
                    .when(!ready, |element| element.children(steps))
                    .when_some(errors.get(&tool).cloned(), |element, error| element.child(div().text_color(theme::color(palette.error)).child(error)))
                    .into_any_element(),
            );
        }
        elements.push(paragraph("Rather not? Go back and let the setup download them for you."));
        elements.push(self.nav(None, cx));
        elements
    }

    fn render_done(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = theme::palette();
        let chosen = self.chosen();
        let missing: Vec<Tool> = chosen.iter().copied().filter(|tool| !self.ready(*tool, cx)).collect();
        let testing = chosen.iter().any(|tool| self.probes.get(tool) == Some(&Probe::Running));
        let failed: Vec<(Tool, String)> = chosen
            .iter()
            .filter_map(|tool| match self.probes.get(tool) {
                Some(Probe::Failed(error)) => Some((*tool, error.clone())),
                _ => None,
            })
            .collect();
        let ready = missing.is_empty() && failed.is_empty() && !testing;
        let probe_failed = !failed.is_empty();
        let background = chosen.contains(&Tool::MacTex) && self.toolchain.read(cx).installing(Tool::MacTex);
        let mut elements = vec![
            title(if ready {
                "You are all set"
            } else if testing {
                "Testing your setup…"
            } else {
                "Almost there"
            }),
            paragraph(if ready && background {
                if cfg!(windows) { "A test document just became a PDF on your PC. The complete LaTeX keeps downloading in the background and takes over on its own once it is there. Open the welcome document to see how it works." } else { "A test document just became a PDF on your Mac. The complete LaTeX keeps downloading in the background and takes over on its own once it is there. Open the welcome document to see how it works." }
            } else if ready {
                if cfg!(windows) { "A test document just became a PDF on your PC. Open the welcome document to see how it works; a short tour shows each part of the editor." } else { "A test document just became a PDF on your Mac. Open the welcome document to see how it works; a short tour shows each part of the editor." }
            } else if testing && ((chosen.contains(&Tool::Tectonic) && !toolchain::complete_latex())) {
                "A small test document compiles with what was installed, so that your first PDF works. LaTeX fetches the packages it needs this first time, which takes two or three minutes."
            } else if testing {
                "A small test document compiles with what was installed, so that your first PDF works. The first time takes a moment."
            } else if !missing.is_empty() {
                "Something is still missing, so documents cannot become PDFs yet. Open this setup again from the project hub any time."
            } else {
                "The test document did not compile. Here is why and what helps."
            }),
        ];
        for tool in &chosen {
            let (text, color): (SharedString, _) = match self.probes.get(tool) {
                _ if missing.contains(tool) => ("Not installed".into(), palette.ui_muted),
                Some(Probe::Running) => ("Testing…".into(), palette.ui_muted),
                Some(Probe::Passed) => ("Works".into(), palette.accent),
                Some(Probe::Failed(_)) => ("Did not work".into(), palette.error),
                None => ("Ready".into(), palette.accent),
            };
            elements.push(div().flex().justify_between().child(tool.language()).child(div().text_color(theme::color(color)).child(text)).into_any_element());
        }
        for (index, (tool, error)) in failed.into_iter().enumerate() {
            let handle = cx.entity().downgrade();
            let retest = handle.clone();
            elements.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_color(theme::color(palette.error)).child(error))
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(link(("probe-again", index), "Test again", move |cx| {
                                retest.update(cx, |this, cx| {
                                    this.probes.remove(&tool);
                                    this.run_probes(cx);
                                })
                                .ok();
                            }))
                            .when(self.toolchain.read(cx).found(tool) == Found::Installed || tool.formula().is_some(), |element| {
                                element.child(link(("probe-reinstall", index), "Install it again", move |cx| {
                                    handle.update(cx, |this, cx| this.reinstall(tool, cx)).ok();
                                }))
                            }),
                    )
                    .into_any_element(),
            );
        }
        if probe_failed {
            elements.push(quiet("setup-report-test", "Still not working? Report the problem", cx, |_, cx| cx.emit(SetupEvent::ReportProblem)).into_any_element());
        }
        if !testing {
            elements.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pt(px(8.))
                    .child(primary("setup-example", "Open the welcome document", cx, |this, cx| this.finish(true, cx)))
                    .child(quiet("setup-hub", "Go to my projects", cx, |this, cx| this.finish(false, cx)))
                    .into_any_element(),
            );
        }
        // Homebrew is a fair sign of someone at home in the terminal.
        elements.push(
            div()
                .when(self.way == Way::Homebrew && self.ready(Tool::Homebrew, cx) && crate::shell_command::script().is_some(), |element| {
                    element.child(quiet("setup-command", "Also add the editor command for Terminal", cx, |_, cx| cx.emit(SetupEvent::InstallCommand)))
                })
                .into_any_element(),
        );
        elements
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
            Step::Way => self.render_way(cx),
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
            .child(
                crate::window_controls::drag_area(div().id("setup-band"))
                    .h(px(40.))
                    .w_full()
                    .flex_none()
                    .flex()
                    .justify_end()
                    .children(crate::window_controls::buttons(window, 40.)),
            )
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

/// Free space on the disk with the home folder, in megabytes.
fn free_megabytes() -> Option<u64> {
    crate::platform::free_megabytes(&crate::platform::home()?)
}

fn title(text: &'static str) -> AnyElement {
    div().text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).child(text).into_any_element()
}

fn paragraph(text: impl Into<SharedString>) -> AnyElement {
    div().text_size(px(14.)).line_height(px(21.)).text_color(theme::color(theme::palette().ui_muted)).child(text.into()).into_any_element()
}

fn numbered(number: usize, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .gap(px(8.))
        .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(format!("{number}.")))
        .child(div().flex_1().min_w_0().child(text.into()))
}

fn link(id: (&'static str, usize), text: &'static str, on_click: impl Fn(&mut gpui::App) + 'static) -> impl IntoElement {
    let palette = theme::palette();
    div()
        .id(id)
        .pl(px(20.))
        .cursor_pointer()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::color(palette.accent))
        .hover(|style| style.opacity(0.8))
        .on_click(move |_, _, cx| on_click(cx))
        .child(text)
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
