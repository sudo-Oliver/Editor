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
    if language == Language::Typst {
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

/// `relative` is seen from where the compiler looks for the file.
pub fn figure(language: Language, relative: &str) -> String {
    if language == Language::Typst {
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
    if language == Language::Typst { format!("@{key}") } else { format!("\\cite{{{key}}}") }
}

/// A citation key for the braces of a `\cite{` at the cursor. Adds the closing
/// brace only when the text after the cursor does not have it.
pub fn citation_key(key: &str, closing: bool) -> String {
    let key = literal(key);
    if closing { format!("{key}}}") } else { key }
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

/// Whether `before` ends in the `@` of a Typst reference or citation, not in
/// a mail address or a comment.
pub fn opens_reference(before: &str) -> bool {
    before.strip_suffix('@').is_some_and(|rest| !rest.ends_with(char::is_alphanumeric) && !rest.contains("//"))
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
    }

    #[test]
    fn figures_escape_the_path() {
        let typst = Snippet::parse(&figure(Language::Typst, "a\"$b.png"), "");
        assert!(typst.text.contains("image(\"a\\\"$b.png\")"));
        let latex = Snippet::parse(&figure(Language::Latex, "x.svg"), "");
        assert!(latex.text.contains("\\includesvg[width=\\linewidth]{x.svg}"));
    }

    #[test]
    fn citations_follow_the_language() {
        assert_eq!(citation(Language::Latex, "a"), "\\cite{a}");
        assert_eq!(citation_key("a", true), "a}");
        assert_eq!(citation_key("a", false), "a");
        assert_eq!(citation(Language::Typst, "a"), "@a");
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

    /// Needs `typst` and `pdflatex`: `cargo test -p app -- --ignored`.
    #[test]
    #[ignore]
    fn every_symbol_compiles() {
        let directory = tempfile::tempdir().unwrap();
        let typst = directory.path().join("all.typ");
        // The figure stays out, because it needs an image file.
        let body = format!("{}\n\n{}", all(Language::Typst), filled(&table(Language::Typst, 2, 2)));
        std::fs::write(&typst, body).unwrap();
        let output = Command::new("typst").arg("compile").arg(&typst).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

        let latex = directory.path().join("all.tex");
        let body = format!("{}\n\n{}", all(Language::Latex), filled(&table(Language::Latex, 2, 2)));
        std::fs::write(
            &latex,
            format!("\\documentclass{{article}}\n\\usepackage{{amsmath,amssymb}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n"),
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
    fn citation_commands_are_recognized() {
        for text in ["\\cite{", "x \\citep{", "\\autocite[p. 3]{", "\\parencite[a][b]{", "\\citep*{", "\\Cite{", "\\footcite{"] {
            assert!(opens_citation(text), "{text}");
        }
        for text in ["\\section{", "cite{", "\\cite", "\\cite{a", "\\cite[{", "\\excited[x\n]{", "\\\\cit{", "{"] {
            assert!(!opens_citation(text), "{text}");
        }
    }
}
