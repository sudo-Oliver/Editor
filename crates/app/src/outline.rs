use std::collections::HashMap;

use editor_core::{Heading, HeadingKind, Language};
use gpui::{AnyElement, Div, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Stateful, Styled, div, prelude::FluentBuilder, px, svg};

use crate::theme;

// The outline reads like a contents page: one line down the left, chapters
// as numbered pills on it, sections as dots.

/// Where the line of the outline runs, from the left edge.
pub const SPINE: f32 = 22.0;
/// The width of the pill that numbers a chapter.
pub const PILL: f32 = 30.0;
pub const LINE: f32 = 18.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub level: usize,
    pub title: String,
    pub line: usize,
    /// Number of open parents, which differs from `level` when a level is skipped.
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
    /// The number LaTeX would give it, counted by depth, such as `[1, 2]`
    /// for 1.2. None for a starred heading, which LaTeX leaves unnumbered.
    /// Figures and tables count on their own, such as `[3]` for the third table.
    pub number: Option<Vec<usize>>,
    pub kind: HeadingKind,
}

/// Figures and tables hang below the heading they follow, deeper than any heading.
const FLOAT_LEVEL: usize = usize::MAX;

/// Turns parsed headings into a tree, stored flat in document order.
pub fn outline(headings: &[Heading], language: Language) -> Vec<OutlineItem> {
    let mut items: Vec<OutlineItem> = Vec::new();
    let mut parents: Vec<usize> = Vec::new();
    let mut counters: Vec<usize> = Vec::new();
    let (mut figures, mut tables) = (0, 0);
    // The references sit with the largest headings of the document.
    let top = headings.iter().filter(|heading| heading.kind == HeadingKind::Section).map(|heading| heading.level).min().unwrap_or(0);
    for heading in headings {
        // A caption of Typst comes with its brackets.
        let source = match heading.kind {
            HeadingKind::Section => heading.title.as_str(),
            _ => heading.title.trim().trim_start_matches('[').trim_end_matches(']'),
        };
        let mut title = match language {
            Language::Latex => clean_latex(source),
            _ => clean_markup(source, language),
        };
        let level = match heading.kind {
            HeadingKind::Section => heading.level,
            HeadingKind::Figure | HeadingKind::Table => FLOAT_LEVEL,
            HeadingKind::References => top,
        };
        if title.is_empty() {
            title = match heading.kind {
                // A heading stays out while its title is still being typed.
                HeadingKind::Section => continue,
                HeadingKind::Figure => "Figure".to_string(),
                HeadingKind::Table => "Table".to_string(),
                HeadingKind::References => "References".to_string(),
            };
        }
        while parents.last().is_some_and(|&parent| items[parent].level >= level) {
            parents.pop();
        }
        if let Some(&parent) = parents.last() {
            items[parent].has_children = true;
        }
        parents.push(items.len());
        let depth = parents.len() - 1;
        let number = match heading.kind {
            HeadingKind::Section => heading.numbered.then(|| {
                counters.resize(depth + 1, 0);
                counters[depth] += 1;
                counters.clone()
            }),
            HeadingKind::Figure => {
                figures += 1;
                Some(vec![figures])
            }
            HeadingKind::Table => {
                tables += 1;
                Some(vec![tables])
            }
            HeadingKind::References => None,
        };
        items.push(OutlineItem { level, title, line: heading.line, depth, has_children: false, collapsed: false, number, kind: heading.kind });
    }
    items
}

/// What the outline writes before a title: `01` or `1.2` for a heading, the
/// number alone for a figure or table, and a dash for what has no number.
pub fn marker(item: &OutlineItem) -> String {
    match (item.kind, item.number.as_deref()) {
        (HeadingKind::Section, Some(number)) => label(number),
        (HeadingKind::Figure | HeadingKind::Table, Some(number)) => number.iter().map(usize::to_string).collect(),
        _ => "—".to_string(),
    }
}

/// Where a row stands to the place being read: in the chain of headings that
/// leads there, or the place itself.
#[derive(Clone, Copy, Default)]
pub struct Place {
    pub in_trail: bool,
    pub here: bool,
}

/// One row of the outline as on a contents page: a chapter is a numbered pill on
/// a line across, a section a dot on the line down, with its number. `pill`
/// gets the chapter's pill, for a click on it; `trailing` goes at the end, such as
/// a page number. The caller adds what a click on the row does.
pub fn row(
    id: impl Into<ElementId>,
    item: &OutlineItem,
    place: Place,
    trailing: Option<AnyElement>,
    pill: impl FnOnce(Stateful<Div>) -> Stateful<Div>,
) -> Stateful<Div> {
    let palette = theme::palette();
    // Each row draws its piece of the line down, as only the rows on screen exist.
    let spine = div().absolute().top_0().bottom_0().left(px(SPINE - 0.5)).w(px(1.)).bg(theme::color(palette.divider));
    let row = div().id(id).relative().w_full().flex().child(spine);
    if item.depth == 0 {
        let label_color = if item.collapsed { palette.ui_text } else { palette.on_accent };
        let chapter = div()
            .id("outline-pill")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(PILL))
            .h(px(20.))
            .rounded_full()
            .bg(theme::color(palette.panel))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .map(|pill| {
                if item.collapsed {
                    pill.border_1().border_color(theme::color(palette.accent)).text_color(theme::color(palette.ui_text))
                } else {
                    let ink = theme::color(palette.accent).opacity(if place.in_trail { 1. } else { 0.6 });
                    pill.bg(ink).text_color(theme::color(palette.on_accent))
                }
            })
            .child(marker_element(item, theme::color(label_color)));
        return row
            .h(px(44.))
            .items_center()
            .pl(px(SPINE - PILL / 2.))
            .pr(px(12.))
            .child(div().absolute().left_0().right_0().top(px(22.)).h(px(1.)).bg(theme::color(palette.divider)))
            .child(pill(chapter))
            .child(
                div()
                    .relative()
                    .min_w_0()
                    .ml(px(6.))
                    .px(px(6.))
                    .bg(theme::color(palette.panel))
                    .font_weight(FontWeight::MEDIUM)
                    .truncate()
                    .child(item.title.clone()),
            )
            .child(div().flex_1())
            .children(trailing.map(|trailing| div().relative().pl(px(6.)).bg(theme::color(palette.panel)).child(trailing)));
    }
    // Sections hang on the line as dots, without indent: the number and the dot
    // tell the depth, and the title keeps the width.
    let dot_top = 6. + LINE / 2.;
    let dot = if place.here {
        div()
            .absolute()
            .left(px(SPINE - 4.5))
            .top(px(dot_top - 4.5))
            .size(px(9.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_1()
            .border_color(theme::color(palette.ui_text))
            .bg(theme::color(palette.panel))
            .child(div().size(px(3.)).rounded_full().bg(theme::color(palette.ui_text)))
    } else {
        div()
            .absolute()
            .left(px(SPINE - 2.5))
            .top(px(dot_top - 2.5))
            .size(px(5.))
            .rounded_full()
            .map(|dot| {
                if item.depth == 1 {
                    dot.bg(theme::color(palette.ui_text))
                } else {
                    dot.border_1().border_color(theme::color(palette.ui_text)).bg(theme::color(palette.panel))
                }
            })
    };
    row.items_start()
        .py(px(6.))
        .pl(px(SPINE + 12.))
        .pr(px(12.))
        .line_height(px(LINE))
        .child(dot)
        .child(
            div()
                .flex_none()
                .w(px(34.))
                .font_family(theme::code_font())
                .text_size(px(11.))
                .text_color(theme::color(palette.ui_muted))
                .child(marker_element(item, theme::color(palette.ui_muted))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .line_clamp(2)
                .text_ellipsis()
                .when(place.here, |title| title.font_weight(FontWeight::MEDIUM))
                .child(item.title.clone()),
        )
        .children(trailing.map(|trailing| div().flex_none().pl(px(8.)).child(trailing)))
}

/// The marker as drawn: the icon of a figure, table or the references before
/// its number, or the number alone.
pub fn marker_element(item: &OutlineItem, color: Hsla) -> AnyElement {
    match icon(item) {
        Some(path) => div()
            .flex()
            .items_center()
            .gap(px(3.))
            .child(svg().path(path).size(px(11.)).flex_none().text_color(color))
            .children((item.kind != HeadingKind::References).then(|| marker(item)))
            .into_any_element(),
        None => marker(item).into_any_element(),
    }
}

/// The icon beside a figure, a table or the references.
pub fn icon(item: &OutlineItem) -> Option<&'static str> {
    match item.kind {
        HeadingKind::Section => None,
        HeadingKind::Figure => Some("icons/image.svg"),
        HeadingKind::Table => Some("icons/table.svg"),
        HeadingKind::References => Some("icons/book.svg"),
    }
}

/// `01` for a chapter at the top, as on a contents page, and `1.2` below it.
pub fn label(number: &[usize]) -> String {
    match number {
        [chapter] => format!("{chapter:02}"),
        _ => number.iter().map(usize::to_string).collect::<Vec<_>>().join("."),
    }
}

/// Lists the items that no collapsed parent hides, with their index.
pub fn visible(items: &[OutlineItem]) -> impl Iterator<Item = (usize, &OutlineItem)> {
    let mut hidden_below = None;
    items.iter().enumerate().filter(move |(_, item)| {
        if hidden_below.is_some_and(|depth| item.depth > depth) {
            return false;
        }
        hidden_below = item.collapsed.then_some(item.depth);
        true
    })
}

/// Keeps headings collapsed across edits. Line numbers move, so a heading is known
/// by its level, title and how many identical headings come before it.
pub fn carry_collapsed(old: &[OutlineItem], new: &mut [OutlineItem]) {
    fn keys(items: &[OutlineItem]) -> impl Iterator<Item = (usize, &str, usize)> {
        let mut seen: HashMap<(usize, &str), usize> = HashMap::new();
        items.iter().map(move |item| {
            let count = seen.entry((item.level, &item.title)).or_default();
            *count += 1;
            (item.level, item.title.as_str(), *count)
        })
    }
    if !old.iter().any(|item| item.collapsed) {
        return;
    }
    let collapsed: std::collections::HashSet<_> =
        keys(old).zip(old).filter(|(_, item)| item.collapsed).map(|(key, _)| key).collect();
    let flags: Vec<bool> = keys(new).map(|key| collapsed.contains(&key)).collect();
    for (item, flag) in new.iter_mut().zip(flags) {
        item.collapsed = flag && item.has_children;
    }
}

/// Commands whose argument is not part of the visible title.
const LATEX_HIDDEN_ARGUMENT: &[&str] = &["label", "index", "footnote", "cite", "ref"];

/// Keeps the text of `\\emph{x}`, and the name of `\\LaTeX` or `\\alpha`.
fn clean_latex(title: &str) -> String {
    let mut result = String::new();
    let mut chars = title.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let mut name = String::new();
                while let Some(letter) = chars.next_if(char::is_ascii_alphabetic) {
                    name.push(letter);
                }
                if name.is_empty() {
                    match chars.next() {
                        Some('\\') | None => result.push(' '),
                        Some(escaped) => result.push(escaped),
                    }
                } else {
                    // `\\LaTeX{}` is a name, but `\\emph{x}` only wraps its argument.
                    let mut ahead = chars.clone();
                    let has_argument = ahead.next() == Some('{') && ahead.next() != Some('}');
                    if !has_argument {
                        result.push_str(&name);
                    } else if LATEX_HIDDEN_ARGUMENT.contains(&name.as_str()) {
                        skip_group(&mut chars);
                    }
                }
            }
            '{' | '}' | '$' => {}
            '%' => {
                chars.by_ref().find(|ch| *ch == '\n');
            }
            '~' => result.push(' '),
            other => result.push(other),
        }
    }
    join_lines(&result)
}

/// Skips one `{...}` group, which the iterator must be at the start of.
fn skip_group(chars: &mut std::iter::Peekable<std::str::Chars>) {
    let mut depth = 0;
    for ch in chars {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return;
        }
    }
}

/// A title can span lines in the source, but has one line in the outline.
fn join_lines(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Removes Typst and Markdown emphasis, labels and link targets. The text
/// inside `$…$` stays as written, because `_` and `*` mean something there.
fn clean_markup(title: &str, language: Language) -> String {
    let mut title = title.trim();
    if language == Language::Typst
        && let Some(position) = title.rfind(" <")
        && title.ends_with('>')
    {
        title = &title[..position];
    }
    let characters: Vec<char> = title.trim_end_matches([' ', '#']).chars().collect();
    let mut result = String::new();
    let mut in_math = false;
    let mut index = 0;
    while index < characters.len() {
        let ch = characters[index];
        let next = characters.get(index + 1);
        index += 1;
        match ch {
            '\\' if next.is_some() => {
                result.extend(next);
                index += 1;
            }
            '$' => in_math = !in_math,
            _ if in_math => result.push(ch),
            '*' | '`' => {}
            '_' => {
                let inside_word = result.chars().last().is_some_and(char::is_alphanumeric)
                    && next.is_some_and(|next| next.is_alphanumeric());
                if inside_word {
                    result.push('_');
                }
            }
            '[' if language == Language::Markdown => {}
            ']' if language == Language::Markdown && next == Some(&'(') => {
                while index < characters.len() && characters[index] != ')' {
                    index += 1;
                }
                index += 1;
            }
            _ => result.push(ch),
        }
    }
    join_lines(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(level: usize, title: &str, line: usize) -> Heading {
        Heading { level, title: title.into(), line, numbered: true, kind: HeadingKind::Section }
    }

    fn shape(items: &[OutlineItem]) -> Vec<(&str, usize, bool)> {
        items.iter().map(|item| (item.title.as_str(), item.depth, item.has_children)).collect()
    }

    fn titles<'a>(items: impl Iterator<Item = (usize, &'a OutlineItem)>) -> Vec<&'a str> {
        items.map(|(_, item)| item.title.as_str()).collect()
    }

    #[test]
    fn levels_become_a_tree() {
        let headings = [
            heading(2, "{A \\emph{b}}", 0),
            heading(4, "{Skips a level}", 1),
            heading(3, "{C}", 2),
            heading(2, "{D}", 3),
            heading(1, "{E}", 4),
        ];
        assert_eq!(
            shape(&outline(&headings, Language::Latex)),
            vec![("A b", 0, true), ("Skips a level", 1, false), ("C", 1, false), ("D", 0, false), ("E", 0, false)]
        );
    }

    #[test]
    fn headings_are_numbered_by_depth() {
        let mut starred = heading(2, "{Preface}", 1);
        starred.numbered = false;
        let headings = [
            heading(2, "{A}", 0),
            starred,
            heading(3, "{B}", 2),
            heading(4, "{Skips}", 3),
            heading(3, "{C}", 4),
            heading(2, "{D}", 5),
            heading(3, "{E}", 6),
        ];
        let labels: Vec<_> =
            outline(&headings, Language::Latex).iter().map(|item| item.number.as_deref().map_or("—".into(), label)).collect();
        assert_eq!(labels, ["01", "—", "1.1", "1.1.1", "1.2", "02", "2.1"]);
    }

    #[test]
    fn empty_titles_stay_out() {
        assert!(outline(&[heading(1, " ", 0)], Language::Typst).is_empty());
    }

    #[test]
    fn collapsing_hides_only_the_subtree() {
        let headings = [heading(1, "A", 0), heading(2, "B", 1), heading(3, "C", 2), heading(1, "D", 3)];
        let mut items = outline(&headings, Language::Typst);
        assert_eq!(titles(visible(&items)), ["A", "B", "C", "D"]);
        items[1].collapsed = true;
        assert_eq!(titles(visible(&items)), ["A", "B", "D"]);
        items[0].collapsed = true;
        assert_eq!(titles(visible(&items)), ["A", "D"]);
    }

    #[test]
    fn collapsed_headings_survive_edits() {
        let before = [heading(1, "Same", 0), heading(2, "x", 1), heading(1, "Same", 2), heading(2, "y", 3)];
        let mut old = outline(&before, Language::Typst);
        old[2].collapsed = true;
        let after = [heading(1, "New", 0), heading(1, "Same", 5), heading(2, "x", 6), heading(1, "Same", 9), heading(2, "y", 10)];
        let mut new = outline(&after, Language::Typst);
        carry_collapsed(&old, &mut new);
        assert_eq!(new.iter().map(|item| item.collapsed).collect::<Vec<_>>(), [false, false, false, true, false]);
    }

    #[test]
    fn titles_lose_their_markup() {
        assert_eq!(clean_markup("*1.* Nicht immer", Language::Typst), "1. Nicht immer");
        assert_eq!(clean_markup("Beispiel zu $N$ und $P$", Language::Typst), "Beispiel zu N und P");
        assert_eq!(clean_markup("_kurz_ $x_1 * y$ snake_case <sec:a>", Language::Typst), "kurz x_1 * y snake_case");
        assert_eq!(clean_markup("**Bold** [Pandoc](https://pandoc.org) `raw` ##", Language::Markdown), "Bold Pandoc raw");
        assert_eq!(clean_latex("The \\LaTeX{} \\textbf{way} of $\\alpha$~\\& more"), "The LaTeX way of alpha & more");
        assert_eq!(clean_latex("{Intro\\label{sec:a}\\footnote{a {b} c} \\& 100\\% % note\n  more}"), "Intro & 100% more");
    }

    #[test]
    fn figures_and_tables_hang_below_their_section_and_count_apart() {
        let float = |kind, title: &str| Heading { level: 0, title: title.into(), line: 0, numbered: false, kind };
        let headings = [
            heading(2, "{A}", 0),
            float(HeadingKind::Figure, "{A frog}"),
            float(HeadingKind::Table, ""),
            heading(3, "{B}", 3),
            float(HeadingKind::Figure, "{Pond}"),
            heading(2, "{C}", 5),
            float(HeadingKind::References, ""),
        ];
        let items = outline(&headings, Language::Latex);
        assert_eq!(
            shape(&items),
            [("A", 0, true), ("A frog", 1, false), ("Table", 1, false), ("B", 1, true), ("Pond", 2, false), ("C", 0, false), ("References", 0, false)]
        );
        let markers: Vec<String> = items.iter().map(marker).collect();
        assert_eq!(markers, ["01", "1", "1", "1.1", "2", "02", "—"]);
        // Typst captions come with their brackets.
        let typst = outline(&[float(HeadingKind::Figure, "[A *bold* frog]")], Language::Typst);
        assert_eq!(typst[0].title, "A bold frog");
    }
}
