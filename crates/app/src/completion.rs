//! What the suggestion popup offers at the cursor. Everything comes from fixed
//! lists and the parse trees of the open documents, so it works offline.

use std::collections::HashMap;

use build::Loaded;
use editor_core::{Language, Symbol, SymbolKind, snippet::literal};

use crate::{package_commands::PackageCommand, picker::fuzzy_score, snippets};

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
    /// The prefix of the labels this command usually takes, such as `eq:` for `\eqref`.
    pub prefer: Option<&'static str>,
}

/// What the citation index knows about labels: their kind, number and
/// caption, and which ones the command at the cursor usually takes.
#[derive(Clone, Copy, Debug, Default)]
pub struct LabelHints<'a> {
    pub details: Option<&'a HashMap<String, String>>,
    pub prefer: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    /// In the slot format of `editor_core::snippet`.
    pub template: String,
    pub detail: String,
}

/// What the project brings beyond the fixed lists: the definitions of the
/// packages it loads, and the commands it already uses.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    pub commands: Vec<PackageCommand>,
    /// Names with their package.
    pub environments: Vec<(String, String)>,
    pub used: Vec<String>,
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
        return (!escaped).then_some(Trigger { kind: TriggerKind::Command, start: rest.len(), prefer: None });
    }
    let key = before.trim_end_matches(|ch: char| ch.is_alphanumeric() || matches!(ch, ':' | '_' | '-' | '.' | '*'));
    // Right after the brace, or after a comma in `\cref{a, `.
    let command = match key.strip_suffix('{') {
        Some(command) => command,
        None => {
            let list = key.trim_end().strip_suffix(',')?;
            let open = list.rfind('{')?;
            if list[open..].contains(['}', '\\']) {
                return None;
            }
            &list[..open]
        }
    };
    let command = strip_options(command);
    // The second label of `\crefrange{a}{`.
    let (command, second) = match command.strip_suffix('}') {
        Some(inner) => (&inner[..inner.rfind('{')?], true),
        None => (command, false),
    };
    let command = command.strip_suffix('*').unwrap_or(command);
    let name = &command[command.trim_end_matches(|ch: char| ch.is_ascii_alphabetic()).len()..];
    if !command[..command.len() - name.len()].ends_with('\\') {
        return None;
    }
    if second && !name.ends_with("range") {
        return None;
    }
    let lists = key.ends_with('{') || matches!(name, "cref" | "Cref" | "cpageref" | "Cpageref" | "labelcref" | "zcref" | "zcpageref");
    let kind = if name == "begin" && key.ends_with('{') {
        TriggerKind::Environment
    } else if citations::tex::is_ref(name) && lists {
        TriggerKind::Label
    } else {
        return None;
    };
    let prefer = matches!(name, "eqref").then_some("eq:");
    Some(Trigger { kind, start: key.len(), prefer })
}

/// The command before its `[…]` options, as in `\zcref[S]`.
fn strip_options(command: &str) -> &str {
    let mut rest = command;
    while let Some(inner) = rest.strip_suffix(']') {
        match inner.rfind('[') {
            Some(open) => rest = &inner[..open],
            None => return command,
        }
    }
    rest
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
        return (!rest.ends_with('\\')).then_some(Trigger { kind: TriggerKind::Code, start, prefer: None });
    }
    let rest = name.strip_suffix('@')?;
    // `a@b` is a mail address, not a reference. A bare `@` opens the citation
    // picker instead, see `snippets::opens_reference`.
    let joined = rest.chars().next_back().is_some_and(char::is_alphanumeric);
    (!joined && start < before.len()).then_some(Trigger { kind: TriggerKind::Label, start, prefer: None })
}

/// The best suggestions for `prefix`, the text that accepting one replaces.
/// LaTeX commands of packages that `loaded` certainly lacks are left out.
pub fn suggestions<'a>(
    language: Language,
    kind: TriggerKind,
    prefix: &str,
    symbols: impl Iterator<Item = &'a Symbol>,
    loaded: &Loaded,
    extra: &'a Extra,
    labels: LabelHints<'a>,
) -> Vec<Entry> {
    let candidates = candidates(language, kind, symbols, loaded, extra, labels.details);
    // Runs on every key, so nothing is copied before the best few are known.
    let mut scored: Vec<(i64, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let score = fuzzy_score(prefix, candidate.label)?;
            let bonus = if candidate.label.starts_with(prefix) { PREFIX_BONUS } else { 0 };
            // `\eqref` takes equations, so `eq:` labels come first among equals.
            let preferred = labels.prefer.is_some_and(|prefer| candidate.template.is_none() && candidate.label.starts_with(prefer));
            Some((score + bonus + if preferred { PREFIX_BONUS / 2 } else { 0 }, index))
        })
        .collect();
    // Stable, so that among equals the earlier candidate wins, also for doubles.
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let mut items: Vec<Entry> = Vec::with_capacity(MAX_ITEMS);
    for (_, index) in scored {
        let candidate = &candidates[index];
        if items.iter().any(|item| item.label == candidate.label) {
            continue;
        }
        items.push(candidate.entry());
        if items.len() == MAX_ITEMS {
            break;
        }
    }
    // A finished word needs no popup that could only repeat it.
    if let [only] = items.as_slice()
        && only.label == prefix
    {
        return Vec::new();
    }
    items
}

/// A suggestion before it is chosen, borrowed from the lists or the symbols.
struct Candidate<'a> {
    label: &'a str,
    /// `None` for a project name, which goes in as literal text.
    template: Option<&'a str>,
    detail: &'a str,
}

impl Candidate<'_> {
    fn entry(&self) -> Entry {
        Entry {
            label: self.label.to_string(),
            template: self.template.map_or_else(|| literal(self.label), str::to_string),
            detail: self.detail.to_string(),
        }
    }
}

/// In the order that wins among doubles: the project's own names, the fixed
/// lists with their templates, then what packages and use add.
fn candidates<'a>(
    language: Language,
    kind: TriggerKind,
    symbols: impl Iterator<Item = &'a Symbol>,
    loaded: &Loaded,
    extra: &'a Extra,
    details: Option<&'a HashMap<String, String>>,
) -> Vec<Candidate<'a>> {
    let wanted = match kind {
        TriggerKind::Command => Some(SymbolKind::Command),
        TriggerKind::Label => Some(SymbolKind::Label),
        TriggerKind::Code => Some(SymbolKind::Binding),
        TriggerKind::Environment => None,
    };
    let mut candidates: Vec<Candidate> = symbols
        .filter(|symbol| Some(symbol.kind) == wanted)
        .map(|symbol| {
            let detail = details.and_then(|details| details.get(&symbol.name)).map_or("project", String::as_str);
            Candidate { label: &symbol.name, template: None, detail }
        })
        .collect();
    let fixed = |label: &'a str, template: &'a str| Candidate { label, template: Some(template), detail: "" };
    match (language, kind) {
        (_, TriggerKind::Environment) => {
            candidates.extend(LATEX_ENVIRONMENTS.iter().map(|name| fixed(name, name)));
            let packages = extra.environments.iter().map(|(name, package)| Candidate { label: name, template: Some(name), detail: package });
            candidates.extend(packages);
        }
        (Language::Latex, TriggerKind::Command) => {
            let commands = LATEX_COMMANDS.iter().map(|template| fixed(command_name(template), template));
            let palette = palette_commands().map(|(label, template, sign)| Candidate { label, template: Some(template), detail: sign });
            candidates.extend(commands.chain(palette).filter(|candidate| loaded.provides(candidate.label)));
            let packages = extra.commands.iter().map(|command| Candidate { label: &command.name, template: Some(&command.template), detail: &command.package });
            let used = extra.used.iter().map(|name| Candidate { label: name, template: None, detail: "used" });
            candidates.extend(packages.chain(used));
        }
        (Language::Typst, TriggerKind::Code) => {
            candidates.extend(TYPST_CODE.iter().map(|template| fixed(typst_name(template), template)));
        }
        _ => {}
    }
    candidates
}

/// The commands of the symbol palette as name, template and sign.
fn palette_commands() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    snippets::GROUPS.iter().flat_map(|group| group.symbols).filter_map(|symbol| {
        let name = command_name(symbol.latex);
        (name.len() > 1 && name != "\\begin").then_some((name, symbol.latex, symbol.label))
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
    "\\autoref{${}}",
    "\\cref{${}}",
    "\\Cref{${}}",
    "\\vref{${}}",
    "\\nameref{${}}",
    // natbib and biblatex: each opens the citation picker inside its braces.
    "\\cite{${}}",
    "\\citep{${}}",
    "\\citet{${}}",
    "\\citep*{${}}",
    "\\citet*{${}}",
    "\\citealp{${}}",
    "\\citealt{${}}",
    "\\citeauthor{${}}",
    "\\citeyear{${}}",
    "\\citeyearpar{${}}",
    "\\autocite{${}}",
    "\\Autocite{${}}",
    "\\parencite{${}}",
    "\\Parencite{${}}",
    "\\textcite{${}}",
    "\\Textcite{${}}",
    "\\footcite{${}}",
    "\\smartcite{${}}",
    "\\fullcite{${}}",
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
    use editor_core::snippet::Snippet;

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
        assert_eq!(at(Language::Latex, "\\cref{fig:a, tab"), Some((TriggerKind::Label, "tab")));
        assert_eq!(at(Language::Latex, "\\Cref{a,"), Some((TriggerKind::Label, "")));
        assert_eq!(at(Language::Latex, "\\zcref[S]{eq:x,eq"), Some((TriggerKind::Label, "eq")));
        assert_eq!(at(Language::Latex, "\\crefrange{fig:a}{fig"), Some((TriggerKind::Label, "fig")));
        assert_eq!(at(Language::Latex, "\\subref*{a"), Some((TriggerKind::Label, "a")));
        // `\ref` takes one label, so a comma ends it.
        assert_eq!(at(Language::Latex, "\\ref{a,"), None);
        assert_eq!(at(Language::Latex, "\\begin{a,"), None);
        assert_eq!(trigger(Language::Latex, "\\eqref{").map(|trigger| trigger.prefer), Some(Some("eq:")));
    }

    #[test]
    fn labels_show_what_they_are_and_eqref_prefers_equations() {
        let symbols = [
            Symbol { kind: SymbolKind::Label, name: "eq:energy".to_string() },
            Symbol { kind: SymbolKind::Label, name: "fig:energy".to_string() },
        ];
        let details: HashMap<String, String> = [("fig:energy".to_string(), "figure 3.2: Energy use".to_string())].into();
        let hints = LabelHints { details: Some(&details), prefer: None };
        let items = suggestions(Language::Latex, TriggerKind::Label, "en", symbols.iter(), &Loaded::default(), &Extra::default(), hints);
        let detail = |label: &str| items.iter().find(|item| item.label == label).map(|item| item.detail.clone());
        assert_eq!((detail("fig:energy").as_deref(), detail("eq:energy").as_deref()), (Some("figure 3.2: Energy use"), Some("project")));
        let hints = LabelHints { details: Some(&details), prefer: Some("eq:") };
        let items = suggestions(Language::Latex, TriggerKind::Label, "energy", symbols.iter(), &Loaded::default(), &Extra::default(), hints);
        assert_eq!(items[0].label, "eq:energy");
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
    #[ignore = "prints a timing, run with --ignored --nocapture --release"]
    fn suggestion_timing() {
        let symbols: Vec<Symbol> = (0..200).map(|index| Symbol { kind: SymbolKind::Command, name: format!("\\own{index}") }).collect();
        let article = build::loaded(&build::ProjectUsage { classes: vec!["article".to_string()], ..Default::default() });
        // About glossaries, biblatex and a few more, and a long document's commands.
        let packages = Extra {
            commands: (0..1_000)
                .map(|index| PackageCommand { name: format!("\\pkg{index}"), template: format!("\\pkg{index}{{${{}}}}"), package: "p".to_string() })
                .collect(),
            environments: Vec::new(),
            used: (0..500).map(|index| format!("\\used{index}")).collect(),
        };
        for (name, loaded, extra) in [("unsure", Loaded::default(), Extra::default()), ("article", article, Extra::default()), ("1500 more", Loaded::default(), packages)] {
            for prefix in ["\\c", "\\cit", "\\sec"] {
                let started = std::time::Instant::now();
                for _ in 0..10_000 {
                    std::hint::black_box(suggestions(Language::Latex, TriggerKind::Command, prefix, symbols.iter(), &loaded, &extra, LabelHints::default()));
                }
                println!("{name} {prefix}: {:.1} µs per call", started.elapsed().as_secs_f64() * 1e6 / 10_000.);
            }
        }
    }

    #[test]
    fn commands_of_missing_packages_stay_out() {
        let article = |packages: &[&str]| {
            build::loaded(&build::ProjectUsage {
                classes: vec!["article".to_string()],
                packages: packages.iter().map(ToString::to_string).collect(),
                ..Default::default()
            })
        };
        let cite = |loaded: &Loaded| labels(&suggestions(Language::Latex, TriggerKind::Command, "\\cit", std::iter::empty(), loaded, &Extra::default(), LabelHints::default())).join(" ");
        let plain = cite(&article(&[]));
        assert!(plain.contains("\\cite") && !plain.contains("\\citep") && !plain.contains("\\citealp"), "{plain}");
        assert!(cite(&article(&["natbib"])).contains("\\citep"));
        // A `\usepackage` that only the open file has so far.
        assert!(cite(&article(&[]).with(["natbib"])).contains("\\citep"));
        // Own commands stay, whatever is loaded.
        let own = [Symbol { kind: SymbolKind::Command, name: "\\citeown".to_string() }];
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\citeo", own.iter(), &article(&[]), &Extra::default(), LabelHints::default());
        assert_eq!(labels(&items), ["\\citeown"]);
        // Sections and other LaTeX commands need no package.
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\sec", std::iter::empty(), &article(&[]), &Extra::default(), LabelHints::default());
        assert_eq!(items[0].label, "\\section");
    }

    #[test]
    fn packages_and_the_project_add_their_commands() {
        let extra = Extra {
            commands: vec![
                PackageCommand { name: "\\glsdesc".to_string(), template: "\\glsdesc{${}}".to_string(), package: "glossaries".to_string() },
                // The built-in template wins over a double from a package.
                PackageCommand { name: "\\section".to_string(), template: "\\section".to_string(), package: "x".to_string() },
            ],
            environments: vec![("glossarytable".to_string(), "glossaries".to_string())],
            used: vec!["\\myniche".to_string(), "\\section".to_string()],
        };
        let at = |kind, prefix| suggestions(Language::Latex, kind, prefix, std::iter::empty(), &Loaded::default(), &extra, LabelHints::default());
        let items = at(TriggerKind::Command, "\\glsd");
        assert_eq!((items[0].label.as_str(), items[0].template.as_str(), items[0].detail.as_str()), ("\\glsdesc", "\\glsdesc{${}}", "glossaries"));
        let items = at(TriggerKind::Command, "\\mynic");
        assert_eq!((items[0].label.as_str(), items[0].template.as_str(), items[0].detail.as_str()), ("\\myniche", "\\myniche", "used"));
        let items = at(TriggerKind::Command, "\\sectio");
        assert_eq!((items[0].template.as_str(), items.iter().filter(|item| item.label == "\\section").count()), ("\\section{${}}", 1));
        assert_eq!(at(TriggerKind::Environment, "glossaryt")[0].detail, "glossaries");
    }

    /// The text before the cursor once a suggestion is accepted.
    fn before_cursor(template: &str) -> String {
        let snippet = Snippet::parse(template, "");
        snippet.text[..snippet.slots[0].start].to_string()
    }

    #[test]
    fn every_citation_command_opens_the_picker() {
        let cites: Vec<&str> = LATEX_COMMANDS.iter().copied().filter(|template| command_name(template).to_ascii_lowercase().contains("cite")).collect();
        assert_eq!(cites.len(), 19);
        for template in cites {
            assert!(snippets::opens_citation(&before_cursor(template)), "{template}");
        }
        for (typed, wanted) in [("\\citep", "\\citep"), ("\\textc", "\\textcite"), ("\\parenc", "\\parencite"), ("\\citet", "\\citet*")] {
            let items = suggestions(Language::Latex, TriggerKind::Command, typed, std::iter::empty(), &Loaded::default(), &Extra::default(), LabelHints::default());
            assert!(labels(&items).contains(&wanted), "{typed}: {:?}", labels(&items));
        }
        // The plain command stays first among its variants.
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\cite", std::iter::empty(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items[0].label, "\\cite");
    }

    #[test]
    fn every_reference_command_suggests_labels() {
        for name in ["\\ref", "\\eqref", "\\pageref", "\\autoref", "\\cref", "\\Cref", "\\vref", "\\nameref"] {
            let template = LATEX_COMMANDS.iter().find(|template| command_name(template) == name).expect(name);
            assert_eq!(at(Language::Latex, &before_cursor(template)), Some((TriggerKind::Label, "")), "{name}");
        }
    }

    #[test]
    fn commands_come_from_the_list_and_the_palette() {
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\fr", std::iter::empty(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items[0].label, "\\frac");
        assert_eq!(items[0].template, "\\frac{${}}{${}}");
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\alph", std::iter::empty(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items[0].detail, "α");
    }

    #[test]
    fn project_symbols_join_by_kind() {
        let symbols = [
            Symbol { kind: SymbolKind::Label, name: "fig:a".to_string() },
            Symbol { kind: SymbolKind::Command, name: "\\myfoo".to_string() },
        ];
        let items = suggestions(Language::Latex, TriggerKind::Label, "fi", symbols.iter(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(labels(&items), ["fig:a"]);
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\myf", symbols.iter(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items[0].label, "\\myfoo");
    }

    #[test]
    fn symbols_with_dollars_stay_literal() {
        let symbols = [Symbol { kind: SymbolKind::Label, name: "a$b".to_string() }];
        let items = suggestions(Language::Latex, TriggerKind::Label, "a", symbols.iter(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items[0].template, "a$$b");
    }

    #[test]
    fn a_finished_word_closes_the_popup() {
        let symbols = [Symbol { kind: SymbolKind::Label, name: "fig:a".to_string() }];
        assert!(suggestions(Language::Latex, TriggerKind::Label, "fig:a", symbols.iter(), &Loaded::default(), &Extra::default(), LabelHints::default()).is_empty());
    }

    #[test]
    fn the_popup_stays_short_and_has_no_doubles() {
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\", std::iter::empty(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items.len(), MAX_ITEMS);
        let symbols = [Symbol { kind: SymbolKind::Command, name: "\\frac".to_string() }];
        let items = suggestions(Language::Latex, TriggerKind::Command, "\\fra", symbols.iter(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items.iter().filter(|entry| entry.label == "\\frac").count(), 1);
    }

    #[test]
    fn environments_and_typst_names_match() {
        let items = suggestions(Language::Latex, TriggerKind::Environment, "ite", std::iter::empty(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items[0].label, "itemize");
        let items = suggestions(Language::Typst, TriggerKind::Code, "fig", std::iter::empty(), &Loaded::default(), &Extra::default(), LabelHints::default());
        assert_eq!(items[0].label, "figure");
        assert_eq!(items[0].template, "figure(${})");
    }
}
