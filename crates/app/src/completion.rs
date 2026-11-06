//! What the suggestion popup offers at the cursor. Everything comes from fixed
//! lists and the parse trees of the open documents, so it works offline.

use std::collections::HashSet;

use editor_core::{Language, Symbol, SymbolKind, snippet::literal};

use crate::{picker::fuzzy_score, snippets};

const MAX_ITEMS: usize = 8;
/// Keeps names that start with the typed text above scattered matches.
const PREFIX_BONUS: i64 = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    /// `\` and a command name in LaTeX.
    Command,
    /// The name in `\begin{`.
    Environment,
    /// The key in `\ref{` or after `@` in Typst.
    Label,
    /// A name after `#` in Typst.
    Code,
}

/// A place that takes suggestions. `start` is where the replaced text begins in
/// the text before the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trigger {
    pub kind: TriggerKind,
    pub start: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    /// In the slot format of `editor_core::snippet`.
    pub template: String,
    pub detail: String,
}

/// Finds the trigger in `before`, the text of the line before the cursor. Plain
/// text has none, so that writing prose stays undisturbed.
pub fn trigger(language: Language, before: &str) -> Option<Trigger> {
    match language {
        Language::Latex => latex_trigger(before),
        Language::Typst => typst_trigger(before),
        Language::Markdown | Language::Plain => None,
    }
}

fn latex_trigger(before: &str) -> Option<Trigger> {
    if has_latex_comment(before) {
        return None;
    }
    let name = before.trim_end_matches(|ch: char| ch.is_ascii_alphabetic());
    if let Some(rest) = name.strip_suffix('\\') {
        let escaped = !(rest.len() - rest.trim_end_matches('\\').len()).is_multiple_of(2);
        return (!escaped).then_some(Trigger { kind: TriggerKind::Command, start: rest.len() });
    }
    let key = before.trim_end_matches(|ch: char| ch.is_alphanumeric() || matches!(ch, ':' | '_' | '-' | '.' | '*'));
    let command = key.strip_suffix('{')?;
    let kind = if command.ends_with("\\begin") {
        TriggerKind::Environment
    } else if ["\\ref", "\\eqref", "\\autoref", "\\cref", "\\Cref", "\\vref", "\\pageref", "\\nameref"]
        .iter()
        .any(|name| command.ends_with(name))
    {
        TriggerKind::Label
    } else {
        return None;
    };
    Some(Trigger { kind, start: key.len() })
}

fn has_latex_comment(before: &str) -> bool {
    before.char_indices().any(|(index, ch)| {
        let backslashes = before[..index].len() - before[..index].trim_end_matches('\\').len();
        ch == '%' && backslashes.is_multiple_of(2)
    })
}

fn typst_trigger(before: &str) -> Option<Trigger> {
    if before.contains("//") {
        return None;
    }
    let name = before.trim_end_matches(|ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '-'));
    let start = name.len();
    if let Some(rest) = name.strip_suffix('#') {
        return (!rest.ends_with('\\')).then_some(Trigger { kind: TriggerKind::Code, start });
    }
    let rest = name.strip_suffix('@')?;
    // `a@b` is a mail address, not a reference. A bare `@` opens the citation
    // picker instead, see `snippets::opens_reference`.
    let joined = rest.chars().next_back().is_some_and(char::is_alphanumeric);
    (!joined && start < before.len()).then_some(Trigger { kind: TriggerKind::Label, start })
}

/// The best suggestions for `prefix`, the text that accepting one replaces.
pub fn suggestions<'a>(
    language: Language,
    kind: TriggerKind,
    prefix: &str,
    symbols: impl Iterator<Item = &'a Symbol>,
) -> Vec<Entry> {
    let entries = candidates(language, kind, symbols);
    let mut seen = HashSet::new();
    let mut scored: Vec<(i64, Entry)> = entries
        .into_iter()
        .filter(|entry| seen.insert(entry.label.clone()))
        .filter_map(|entry| {
            let score = fuzzy_score(prefix, &entry.label)?;
            let bonus = if entry.label.starts_with(prefix) { PREFIX_BONUS } else { 0 };
            Some((score + bonus, entry))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let items: Vec<Entry> = scored.into_iter().take(MAX_ITEMS).map(|(_, entry)| entry).collect();
    // A finished word needs no popup that could only repeat it.
    if let [only] = items.as_slice()
        && only.label == prefix
    {
        return Vec::new();
    }
    items
}

fn candidates<'a>(language: Language, kind: TriggerKind, symbols: impl Iterator<Item = &'a Symbol>) -> Vec<Entry> {
    let wanted = match kind {
        TriggerKind::Command => Some(SymbolKind::Command),
        TriggerKind::Label => Some(SymbolKind::Label),
        TriggerKind::Code => Some(SymbolKind::Binding),
        TriggerKind::Environment => None,
    };
    let mut entries: Vec<Entry> = symbols
        .filter(|symbol| Some(symbol.kind) == wanted)
        .map(|symbol| Entry {
            label: symbol.name.clone(),
            template: literal(&symbol.name),
            detail: "project".to_string(),
        })
        .collect();
    match (language, kind) {
        (_, TriggerKind::Environment) => entries.extend(LATEX_ENVIRONMENTS.iter().map(|name| fixed(name, name))),
        (Language::Latex, TriggerKind::Command) => {
            entries.extend(LATEX_COMMANDS.iter().map(|template| fixed(command_name(template), template)));
            entries.extend(palette_commands());
        }
        (Language::Typst, TriggerKind::Code) => {
            entries.extend(TYPST_CODE.iter().map(|template| fixed(typst_name(template), template)));
        }
        _ => {}
    }
    entries
}

fn fixed(label: &str, template: &str) -> Entry {
    Entry { label: label.to_string(), template: template.to_string(), detail: String::new() }
}

/// The commands of the symbol palette, which shows each with its sign.
fn palette_commands() -> impl Iterator<Item = Entry> {
    snippets::GROUPS.iter().flat_map(|group| group.symbols).filter_map(|symbol| {
        let name = command_name(symbol.latex);
        (name.len() > 1 && name != "\\begin").then(|| Entry {
            label: name.to_string(),
            template: symbol.latex.to_string(),
            detail: symbol.label.to_string(),
        })
    })
}

/// The leading `\name` of a template.
fn command_name(template: &str) -> &str {
    let Some(rest) = template.strip_prefix('\\') else {
        return "";
    };
    let end = rest.find(|ch: char| !ch.is_ascii_alphabetic() && ch != '*').unwrap_or(rest.len());
    &template[..end + 1]
}

fn typst_name(template: &str) -> &str {
    let end = template.find(|ch: char| !ch.is_alphanumeric() && ch != '-').unwrap_or(template.len());
    &template[..end]
}

const LATEX_COMMANDS: &[&str] = &[
    "\\begin{${}}",
    "\\end{${}}",
    "\\documentclass{${}}",
    "\\usepackage{${}}",
    "\\title{${}}",
    "\\author{${}}",
    "\\date{${}}",
    "\\maketitle",
    "\\tableofcontents",
    "\\listoffigures",
    "\\listoftables",
    "\\part{${}}",
    "\\chapter{${}}",
    "\\section{${}}",
    "\\section*{${}}",
    "\\subsection{${}}",
    "\\subsection*{${}}",
    "\\subsubsection{${}}",
    "\\paragraph{${}}",
    "\\item",
    "\\label{${}}",
    "\\ref{${}}",
    "\\eqref{${}}",
    "\\pageref{${}}",
    "\\cite{${}}",
    "\\footnote{${}}",
    "\\caption{${}}",
    "\\centering",
    "\\includegraphics{${}}",
    "\\input{${}}",
    "\\include{${}}",
    "\\bibliography{${}}",
    "\\bibliographystyle{${}}",
    "\\printbibliography",
    "\\addbibresource{${}}",
    "\\newcommand{${}}{${}}",
    "\\renewcommand{${}}{${}}",
    "\\newenvironment{${}}{${}}{${}}",
    "\\newtheorem{${}}{${}}",
    "\\textbf{${}}",
    "\\textit{${}}",
    "\\texttt{${}}",
    "\\emph{${}}",
    "\\underline{${}}",
    "\\text{${}}",
    "\\textcolor{${}}{${}}",
    "\\href{${}}{${}}",
    "\\url{${}}",
    "\\hspace{${}}",
    "\\vspace{${}}",
    "\\newpage",
    "\\clearpage",
    "\\noindent",
    "\\hline",
    "\\mathbb{${}}",
    "\\mathbf{${}}",
    "\\mathcal{${}}",
    "\\mathrm{${}}",
    "\\operatorname{${}}",
    "\\binom{${}}{${}}",
    "\\overline{${}}",
    "\\tilde{${}}",
];

const LATEX_ENVIRONMENTS: &[&str] = &[
    "document",
    "abstract",
    "itemize",
    "enumerate",
    "description",
    "figure",
    "table",
    "tabular",
    "center",
    "quote",
    "verbatim",
    "equation",
    "equation*",
    "align",
    "align*",
    "gather",
    "gather*",
    "cases",
    "matrix",
    "pmatrix",
    "bmatrix",
    "theorem",
    "lemma",
    "proof",
    "definition",
    "minipage",
    "thebibliography",
];

const TYPST_CODE: &[&str] = &[
    "let ${} = ${}",
    "set ${}",
    "show ${}: ${}",
    "import \"${}\"",
    "include \"${}\"",
    "if ${} { ${} }",
    "for ${} in ${} { ${} }",
    "while ${} { ${} }",
    "context ${}",
    "align(${})[${}]",
    "bibliography(\"${}\")",
    "block[${}]",
    "box[${}]",
    "cite(<${}>)",
    "circle(${})",
    "columns(${})[${}]",
    "counter(${})",
    "datetime(${})",
    "emph[${}]",
    "enum(${})",
    "figure(${})",
    "footnote[${}]",
    "grid(${})",
    "h(${})",
    "heading[${}]",
    "image(\"${}\")",
    "label(\"${}\")",
    "line(${})",
    "link(\"${}\")[${}]",
    "list(${})",
    "lorem(${})",
    "outline()",
    "pad(${})[${}]",
    "page(${})[${}]",
    "pagebreak()",
    "par(${})[${}]",
    "place(${})[${}]",
    "query(${})",
    "raw(\"${}\")",
    "rect(${})",
    "ref(<${}>)",
    "stack(${})",
    "state(${})",
    "strong[${}]",
    "table(${})",
    "text(${})[${}]",
    "underline[${}]",
    "v(${})",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn at(language: Language, before: &str) -> Option<(TriggerKind, &str)> {
        trigger(language, before).map(|trigger| (trigger.kind, &before[trigger.start..]))
    }

    fn labels(items: &[Entry]) -> Vec<&str> {
        items.iter().map(|entry| entry.label.as_str()).collect()
    }

    #[test]
    fn latex_triggers_on_commands_environments_and_references() {
        assert_eq!(at(Language::Latex, "a \\fr"), Some((TriggerKind::Command, "\\fr")));
        assert_eq!(at(Language::Latex, "\\"), Some((TriggerKind::Command, "\\")));
        assert_eq!(at(Language::Latex, "\\begin{ite"), Some((TriggerKind::Environment, "ite")));
        assert_eq!(at(Language::Latex, "see \\ref{fig:a"), Some((TriggerKind::Label, "fig:a")));
        assert_eq!(at(Language::Latex, "\\eqref{"), Some((TriggerKind::Label, "")));
    }

    #[test]
    fn prose_gets_no_popup() {
        for before in ["", "some words", "ab", "x{y", "\\textbf{word", "line\\\\", "line\\\\next", "50\\% \\fr ", "% \\fr", "a \\% b % \\fr"] {
            assert_eq!(at(Language::Latex, before), None, "{before}");
        }
        for before in ["some words", "ab", "mail@example", "// #le", "\\#le"] {
            assert_eq!(at(Language::Typst, before), None, "{before}");
        }
        assert_eq!(at(Language::Markdown, "\\fr"), None);
        assert_eq!(at(Language::Plain, "\\fr"), None);
    }

    #[test]
    fn the_citation_picker_keeps_its_place() {
        assert_eq!(at(Language::Latex, "\\cite{"), None);
        assert_eq!(at(Language::Latex, "\\cite{kn"), None);
        assert_eq!(at(Language::Latex, "\\autocite[p. 3]{kn"), None);
    }

    #[test]
    fn an_escaped_percent_is_no_comment() {
        assert_eq!(at(Language::Latex, "50\\% \\fr"), Some((TriggerKind::Command, "\\fr")));
    }

    #[test]
    fn typst_triggers_on_code_and_references() {
        assert_eq!(at(Language::Typst, "#fig"), Some((TriggerKind::Code, "fig")));
        assert_eq!(at(Language::Typst, "#"), Some((TriggerKind::Code, "")));
        assert_eq!(at(Language::Typst, "see @fig-a"), Some((TriggerKind::Label, "fig-a")));
        assert_eq!(at(Language::Typst, "see @"), None);
    }

    #[test]
    fn commands_come_from_the_list_and_the_palette() {
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\fr", std::iter::empty());
        assert_eq!(items[0].label, "\\frac");
        assert_eq!(items[0].template, "\\frac{${}}{${}}");
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\alph", std::iter::empty());
        assert_eq!(items[0].detail, "α");
    }

    #[test]
    fn project_symbols_join_by_kind() {
        let symbols = [
            Symbol { kind: SymbolKind::Label, name: "fig:a".to_string() },
            Symbol { kind: SymbolKind::Command, name: "\\myfoo".to_string() },
        ];
        let items = suggestions(Language::Latex, TriggerKind::Label, "fi", symbols.iter());
        assert_eq!(labels(&items), ["fig:a"]);
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\myf", symbols.iter());
        assert_eq!(items[0].label, "\\myfoo");
    }

    #[test]
    fn symbols_with_dollars_stay_literal() {
        let symbols = [Symbol { kind: SymbolKind::Label, name: "a$b".to_string() }];
        let items = suggestions(Language::Latex, TriggerKind::Label, "a", symbols.iter());
        assert_eq!(items[0].template, "a$$b");
    }

    #[test]
    fn a_finished_word_closes_the_popup() {
        let symbols = [Symbol { kind: SymbolKind::Label, name: "fig:a".to_string() }];
        assert!(suggestions(Language::Latex, TriggerKind::Label, "fig:a", symbols.iter()).is_empty());
    }

    #[test]
    fn the_popup_stays_short_and_has_no_doubles() {
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\", std::iter::empty());
        assert_eq!(items.len(), MAX_ITEMS);
        let symbols = [Symbol { kind: SymbolKind::Command, name: "\\frac".to_string() }];
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\fra", symbols.iter());
        assert_eq!(items.iter().filter(|entry| entry.label == "\\frac").count(), 1);
    }

    #[test]
    fn environments_and_typst_names_match() {
        let items = suggestions(Language::Latex, TriggerKind::Environment, "ite", std::iter::empty());
        assert_eq!(items[0].label, "itemize");
        let items = suggestions(Language::Typst, TriggerKind::Code, "fig", std::iter::empty());
        assert_eq!(items[0].label, "figure");
        assert_eq!(items[0].template, "figure(${})");
    }
}
