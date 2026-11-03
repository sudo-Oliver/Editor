use std::collections::HashMap;

use editor_core::{Heading, Language};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub level: usize,
    pub title: String,
    pub line: usize,
    /// Number of open parents, which differs from `level` when a level is skipped.
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
}

/// Turns parsed headings into a tree, stored flat in document order.
pub fn outline(headings: &[Heading], language: Language) -> Vec<OutlineItem> {
    let mut items: Vec<OutlineItem> = Vec::new();
    let mut parents: Vec<usize> = Vec::new();
    for heading in headings {
        let title = match language {
            Language::Latex => clean_latex(&heading.title),
            _ => clean_markup(&heading.title, language),
        };
        // A heading stays out while its title is still being typed.
        if title.is_empty() {
            continue;
        }
        while parents.last().is_some_and(|&parent| items[parent].level >= heading.level) {
            parents.pop();
        }
        if let Some(&parent) = parents.last() {
            items[parent].has_children = true;
        }
        parents.push(items.len());
        items.push(OutlineItem {
            level: heading.level,
            title,
            line: heading.line,
            depth: parents.len() - 1,
            has_children: false,
            collapsed: false,
        });
    }
    items
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
        Heading { level, title: title.into(), line }
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
}
