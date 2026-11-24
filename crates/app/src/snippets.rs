//! What the editor toolbar inserts, per language. Templates use the slots of
//! `editor_core::snippet`.

use std::path::Path;

use editor_core::{Language, snippet::literal};

use crate::image_view::is_image;

pub struct Symbol {
    /// What the palette shows.
    pub label: &'static str,
    pub latex: &'static str,
    pub typst: &'static str,
}

impl Symbol {
    pub fn template(&self, language: Language) -> &'static str {
        if language == Language::Typst { self.typst } else { self.latex }
    }
}

pub struct Group {
    pub name: &'static str,
    pub symbols: &'static [Symbol],
}

const fn s(label: &'static str, latex: &'static str, typst: &'static str) -> Symbol {
    Symbol { label, latex, typst }
}

pub const GROUPS: &[Group] = &[
    Group {
        name: "Templates",
        symbols: &[
            s("a/b", "\\frac{${}}{${}}", "(${})/(${})"),
            s("√x", "\\sqrt{${}}", "sqrt(${})"),
            s("xⁿ", "${}^{${}}", "${}^(${})"),
            s("xₙ", "${}_{${}}", "${}_(${})"),
            s("∑", "\\sum_{${}}^{${}}", "sum_(${})^(${})"),
            s("∏", "\\prod_{${}}^{${}}", "product_(${})^(${})"),
            s("∫", "\\int_{${}}^{${}}", "integral_(${})^(${})"),
            s("lim", "\\lim_{${} \\to ${}}", "lim_(${} -> ${})"),
            s("( )", "\\left( ${} \\right)", "lr(( ${} ))"),
            s("x̄", "\\bar{${}}", "macron(${})"),
            s("x̂", "\\hat{${}}", "hat(${})"),
            s("x⃗", "\\vec{${}}", "arrow(${})"),
            s("[⋮]", "\\begin{pmatrix}\n  ${} & ${} \\\\\n  ${} & ${}\n\\end{pmatrix}", "mat(${}, ${}; ${}, ${})"),
            s("{⋮", "\\begin{cases}\n  ${} & ${} \\\\\n  ${} & ${}\n\\end{cases}", "cases(${} & ${}, ${} & ${})"),
        ],
    },
    Group {
        name: "Greek",
        symbols: &[
            s("α", "\\alpha", "alpha"),
            s("β", "\\beta", "beta"),
            s("γ", "\\gamma", "gamma"),
            s("δ", "\\delta", "delta"),
            s("ε", "\\varepsilon", "epsilon"),
            s("ζ", "\\zeta", "zeta"),
            s("η", "\\eta", "eta"),
            s("θ", "\\theta", "theta"),
            s("ι", "\\iota", "iota"),
            s("κ", "\\kappa", "kappa"),
            s("λ", "\\lambda", "lambda"),
            s("μ", "\\mu", "mu"),
            s("ν", "\\nu", "nu"),
            s("ξ", "\\xi", "xi"),
            s("π", "\\pi", "pi"),
            s("ρ", "\\rho", "rho"),
            s("σ", "\\sigma", "sigma"),
            s("τ", "\\tau", "tau"),
            s("υ", "\\upsilon", "upsilon"),
            s("φ", "\\varphi", "phi"),
            s("χ", "\\chi", "chi"),
            s("ψ", "\\psi", "psi"),
            s("ω", "\\omega", "omega"),
            s("Γ", "\\Gamma", "Gamma"),
            s("Δ", "\\Delta", "Delta"),
            s("Θ", "\\Theta", "Theta"),
            s("Λ", "\\Lambda", "Lambda"),
            s("Ξ", "\\Xi", "Xi"),
            s("Π", "\\Pi", "Pi"),
            s("Σ", "\\Sigma", "Sigma"),
            s("Φ", "\\Phi", "Phi"),
            s("Ψ", "\\Psi", "Psi"),
            s("Ω", "\\Omega", "Omega"),
        ],
    },
    Group {
        name: "Operators",
        symbols: &[
            s("±", "\\pm", "plus.minus"),
            s("×", "\\times", "times"),
            s("÷", "\\div", "div"),
            s("·", "\\cdot", "dot.op"),
            s("∘", "\\circ", "compose"),
            s("∂", "\\partial", "partial"),
            s("∇", "\\nabla", "nabla"),
            s("∞", "\\infty", "oo"),
            s("∮", "\\oint", "integral.cont"),
            s("∪", "\\cup", "union"),
            s("∩", "\\cap", "∩"),
            s("∅", "\\emptyset", "emptyset"),
            s("∀", "\\forall", "forall"),
            s("∃", "\\exists", "exists"),
            s("¬", "\\neg", "not"),
            s("∧", "\\land", "and"),
            s("∨", "\\lor", "or"),
        ],
    },
    Group {
        name: "Relations",
        symbols: &[
            s("≤", "\\leq", "<="),
            s("≥", "\\geq", ">="),
            s("≠", "\\neq", "!="),
            s("≈", "\\approx", "approx"),
            s("≡", "\\equiv", "equiv"),
            s("∼", "\\sim", "tilde.op"),
            s("∝", "\\propto", "prop"),
            s("≪", "\\ll", "<<"),
            s("≫", "\\gg", ">>"),
            s("∈", "\\in", "in"),
            s("∉", "\\notin", "in.not"),
            s("⊂", "\\subset", "subset"),
            s("⊆", "\\subseteq", "subset.eq"),
            s("⊃", "\\supset", "supset"),
            s("⊇", "\\supseteq", "supset.eq"),
            s("⟂", "\\perp", "perp"),
            s("∥", "\\parallel", "parallel"),
        ],
    },
    Group {
        name: "Arrows",
        symbols: &[
            s("→", "\\rightarrow", "->"),
            s("←", "\\leftarrow", "<-"),
            s("↔", "\\leftrightarrow", "<->"),
            s("⇒", "\\Rightarrow", "=>"),
            s("⇐", "\\Leftarrow", "arrow.l.double"),
            s("⇔", "\\Leftrightarrow", "<=>"),
            s("↦", "\\mapsto", "|->"),
            s("↑", "\\uparrow", "arrow.t"),
            s("↓", "\\downarrow", "arrow.b"),
        ],
    },
];

pub fn table(language: Language, rows: usize, columns: usize) -> String {
    let row = |cell: &str, separator: &str| vec![cell; columns].join(separator);
    if language == Language::Markdown {
        // The first row is the header, which a pipe table needs.
        let line = format!("| {} |", row("${}", " | "));
        let rule = format!("|{}", "---|".repeat(columns));
        let body = vec![line.clone(); rows.saturating_sub(1)];
        [vec![line, rule], body].concat().join("\n") + "\n"
    } else if language == Language::Typst {
        let lines = vec![format!("  {},", row("[${}]", ", ")); rows].join("\n");
        format!("#table(\n  columns: {columns},\n{lines}\n)\n")
    } else {
        let lines = vec![format!("    {} \\\\\n    \\hline", row("${}", " & ")); rows].join("\n");
        let spec = format!("|{}", "c|".repeat(columns));
        format!(
            "\\begin{{table}}[h]\n  \\centering\n  \\begin{{tabular}}{{{spec}}}\n    \\hline\n{lines}\n  \\end{{tabular}}\n  \\caption{{${{Caption}}}}\n  \\label{{tab:${{label}}}}\n\\end{{table}}\n"
        )
    }
}

pub struct CodeLanguage {
    /// What the popup shows.
    pub label: &'static str,
    /// The name Typst's raw blocks know, empty for none.
    pub typst: &'static str,
    /// The options `lstlisting` gets: a language `listings` knows, or the rules
    /// of one it does not, so that the code is still highlighted.
    pub listings: &'static str,
}

const fn code(label: &'static str, typst: &'static str, listings: &'static str) -> CodeLanguage {
    CodeLanguage { label, typst, listings }
}

pub const CODE_LANGUAGES: &[CodeLanguage] = &[
    code("Python", "python", "language=Python"),
    code("C", "c", "language=C"),
    code("C++", "cpp", "language=C++"),
    code("Java", "java", "language=Java"),
    code(
        "Rust",
        "rust",
        "morekeywords={as,async,await,break,const,continue,crate,dyn,else,enum,extern,false,fn,for,if,impl,in,let,loop,match,mod,move,mut,pub,ref,return,self,Self,static,struct,super,trait,true,type,unsafe,use,where,while}, sensitive=true, morecomment={[l]{//}}, morecomment={[s]{/*}{*/}}, morestring={[b]\"}",
    ),
    code(
        "JavaScript",
        "js",
        "morekeywords={async,await,break,case,catch,class,const,continue,default,delete,do,else,export,extends,false,finally,for,function,if,import,in,instanceof,let,new,null,of,return,super,switch,this,throw,true,try,typeof,undefined,var,void,while,yield}, sensitive=true, morecomment={[l]{//}}, morecomment={[s]{/*}{*/}}, morestring={[b]\"}, morestring={[b]'}",
    ),
    code(
        "TypeScript",
        "ts",
        "morekeywords={abstract,any,as,async,await,boolean,break,case,catch,class,const,continue,default,do,else,enum,export,extends,false,finally,for,function,if,implements,import,in,interface,let,new,null,number,private,protected,public,readonly,return,string,super,switch,this,throw,true,try,type,typeof,undefined,var,void,while}, sensitive=true, morecomment={[l]{//}}, morecomment={[s]{/*}{*/}}, morestring={[b]\"}, morestring={[b]'}",
    ),
    code(
        "Go",
        "go",
        "morekeywords={break,case,chan,const,continue,default,defer,else,fallthrough,false,for,func,go,goto,if,import,interface,map,nil,package,range,return,select,struct,switch,true,type,var}, sensitive=true, morecomment={[l]{//}}, morecomment={[s]{/*}{*/}}, morestring={[b]\"}",
    ),
    code("Bash", "bash", "language=bash"),
    code("SQL", "sql", "language=SQL"),
    code("HTML", "html", "language=HTML"),
    code("Haskell", "haskell", "language=Haskell"),
    code("Plain text", "", ""),
];

/// A code block with syntax highlighting. LaTeX takes `listings`, which needs
/// no `-shell-escape`, unlike `minted`.
pub fn code_block(language: Language, code: &CodeLanguage) -> String {
    if matches!(language, Language::Typst | Language::Markdown) {
        format!("```{}\n${{}}\n```\n", code.typst)
    } else {
        let options = if code.listings.is_empty() { String::new() } else { format!("[{}]", code.listings) };
        format!("\\begin{{lstlisting}}{}\n${{}}\n\\end{{lstlisting}}\n", options.replace('$', "$$"))
    }
}

/// `relative` is seen from where the compiler looks for the file.
pub fn figure(language: Language, relative: &str) -> String {
    if language == Language::Markdown {
        // Angle brackets let the path hold spaces and parentheses.
        let path = literal(&relative.replace('<', "%3C").replace('>', "%3E"));
        format!("![${{Caption}}](<{path}>)\n")
    } else if language == Language::Typst {
        let path = literal(&relative.replace('\\', "\\\\").replace('"', "\\\""));
        format!("#figure(\n  image(\"{path}\"),\n  caption: [${{Caption}}],\n) <fig:${{label}}>\n")
    } else {
        let path = literal(relative);
        let command = if relative.to_ascii_lowercase().ends_with(".svg") { "includesvg" } else { "includegraphics" };
        format!(
            "\\begin{{figure}}[h]\n  \\centering\n  \\{command}[width=\\linewidth]{{{path}}}\n  \\caption{{${{Caption}}}}\n  \\label{{fig:${{label}}}}\n\\end{{figure}}\n"
        )
    }
}

pub fn citation(language: Language, key: &str) -> String {
    let key = literal(key);
    match language {
        Language::Typst => format!("@{key}"),
        Language::Markdown => format!("[@{key}]"),
        _ => format!("\\cite{{{key}}}"),
    }
}

/// Whether `before`, the text before the cursor, ends in the opening brace of a
/// LaTeX citation command such as `\cite{`, `\autocite[p. 3]{` or `\citep*{`.
pub fn opens_citation(before: &str) -> bool {
    let Some(mut rest) = before.strip_suffix('{') else {
        return false;
    };
    while let Some(inner) = rest.strip_suffix(']') {
        // An optional argument, on the same line.
        let Some(open) = inner.rfind('[') else {
            return false;
        };
        if inner[open..].contains('\n') {
            return false;
        }
        rest = &inner[..open];
    }
    let rest = rest.strip_suffix('*').unwrap_or(rest);
    let name_length = rest.chars().rev().take_while(char::is_ascii_alphabetic).map(char::len_utf8).sum::<usize>();
    let (head, name) = rest.split_at(rest.len() - name_length);
    head.ends_with('\\') && name.to_ascii_lowercase().contains("cite")
}

/// Where a key goes at the cursor, inside the braces of a LaTeX citation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiteSlot {
    /// The keys already in these braces, which the picker leaves out.
    pub cited: Vec<String>,
    /// The braces are still open, so the key brings the `}`.
    pub close: bool,
    /// A key follows the cursor, so a comma goes between them.
    pub before_key: bool,
}

/// The slot for a key when the cursor is where one starts in a citation:
/// right after `\cite{` and its kind, after a comma in the list, or in a
/// further group of `\cites{a}{`. `before` and `after` are the text of the
/// line on either side of the cursor.
pub fn cite_slot(before: &str, after: &str) -> Option<CiteSlot> {
    let open = unclosed_brace(before)?;
    let inside = &before[open + 1..];
    if !(inside.trim().is_empty() || inside.trim_end().ends_with(',')) {
        return None;
    }
    if !opens_citation(&before[..open + 1]) && !continues_multicite(&before[..open]) {
        return None;
    }
    let end = after.find(['}', '{']).filter(|at| after.as_bytes()[*at] == b'}');
    let rest = &after[..end.unwrap_or(after.len())];
    let cited = inside.split(',').chain(rest.split(',')).map(str::trim).filter(|key| !key.is_empty()).map(str::to_string).collect();
    Some(CiteSlot { cited, close: end.is_none(), before_key: rest.trim_start().chars().next().is_some_and(|ch| ch != ',') })
}

/// Where the last `{` that is still open stands, not counting `\{`.
fn unclosed_brace(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    for index in (0..bytes.len()).rev() {
        let escaped = bytes[..index].iter().rev().take_while(|byte| **byte == b'\\').count() % 2 == 1;
        match bytes[index] {
            b'}' if !escaped => depth += 1,
            b'{' if !escaped && depth == 0 => return Some(index),
            b'{' if !escaped => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Whether `text`, up to a `{`, is a multicite command with groups before
/// it, as in `\autocites(see)()[p. 2]{a}[3]`.
fn continues_multicite(text: &str) -> bool {
    let mut rest = text.trim_end();
    let mut groups = 0;
    loop {
        let (open, close) = match rest.as_bytes().last() {
            Some(b'}') => (b'{', b'}'),
            Some(b']') => (b'[', b']'),
            Some(b')') => (b'(', b')'),
            _ => break,
        };
        let mut depth = 0usize;
        let Some(start) = rest.bytes().enumerate().rev().find_map(|(index, byte)| {
            if byte == close {
                depth += 1;
            } else if byte == open {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            None
        }) else {
            return false;
        };
        groups += usize::from(close == b'}');
        rest = rest[..start].trim_end();
    }
    let rest = rest.strip_suffix('*').unwrap_or(rest);
    let name_length = rest.chars().rev().take_while(char::is_ascii_alphabetic).count();
    let (head, name) = rest.split_at(rest.len() - name_length);
    groups > 0 && head.ends_with('\\') && name.len() > 4 && name.to_ascii_lowercase().ends_with("cites")
}

/// Whether `before` ends in the `@` of a Typst reference or citation, not in
/// a mail address or a comment, or in `#cite(<`.
pub fn opens_reference(before: &str) -> bool {
    before.strip_suffix('@').is_some_and(|rest| !rest.ends_with(char::is_alphanumeric) && !rest.contains("//")) || opens_typst_cite(before)
}

/// Whether `before` ends in the `@` of a Pandoc citation, `[@` or `; @`.
pub fn opens_markdown_cite(before: &str) -> bool {
    before.ends_with("[@") || before.ends_with("; @")
}

/// Whether `before` ends in the `<` of `cite(<`, where the key needs its `>`.
pub fn opens_typst_cite(before: &str) -> bool {
    before.ends_with("cite(<") && !before.contains("//")
}

/// Whether a figure in `language` can show this file.
pub fn is_figure_file(language: Language, path: &Path) -> bool {
    let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or_default().to_ascii_lowercase();
    match language {
        Language::Typst => is_image(path) || extension == "pdf",
        _ => matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "pdf" | "svg"),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use editor_core::snippet::Snippet;

    use super::*;

    #[test]
    fn tables_have_one_slot_per_cell() {
        let latex = Snippet::parse(&table(Language::Latex, 2, 3), "");
        assert_eq!(latex.slots.len(), 2 * 3 + 2);
        assert!(latex.text.contains("\\begin{tabular}{|c|c|c|}"));
        let typst = Snippet::parse(&table(Language::Typst, 2, 3), "");
        assert_eq!(typst.slots.len(), 2 * 3);
        assert!(typst.text.starts_with("#table(\n  columns: 3,\n  [], [], [],\n"));
        let markdown = Snippet::parse(&table(Language::Markdown, 2, 3), "");
        assert_eq!(markdown.slots.len(), 2 * 3);
        assert_eq!(markdown.text, "|  |  |  |\n|---|---|---|\n|  |  |  |\n");
    }

    #[test]
    fn figures_escape_the_path() {
        let typst = Snippet::parse(&figure(Language::Typst, "a\"$b.png"), "");
        assert!(typst.text.contains("image(\"a\\\"$b.png\")"));
        let latex = Snippet::parse(&figure(Language::Latex, "x.svg"), "");
        assert!(latex.text.contains("\\includesvg[width=\\linewidth]{x.svg}"));
        let markdown = Snippet::parse(&figure(Language::Markdown, "my <a>.png"), "");
        assert_eq!(markdown.text, "![Caption](<my %3Ca%3E.png>)\n");
    }

    #[test]
    fn code_blocks_take_the_selection() {
        let rust = CODE_LANGUAGES.iter().find(|code| code.label == "Rust").unwrap();
        let mut typst = Snippet::parse(&code_block(Language::Typst, rust), "");
        assert_eq!(typst.slots.len(), 1);
        typst.fill_first("fn main() {}");
        assert_eq!(typst.text, "```rust\nfn main() {}\n```\n");
        let latex = Snippet::parse(&code_block(Language::Latex, &CODE_LANGUAGES[0]), "");
        assert_eq!(latex.text, "\\begin{lstlisting}[language=Python]\n\n\\end{lstlisting}\n");
        let plain = Snippet::parse(&code_block(Language::Latex, CODE_LANGUAGES.last().unwrap()), "");
        assert_eq!(plain.text, "\\begin{lstlisting}\n\n\\end{lstlisting}\n");
    }

    #[test]
    fn citations_follow_the_language() {
        assert_eq!(citation(Language::Latex, "a"), "\\cite{a}");
        assert_eq!(citation(Language::Typst, "a"), "@a");
        assert_eq!(citation(Language::Markdown, "a"), "[@a]");
        assert!(opens_markdown_cite("see [@") && opens_markdown_cite("[@a; @"));
        assert!(!opens_markdown_cite("mail @"));
    }

    #[test]
    fn figure_files_follow_the_language() {
        assert!(is_figure_file(Language::Typst, Path::new("a.PDF")));
        assert!(is_figure_file(Language::Latex, Path::new("a.jpg")));
        assert!(!is_figure_file(Language::Latex, Path::new("a.webp")));
        assert!(!is_figure_file(Language::Typst, Path::new("a.tex")));
    }

    fn filled(template: &str) -> String {
        Snippet::parse(&template.replace("${}", "x"), "").text
    }

    fn all(language: Language) -> String {
        GROUPS
            .iter()
            .flat_map(|group| group.symbols)
            .map(|symbol| format!("$ {} $", filled(symbol.template(language))))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn code_blocks(language: Language) -> String {
        CODE_LANGUAGES
            .iter()
            .map(|code| Snippet::parse(&code_block(language, code).replace("${}", "if x then \"y\" // z"), "").text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Needs `typst` and `pdflatex`: `cargo test -p app -- --ignored`.
    #[test]
    #[ignore]
    fn every_symbol_compiles() {
        let directory = tempfile::tempdir().unwrap();
        let typst = directory.path().join("all.typ");
        // The figure stays out, because it needs an image file.
        let body = format!("{}\n\n{}\n\n{}", all(Language::Typst), filled(&table(Language::Typst, 2, 2)), code_blocks(Language::Typst));
        std::fs::write(&typst, body).unwrap();
        let output = Command::new("typst").arg("compile").arg(&typst).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

        let latex = directory.path().join("all.tex");
        let body = format!("{}\n\n{}\n\n{}", all(Language::Latex), filled(&table(Language::Latex, 2, 2)), code_blocks(Language::Latex));
        std::fs::write(
            &latex,
            format!("\\documentclass{{article}}\n\\usepackage{{amsmath,amssymb,listings}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n"),
        )
        .unwrap();
        let output = Command::new("pdflatex")
            .args(["-interaction=nonstopmode", "-halt-on-error", "all.tex"])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    }

    #[test]
    fn a_bare_at_opens_typst_references() {
        assert!(opens_reference("see @"));
        assert!(opens_reference("@"));
        assert!(!opens_reference("mail me@"));
        assert!(!opens_reference("// see @"));
        assert!(!opens_reference("see @fig"));
    }

    #[test]
    fn keys_go_after_the_brace_a_comma_or_into_the_next_group() {
        let slot = |before: &str, after: &str| cite_slot(before, after);
        assert_eq!(slot("\\cite{", ""), Some(CiteSlot { cited: vec![], close: true, before_key: false }));
        assert_eq!(slot("\\cite{", "}"), Some(CiteSlot { cited: vec![], close: false, before_key: false }));
        assert_eq!(slot("see \\citep[p. 3]{smith2020, ", "} and"), Some(CiteSlot { cited: vec!["smith2020".into()], close: false, before_key: false }));
        assert_eq!(slot("\\cite{a,", "b, c} x"), Some(CiteSlot { cited: vec!["a".into(), "b".into(), "c".into()], close: false, before_key: true }));
        assert_eq!(slot("\\autocites(see)()[p. 2]{a}[3]{", ""), Some(CiteSlot { cited: vec![], close: true, before_key: false }));
        assert!(slot("\\cites{a}{", "}").is_some());
        for before in ["\\cite{smi", "\\section{", "\\cite{a} {", "\\textbf{", "\\cite{a}{", "\\ref{x,"] {
            assert_eq!(slot(before, ""), None, "{before}");
        }
        assert!(opens_reference("#cite(<") && opens_typst_cite("x #cite(<"));
    }

    #[test]
    fn citation_commands_are_recognized() {
        for text in ["\\cite{", "x \\citep{", "\\autocite[p. 3]{", "\\parencite[a][b]{", "\\citep*{", "\\Cite{", "\\footcite{"] {
            assert!(opens_citation(text), "{text}");
        }
        for text in ["\\section{", "cite{", "\\cite", "\\cite{a", "\\cite[{", "\\excited[x\n]{", "\\\\cit{", "{"] {
            assert!(!opens_citation(text), "{text}");
        }
    }
}
