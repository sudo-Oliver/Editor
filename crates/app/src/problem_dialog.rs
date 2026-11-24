//! The dialog that turns a problem, in the user's own words, into a GitHub
//! issue with the technical details added, see `problem_report`.

use editor_core::Language;
use gpui::{AnyElement, ClipboardItem, Context, Focusable, SharedString, Window, div, prelude::*, px};

use super::{Dialog, PreviewState, Workspace};
use crate::{
    problem_report::{self, Details, Report},
    theme,
    toolchain::{self, Found, Tool},
};

/// Status messages a report keeps.
pub(super) const KEPT_MESSAGES: usize = 12;

impl Workspace {
    /// Opens the dialog, with `crash` from the last run attached if there is one.
    pub(super) fn open_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Read once here: the dialog draws on every key, and this looks at the disk.
        self.report_snapshot = Some(self.report_details(cx));
        self.open_dialog(Dialog::ReportProblem, window, cx);
        window.focus(&self.report_field.read(cx).focus_handle(cx), cx);
    }

    /// What the app knows that helps to find the cause.
    fn report_details(&self, cx: &Context<Self>) -> Details {
        let shown = |found: Found| match found {
            Found::Installed => "installed by the editor".to_string(),
            Found::Own(path) => format!("found at {}", problem_report::shown_path(&path)),
            Found::Missing => "not installed".to_string(),
        };
        let tools = vec![
            ("LaTeX".to_string(), shown(toolchain::latex())),
            ("Typst".to_string(), shown(toolchain::find(Tool::Typst))),
            ("Pandoc".to_string(), shown(toolchain::find(Tool::Pandoc))),
            ("Homebrew".to_string(), shown(toolchain::find(Tool::Homebrew))),
        ];
        let install_errors = self.toolchain.read(cx).errors.iter().map(|(tool, error)| (tool.name().to_string(), error.clone())).collect();
        let document = self.active_view().map(|view| {
            let view = view.read(cx);
            let kind = match view.state.language() {
                Language::Latex => "LaTeX",
                Language::Typst => "Typst",
                Language::Markdown => "Markdown",
                Language::Plain => "plain text",
            };
            let lines = view.state.buffer().line_count();
            format!("{kind}, {lines} lines")
        });
        let build = self.engine.map(|engine| {
            let outcome = match (self.building, self.last_build_succeeded, self.preview_state) {
                (true, _, _) => "running",
                (false, true, _) => "succeeded",
                (false, false, PreviewState::Empty) => "failed, no PDF",
                (false, false, _) => "failed",
            };
            let errors = self.error_count();
            format!("{} with {errors} error{}, {}", engine.label(), if errors == 1 { "" } else { "s" }, outcome)
        });
        let build_errors = self
            .problems
            .iter()
            .take(5)
            .map(|problem| match &problem.location {
                Some(location) => format!("{} ({location})", problem.title),
                None => problem.title.clone(),
            })
            .collect();
        Details {
            tools,
            install_errors,
            document,
            build,
            build_errors,
            build_log: self.build_log.clone(),
            messages: self.recent_messages.iter().cloned().collect(),
            collaboration: None,
            crash: self.crash_report.clone(),
        }
    }

    fn report(&self, cx: &Context<Self>) -> Report {
        let description = self.report_field.read(cx).state.buffer().to_string();
        let details = self.report_with_details.then(|| self.report_snapshot.clone().unwrap_or_else(|| self.report_details(cx)));
        Report::new(&description, details.as_ref())
    }

    /// Puts the whole report on the clipboard and opens the issue on GitHub.
    fn send_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.report_field.read(cx).state.buffer().to_string().trim().is_empty() && self.crash_report.is_none() {
            self.report_error = Some("Write a sentence or two about what happened first.".into());
            cx.notify();
            return;
        }
        let report = self.report(cx);
        cx.write_to_clipboard(ClipboardItem::new_string(report.body.clone()));
        cx.open_url(&report.url());
        self.crash_report = None;
        self.report_error = None;
        self.report_field.update(cx, |field, cx| field.clear(cx));
        self.close_dialog(window, cx);
        self.set_status("Your report is open on GitHub. Press Submit new issue there to send it.", cx);
    }

    pub(super) fn render_report_dialog(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = theme::palette();
        let muted = |text: &str| div().flex_none().text_color(theme::color(palette.ui_muted)).child(SharedString::from(text.to_string())).into_any_element();
        let mut content = Vec::new();
        if self.crash_report.is_some() {
            content.push(
                div()
                    .flex_none()
                    .p(px(8.))
                    .rounded(px(4.))
                    .bg(theme::color(palette.error_panel))
                    .child("The editor quit unexpectedly last time. Sending this report helps to fix it, even with nothing written below.")
                    .into_any_element(),
            );
        }
        content.push(muted("Tell in your own words what happened and what you wanted to do. The editor adds the technical details itself, so you need not know them."));
        content.push(
            div()
                .flex_none()
                .h(px(120.))
                .p(px(6.))
                .rounded(px(4.))
                .overflow_hidden()
                .bg(theme::color(palette.bg))
                .border_1()
                .border_color(theme::color(palette.divider))
                .child(self.report_field.clone())
                .into_any_element(),
        );
        content.push(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .child(self.toggle_switch("report-details", self.report_with_details, true, cx, |this, _, cx| {
                    this.report_with_details = !this.report_with_details;
                    cx.notify();
                }))
                .child(div().flex_1().min_w_0().child("Add technical details: version, writing tools and the last compile, whose log can quote a few lines of your document. No user name. Show what is sent to check it."))
                .into_any_element(),
        );
        if self.report_with_details {
            let shown = self.report_preview;
            content.push(
                div()
                    .id("report-preview-toggle")
                    .flex_none()
                    .cursor_pointer()
                    .text_color(theme::color(palette.accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.report_preview = !this.report_preview;
                        cx.notify();
                    }))
                    .child(if shown { "Hide what is sent" } else { "Show what is sent" })
                    .into_any_element(),
            );
            if shown {
                content.push(
                    div()
                        .id("report-preview")
                        .flex_none()
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .p(px(6.))
                        .rounded(px(4.))
                        .bg(theme::color(palette.bg))
                        .font_family(theme::code_font())
                        .text_size(px(11.))
                        .child(self.report(cx).body)
                        .into_any_element(),
                );
            }
        }
        content.push(muted(
            "GitHub opens in your browser with the report filled in. Without a GitHub account, create one there for free first; it takes a minute. Then press Submit new issue. If the form is empty after signing up, click into it and press Cmd+V: the report is also on your clipboard.",
        ));
        content.push(
            div()
                .flex()
                .flex_none()
                .pt(px(4.))
                .child(self.panel_button("report-send", "Continue on GitHub…", true, cx, |this, window, cx| this.send_report(window, cx)))
                .into_any_element(),
        );
        if let Some(error) = &self.report_error {
            content.push(div().flex_none().text_color(theme::color(palette.error)).child(error.clone()).into_any_element());
        }
        content
    }
}
