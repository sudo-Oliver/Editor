//! Changes an add-on proposes, shown before they happen. The document stays as
//! it is while a read-only view shows the old lines struck red and the new ones
//! green, block by block. Accepting a block changes the document by just that
//! block, one undo step each; rejecting changes nothing. The add-on hears the
//! decisions once every block has one.

use std::{ops::Range, path::PathBuf};

use addon_api::{Reply, TextEdit};
use editor_core::{Decoration, DecorationKind, line_diff::line_diff};
use gpui::{AnyElement, Context, Entity, FontWeight, KeyDownEvent, div, prelude::*, px};

use super::Workspace;
use crate::{addon_panel::AddonPanel, addons, editor_view::EditorView, theme};

const INSERTED_COLOR: u32 = 0x2b8a3e;
const DELETED_COLOR: u32 = 0xe5484d;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Pending,
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hunk {
    /// Lines of the original text it replaces.
    old: Range<usize>,
    new: Vec<String>,
    decision: Decision,
}

/// The proposal and the decisions so far, without the editor around it.
#[derive(Clone, Debug)]
pub(crate) struct Proposal {
    lines: Vec<String>,
    pub(crate) hunks: Vec<Hunk>,
}

impl Proposal {
    pub(crate) fn new(original: &str, proposed: &str) -> Result<Self, String> {
        let diff = line_diff(original, proposed).ok_or("The change is too large to review.")?;
        let new_lines: Vec<&str> = proposed.split('\n').collect();
        let hunks = diff
            .into_iter()
            .map(|hunk| Hunk { old: hunk.old_lines, new: new_lines[hunk.lines].iter().map(|line| line.to_string()).collect(), decision: Decision::Pending })
            .collect();
        Ok(Self { lines: original.split('\n').map(str::to_string).collect(), hunks })
    }

    /// The text with the accepted blocks in place, or with `all` of them.
    pub(crate) fn text(&self, all: Option<Decision>) -> String {
        let mut lines: Vec<&str> = Vec::with_capacity(self.lines.len());
        let mut at = 0;
        for hunk in &self.hunks {
            lines.extend(self.lines[at..hunk.old.start].iter().map(String::as_str));
            if all.unwrap_or(hunk.decision) == Decision::Accepted {
                lines.extend(hunk.new.iter().map(String::as_str));
            } else {
                lines.extend(self.lines[hunk.old.clone()].iter().map(String::as_str));
            }
            at = hunk.old.end;
        }
        lines.extend(self.lines[at..].iter().map(String::as_str));
        lines.join("\n")
    }

    pub(crate) fn pending(&self) -> impl Iterator<Item = usize> + '_ {
        self.hunks.iter().enumerate().filter(|(_, hunk)| hunk.decision == Decision::Pending).map(|(index, _)| index)
    }

    pub(crate) fn decided(&self, decision: Decision) -> Vec<usize> {
        self.hunks.iter().enumerate().filter(|(_, hunk)| hunk.decision == decision).map(|(index, _)| index).collect()
    }

    /// What the view shows: the text with each open block twice, old and new,
    /// the marks for them, and the line where each open block starts.
    pub(crate) fn merged(&self) -> (String, Vec<Decoration>, Vec<(usize, usize)>) {
        let mut text = String::new();
        let mut decorations = Vec::new();
        let mut starts = Vec::new();
        let mut line = 0;
        let mut push = |text: &mut String, content: &str, kind: Option<DecorationKind>, line: &mut usize| {
            if *line > 0 {
                text.push('\n');
            }
            let start = text.len();
            text.push_str(content);
            if let Some(kind) = kind {
                let color = if kind == DecorationKind::Inserted { INSERTED_COLOR } else { DELETED_COLOR };
                decorations.push(Decoration { range: start..text.len(), kind, color, label: None });
            }
            *line += 1;
        };
        let mut at = 0;
        for (index, hunk) in self.hunks.iter().enumerate() {
            for content in &self.lines[at..hunk.old.start] {
                push(&mut text, content, None, &mut line);
            }
            match hunk.decision {
                Decision::Pending => {
                    starts.push((index, line));
                    for content in &self.lines[hunk.old.clone()] {
                        push(&mut text, content, Some(DecorationKind::Deleted), &mut line);
                    }
                    for content in &hunk.new {
                        push(&mut text, content, Some(DecorationKind::Inserted), &mut line);
                    }
                }
                Decision::Accepted => hunk.new.iter().for_each(|content| push(&mut text, content, None, &mut line)),
                Decision::Rejected => self.lines[hunk.old.clone()].iter().for_each(|content| push(&mut text, content, None, &mut line)),
            }
            at = hunk.old.end;
        }
        for content in &self.lines[at..] {
            push(&mut text, content, None, &mut line);
        }
        (text, decorations, starts)
    }
}

/// The new text from `edits`, each `old` occurring once in the text so far.
pub(crate) fn apply_edits(text: &str, edits: &[TextEdit]) -> Result<String, String> {
    let mut text = text.to_string();
    for edit in edits {
        let range = addons::find_unique(&text, &edit.old)?;
        text.replace_range(range, &edit.new);
    }
    Ok(text)
}

/// The smallest replacement that turns `old` into `new`.
pub(crate) fn smallest_change<'a>(old: &str, new: &'a str) -> (Range<usize>, &'a str) {
    let prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    let prefix = (0..=prefix).rev().find(|&at| old.is_char_boundary(at) && new.is_char_boundary(at)).unwrap_or(0);
    let longest = old.len().min(new.len()) - prefix;
    let suffix = old.bytes().rev().zip(new.bytes().rev()).take(longest).take_while(|(a, b)| a == b).count();
    let suffix = (0..=suffix).rev().find(|&length| old.is_char_boundary(old.len() - length) && new.is_char_boundary(new.len() - length)).unwrap_or(0);
    (prefix..old.len() - suffix, &new[prefix..new.len() - suffix])
}

pub(super) struct Review {
    pub(super) path: PathBuf,
    pub(super) panel: Entity<AddonPanel>,
    request: u64,
    title: String,
    proposal: Proposal,
    /// The text the document must have, so that outside changes end the review.
    expected: String,
    view: Entity<EditorView>,
    /// The open block in view.
    current: usize,
}

impl Workspace {
    pub(super) fn start_proposal(
        &mut self,
        panel: Entity<AddonPanel>,
        request: u64,
        relative: &str,
        title: String,
        edits: Vec<TextEdit>,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<Result<Reply, String>> {
        let path = match self.addon_file(relative) {
            Ok(path) => path,
            Err(error) => return Some(Err(error)),
        };
        if self.proposals.iter().any(|review| review.path == path) {
            return Some(Err(format!("A review of {relative} is open already.")));
        }
        let document = match self.document_at(&path, cx) {
            Some(index) => self.documents[index].view.clone(),
            None => match addons::read_file_limited(&path) {
                Ok(content) => {
                    self.add_document(path.clone(), &content, cx);
                    self.documents.last().map(|document| document.view.clone())?
                }
                Err(error) => return Some(Err(error)),
            },
        };
        let original = document.read(cx).state.buffer().to_string();
        let proposed = match text.map_or_else(|| apply_edits(&original, &edits), Ok) {
            Ok(proposed) => proposed,
            Err(error) => return Some(Err(error)),
        };
        let proposal = match Proposal::new(&original, &proposed) {
            Ok(proposal) => proposal,
            Err(error) => return Some(Err(error)),
        };
        if proposal.hunks.is_empty() {
            return Some(Ok(Reply::Reviewed { accepted: Vec::new(), rejected: Vec::new() }));
        }
        let view = cx.new(|cx| {
            let mut view = EditorView::new("", Some(path.clone()), cx);
            view.state.set_read_only(true);
            view
        });
        let title = if title.trim().is_empty() { format!("Changes from {}", panel.read(cx).add_on.manifest.name) } else { title };
        self.proposals.push(Review { path: path.clone(), panel, request, title, proposal, expected: original, view, current: 0 });
        self.refresh_proposal(&path, cx);
        // The review shows where the user looks.
        self.active = self.document_at(&path, cx);
        self.show_hub = false;
        self.focus_requested = true;
        cx.notify();
        None
    }

    fn proposal_index(&self, path: &PathBuf) -> Option<usize> {
        self.proposals.iter().position(|review| &review.path == path)
    }

    /// Draws the merged text again, at the open block.
    fn refresh_proposal(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        let Some(index) = self.proposal_index(path) else {
            return;
        };
        let review = &mut self.proposals[index];
        let (text, decorations, starts) = review.proposal.merged();
        let line = starts.iter().find(|(hunk, _)| *hunk >= review.current).or(starts.first()).map(|(hunk, line)| {
            review.current = *hunk;
            *line
        });
        review.view.update(cx, |view, cx| {
            view.state.set_read_only(false);
            let length = view.state.buffer().len();
            view.edit_from_outside(0..length, &text, cx);
            view.state.set_read_only(true);
            view.set_decorations(decorations, cx);
            if let Some(line) = line {
                view.place_cursor(line, 0, cx);
            }
        });
    }

    /// Accepts or rejects the open block, or every open one with `all`.
    pub(super) fn decide_proposal(&mut self, decision: Decision, all: bool, cx: &mut Context<Self>) {
        let Some(index) = self.active_proposal(cx) else {
            return;
        };
        let path = self.proposals[index].path.clone();
        let Some(document) = self.document_at(&path, cx).map(|at| self.documents[at].view.clone()) else {
            return self.end_proposal(index, Err("The document closed during the review.".to_string()), cx);
        };
        let review = &mut self.proposals[index];
        if document.read(cx).state.buffer().to_string() != review.expected {
            return self.end_proposal(index, Err("The document changed during the review, so the review ended.".to_string()), cx);
        }
        let targets: Vec<usize> = if all { review.proposal.pending().collect() } else { vec![review.current] };
        for target in &targets {
            if let Some(hunk) = review.proposal.hunks.get_mut(*target) {
                hunk.decision = decision;
            }
        }
        let after = review.proposal.text(None);
        if after != review.expected {
            let (range, text) = smallest_change(&review.expected, &after);
            let text = text.to_string();
            document.update(cx, |view, cx| view.edit_from_outside(range, &text, cx));
            review.expected = after;
        }
        if review.proposal.pending().next().is_none() {
            let reply = Reply::Reviewed { accepted: review.proposal.decided(Decision::Accepted), rejected: review.proposal.decided(Decision::Rejected) };
            return self.end_proposal(index, Ok(reply), cx);
        }
        self.refresh_proposal(&path, cx);
        cx.notify();
    }

    pub(super) fn step_proposal(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(index) = self.active_proposal(cx) else {
            return;
        };
        let review = &mut self.proposals[index];
        let pending: Vec<usize> = review.proposal.pending().collect();
        let position = pending.iter().position(|&hunk| hunk == review.current).unwrap_or(0);
        let next = if forward { (position + 1) % pending.len() } else { (position + pending.len() - 1) % pending.len() };
        review.current = pending[next];
        let path = review.path.clone();
        self.refresh_proposal(&path, cx);
        cx.notify();
    }

    pub(super) fn end_proposal(&mut self, index: usize, result: Result<Reply, String>, cx: &mut Context<Self>) {
        let review = self.proposals.remove(index);
        review.panel.update(cx, |panel, _| panel.reply(review.request, result));
        self.focus_requested = true;
        cx.notify();
    }

    pub(super) fn cancel_proposal(&mut self, relative: &str, cx: &mut Context<Self>) -> Result<Reply, String> {
        let path = self.addon_file(relative)?;
        if let Some(index) = self.proposal_index(&path) {
            self.end_proposal(index, Err("The review was cancelled.".to_string()), cx);
        }
        Ok(Reply::Done)
    }

    /// Ends the reviews of an add-on that stops or turns off.
    pub(super) fn end_proposals_of(&mut self, panel: &Entity<AddonPanel>, cx: &mut Context<Self>) {
        while let Some(index) = self.proposals.iter().position(|review| &review.panel == panel) {
            self.end_proposal(index, Err("The add-on stopped.".to_string()), cx);
        }
    }

    /// The review of the document in view.
    pub(super) fn active_proposal(&self, cx: &gpui::App) -> Option<usize> {
        if self.proposals.is_empty() {
            return None;
        }
        let active = self.active.and_then(|index| self.documents.get(index))?;
        let path = active.view.read(cx).path.clone()?;
        self.proposals.iter().position(|review| review.path == path)
    }

    pub(super) fn render_proposal(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let palette = theme::palette();
        let review = &self.proposals[index];
        let pending: Vec<usize> = review.proposal.pending().collect();
        let position = pending.iter().position(|&hunk| hunk == review.current).map_or(1, |position| position + 1);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .cursor_pointer()
                .font_weight(FontWeight::SEMIBOLD)
                .bg(theme::color(if primary { palette.accent } else { palette.divider }))
                .when(primary, |style| style.text_color(theme::color(palette.on_accent)))
                .hover(|style| style.opacity(0.85))
                .child(label)
        };
        let bar = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(4.))
            .px(px(10.))
            .py(px(6.))
            .bg(theme::color(palette.panel))
            .border_b_1()
            .border_color(theme::color(palette.divider))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(review.title.clone()))
                    .child(div().flex_none().text_color(theme::color(palette.ui_muted)).child(format!("Change {position} of {}", pending.len()))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(button("review-accept", "Accept  y", true).on_click(cx.listener(|this, _, _, cx| this.decide_proposal(Decision::Accepted, false, cx))))
                    .child(button("review-reject", "Reject  n", false).on_click(cx.listener(|this, _, _, cx| this.decide_proposal(Decision::Rejected, false, cx))))
                    .child(button("review-previous", "Previous  [", false).on_click(cx.listener(|this, _, _, cx| this.step_proposal(false, cx))))
                    .child(button("review-next", "Next  ]", false).on_click(cx.listener(|this, _, _, cx| this.step_proposal(true, cx))))
                    .child(div().flex_1())
                    .child(button("review-reject-all", "Reject all", false).on_click(cx.listener(|this, _, _, cx| this.decide_proposal(Decision::Rejected, true, cx))))
                    .child(button("review-accept-all", "Accept all", false).on_click(cx.listener(|this, _, _, cx| this.decide_proposal(Decision::Accepted, true, cx)))),
            );
        div()
            .flex()
            .flex_col()
            .size_full()
            // In the read-only view, single keys decide, as in a review elsewhere.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.alt {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "y" => this.decide_proposal(Decision::Accepted, false, cx),
                    "n" => this.decide_proposal(Decision::Rejected, false, cx),
                    "]" => this.step_proposal(true, cx),
                    "[" => this.step_proposal(false, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(bar)
            .child(div().flex_1().min_h_0().child(review.view.clone()))
            .into_any_element()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "\\section{A}\nfirst\nsecond\nthird\nfourth\n";

    #[test]
    fn decisions_shape_the_text() {
        let proposed = "\\section{A}\nFIRST\nsecond\nthird\nnew line\nfourth\n";
        let mut proposal = Proposal::new(OLD, proposed).unwrap();
        assert_eq!(proposal.hunks.len(), 2);
        assert_eq!(proposal.text(Some(Decision::Accepted)), proposed);
        assert_eq!(proposal.text(Some(Decision::Rejected)), OLD);
        proposal.hunks[1].decision = Decision::Accepted;
        proposal.hunks[0].decision = Decision::Rejected;
        assert_eq!(proposal.text(None), "\\section{A}\nfirst\nsecond\nthird\nnew line\nfourth\n");
        assert_eq!(proposal.decided(Decision::Accepted), [1]);
    }

    #[test]
    fn the_merged_view_shows_both_sides_of_open_blocks() {
        let proposal = Proposal::new(OLD, "\\section{A}\nFIRST\nsecond\nthird\nfourth\n").unwrap();
        let (text, decorations, starts) = proposal.merged();
        assert_eq!(text, "\\section{A}\nfirst\nFIRST\nsecond\nthird\nfourth\n");
        assert_eq!(starts, [(0, 1)]);
        assert_eq!(&text[decorations[0].range.clone()], "first");
        assert_eq!(decorations[0].kind, DecorationKind::Deleted);
        assert_eq!(&text[decorations[1].range.clone()], "FIRST");
        assert_eq!(decorations[1].kind, DecorationKind::Inserted);
    }

    #[test]
    fn additions_and_deletions_at_the_ends() {
        for proposed in ["", "added\n", "\\section{A}\nfirst\nsecond\nthird\nfourth\nlast", "start\n\\section{A}\nfirst\nsecond\nthird\nfourth\n", "first\nsecond\n"] {
            let proposal = Proposal::new(OLD, proposed).unwrap();
            assert_eq!(proposal.text(Some(Decision::Accepted)), proposed, "{proposed:?}");
            assert_eq!(proposal.text(Some(Decision::Rejected)), OLD);
        }
    }

    #[test]
    fn accepting_one_block_after_another_matches_accepting_all() {
        let proposed = "\\section{B}\nfirst\nsecond\nthird\nfourth\nfifth\n";
        let mut proposal = Proposal::new(OLD, proposed).unwrap();
        let mut document = OLD.to_string();
        for index in (0..proposal.hunks.len()).rev() {
            proposal.hunks[index].decision = Decision::Accepted;
            let after = proposal.text(None);
            let (range, text) = smallest_change(&document, &after);
            document.replace_range(range, text);
            assert_eq!(document, after);
        }
        assert_eq!(document, proposed);
    }

    #[test]
    fn edits_must_be_unique() {
        let edit = |old: &str, new: &str| TextEdit { old: old.to_string(), new: new.to_string() };
        assert_eq!(apply_edits("a b c", &[edit("b", "B"), edit("c", "C")]).unwrap(), "a B C");
        assert!(apply_edits("a a", &[edit("a", "b")]).is_err());
        assert!(apply_edits("a", &[edit("x", "y")]).is_err());
    }

    #[test]
    fn the_smallest_change_respects_characters() {
        assert_eq!(smallest_change("abcdef", "abXYef"), (2..4, "XY"));
        assert_eq!(smallest_change("äöü", "äxü"), (2..4, "x"));
        assert_eq!(smallest_change("same", "same"), (4..4, ""));
    }
}
