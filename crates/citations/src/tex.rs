//! Reads a LaTeX file once, as TeX would see the parts that matter here:
//! comments, `\verb`, verbatim environments and `\iffalse` hide what is in
//! them, and arguments may stand after spaces, newlines and comments.

use std::ops::Range;

use crate::scan::{BibKind, BibSource, External, Include, IncludeKind, Key, Label, Package, Scan, push_keys};

/// Environments whose content TeX does not read as commands.
const VERBATIM: &[&str] = &[
    "verbatim",
    "verbatim*",
    "Verbatim",
    "Verbatim*",
    "BVerbatim",
    "LVerbatim",
    "lstlisting",
    "minted",
    "comment",
    "filecontents",
    "filecontents*",
    "luacode",
    "luacode*",
    "pycode",
    "sageblock",
    "sagesilent",
    "tcblisting",
];

/// Environments with a caption and a number of their own.
const FLOATS: &[&str] = &[
    "figure",
    "figure*",
    "table",
    "table*",
    "wrapfigure",
    "wraptable",
    "sidewaysfigure",
    "sidewaystable",
    "subfigure",
    "subtable",
    "SCfigure",
    "SCtable",
    "marginfigure",
    "margintable",
    "listing",
    "algorithm",
    "longtable",
];

/// Environments that only arrange things, so a label in them belongs to the one around.
const TRANSPARENT: &[&str] = &[
    "document",
    "center",
    "flushleft",
    "flushright",
    "minipage",
    "adjustbox",
    "small",
    "footnotesize",
    "scriptsize",
    "tabular",
    "tabular*",
    "tabularx",
    "tabulary",
    "threeparttable",
    "resizebox",
    "subequations",
];

const SECTIONS: &[&str] = &["part", "chapter", "section", "subsection", "subsubsection", "paragraph", "subparagraph"];

/// Reference commands whose argument is a comma list of labels.
const LIST_REFS: &[&str] = &["cref", "Cref", "cpageref", "Cpageref", "labelcref", "labelcpageref", "zcref", "zcpageref"];

/// Reference commands with one label.
const REFS: &[&str] = &[
    "ref",
    "eqref",
    "pageref",
    "autoref",
    "Autoref",
    "autopageref",
    "nameref",
    "Nameref",
    "vref",
    "Vref",
    "vpageref",
    "fullref",
    "namecref",
    "nameCref",
    "lcnamecref",
    "namecrefs",
    "subref",
    "zref",
    "zpageref",
    "titleref",
];

/// Reference commands with a first and a last label.
const RANGE_REFS: &[&str] = &["crefrange", "Crefrange", "cpagerefrange", "Cpagerefrange", "vrefrange", "vpagerefrange"];

/// Commands with "cite" in their name whose argument is not keys.
const NOT_CITES: &[&str] = &["citestyle", "citetext", "citeindextrue", "citeindexfalse", "citereset", "citesetup", "DeclareCiteCommand", "DeclareMultiCiteCommand", "citeform"];

/// Whether `name` is a citation command of LaTeX, natbib or biblatex.
pub fn is_cite(name: &str) -> bool {
    name.to_ascii_lowercase().contains("cite") && !NOT_CITES.contains(&name) && !name.starts_with("Declare") && !name.starts_with("defcite")
}

/// Whether `name` takes several groups of keys, as in `\cites{a}{b}`.
fn is_multicite(name: &str) -> bool {
    name.len() > 4 && name.to_ascii_lowercase().ends_with("cites")
}

/// Whether `name` refers to a label.
pub fn is_ref(name: &str) -> bool {
    LIST_REFS.contains(&name) || REFS.contains(&name) || RANGE_REFS.contains(&name) || name == "hyperref"
}

pub fn scan(text: &str) -> Scan {
    let mut scanner = Scanner { text, bytes: text.as_bytes(), pos: 0, scan: Scan::default(), envs: Vec::new(), section: None };
    scanner.run();
    scanner.scan
}

struct Frame {
    name: String,
    float: bool,
    caption: Option<String>,
    /// Indices into the labels of the scan, which get the caption at the end.
    labels: Vec<usize>,
}

struct Scanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    scan: Scan,
    envs: Vec<Frame>,
    /// The last sectioning command and its title.
    section: Option<(String, String)>,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn run(&mut self) {
        while let Some(byte) = self.peek() {
            match byte {
                b'%' => self.comment(),
                b'\\' => self.command(),
                _ => self.pos += 1,
            }
        }
        while let Some(frame) = self.envs.pop() {
            self.close_frame(frame);
        }
    }

    fn line_end(&self, from: usize) -> usize {
        self.bytes[from..].iter().position(|byte| *byte == b'\n').map_or(self.bytes.len(), |newline| from + newline)
    }

    /// Skips a comment, reading `% !TEX root = main.tex` on the way.
    fn comment(&mut self) {
        let end = self.line_end(self.pos);
        let line = self.text[self.pos + 1..end].trim();
        if let Some(rest) = line.strip_prefix('!')
            && let Some((name, value)) = rest.split_once('=')
        {
            let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
            if name == "tex root" {
                self.scan.root = Some(value.trim().to_string());
            }
        }
        self.pos = end;
    }

    fn command(&mut self) {
        self.pos += 1;
        let name_start = self.pos;
        while self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        if self.pos == name_start {
            // `\%`, `\\`, `\{`: one escaped char.
            self.pos = (self.pos + 1).min(self.bytes.len());
            while !self.text.is_char_boundary(self.pos) {
                self.pos += 1;
            }
            return;
        }
        let text = self.text;
        let name = &text[name_start..self.pos];
        match name {
            "verb" => self.verb(),
            "begin" => self.begin(),
            "end" => self.end(),
            "iffalse" => self.skip_iffalse(),
            "documentclass" => {
                self.scan.document = true;
                let options = self.optional();
                if let Some(class) = self.group(false)
                    && self.text[class].trim() == "subfiles"
                    && let Some(options) = options
                {
                    self.scan.root = Some(self.text[options].trim().to_string());
                }
            }
            "usepackage" | "RequirePackage" => {
                let options = self.optional().map(|range| self.text[range].to_string()).unwrap_or_default();
                if let Some(names) = self.group(false) {
                    for name in self.text[names].split(',').map(str::trim).filter(|name| !name.is_empty()) {
                        self.scan.packages.push(Package { name: name.to_string(), options: options.clone() });
                    }
                }
            }
            "input" | "InputIfFileExists" => self.include(IncludeKind::Input, name == "input"),
            "include" => self.include(IncludeKind::Include, false),
            "subfile" => self.include(IncludeKind::Subfile, false),
            "import" | "subimport" | "inputfrom" | "subinputfrom" | "includefrom" | "subincludefrom" => {
                self.star();
                let Some(directory) = self.group(true) else {
                    return;
                };
                let Some(file) = self.group(true) else {
                    return;
                };
                let directory = self.text[directory].trim().to_string();
                let path = self.text[file.clone()].trim().to_string();
                if !path.is_empty() {
                    self.scan.includes.push(Include { path, range: file, kind: IncludeKind::Import { directory, relative: name.starts_with("sub") } });
                }
            }
            "bibliography" | "nobibliography" => self.bib_sources(BibKind::Bibliography),
            "addbibresource" | "addglobalbib" | "addsectionbib" => self.bib_sources(BibKind::Resource),
            "externaldocument" => {
                let prefix = self.optional().map(|range| self.text[range].trim().to_string()).unwrap_or_default();
                if let Some(name) = self.group(true) {
                    self.scan.external_documents.push(External { name: self.text[name].trim().to_string(), prefix });
                }
            }
            "label" | "zlabel" => self.label(),
            "caption" | "captionof" | "subcaption" | "subcaptionbox" => self.caption(name == "captionof"),
            "bibitem" => {
                self.optional();
                if let Some(range) = self.group(true) {
                    let command = name.to_string();
                    push_keys(&mut self.scan.bibitems, &self.text[range.clone()], range.start, &command);
                }
            }
            "hyperref" => {
                if let Some(range) = self.optional() {
                    push_keys(&mut self.scan.refs, &self.text[range.clone()], range.start, "hyperref");
                }
            }
            name if SECTIONS.contains(&name) => {
                let name = name.to_string();
                self.star();
                self.optional();
                if let Some(title) = self.group(false) {
                    self.section = Some((name, crate::text::clean(&self.text[title])));
                }
            }
            name if is_cite(name) => self.cite(name),
            name if is_ref(name) => self.reference(name),
            _ => {}
        }
    }

    fn star(&mut self) {
        let before = self.pos;
        self.spaces();
        if self.peek() == Some(b'*') {
            self.pos += 1;
        } else {
            self.pos = before;
        }
    }

    /// Spaces, newlines and comments that TeX skips between arguments.
    fn spaces(&mut self) {
        while let Some(byte) = self.peek() {
            match byte {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                b'%' => self.pos = self.line_end(self.pos),
                _ => break,
            }
        }
    }

    /// The inside of `[…]`, if one follows.
    fn optional(&mut self) -> Option<Range<usize>> {
        self.delimited(b'[', b']', false)
    }

    /// The inside of `{…}`, if one follows. With `short`, a blank line ends
    /// it unclosed, as TeX's `Paragraph ended before … was complete`, so
    /// that a key being typed does not take the rest of the file.
    fn group(&mut self, short: bool) -> Option<Range<usize>> {
        self.delimited(b'{', b'}', short)
    }

    fn delimited(&mut self, open: u8, close: u8, short: bool) -> Option<Range<usize>> {
        let before = self.pos;
        self.spaces();
        if self.peek() != Some(open) {
            self.pos = before;
            return None;
        }
        let start = self.pos + 1;
        let mut index = start;
        let mut depth = 0usize;
        while let Some(&byte) = self.bytes.get(index) {
            match byte {
                b'\\' => index += 1,
                b'%' => {
                    index = self.line_end(index);
                    continue;
                }
                b'{' if open != b'{' => depth += 1,
                b'}' if open != b'{' => depth = depth.saturating_sub(1),
                byte if byte == open => depth += 1,
                byte if byte == close && depth == 0 => {
                    self.pos = index + 1;
                    return Some(start..index);
                }
                byte if byte == close => depth -= 1,
                b'\n' if short && self.blank_line_after(index) => break,
                _ => {}
            }
            index += 1;
        }
        // Never closed: leave the rest to the main loop.
        self.pos = start;
        None
    }

    fn blank_line_after(&self, newline: usize) -> bool {
        self.bytes[newline + 1..].iter().take_while(|byte| **byte != b'\n').all(|byte| byte.is_ascii_whitespace()) && newline + 1 < self.bytes.len()
    }

    fn verb(&mut self) {
        if self.peek() == Some(b'*') {
            self.pos += 1;
        }
        let Some(delimiter) = self.peek() else {
            return;
        };
        let end = self.line_end(self.pos);
        self.pos = self.bytes[self.pos + 1..end].iter().position(|byte| *byte == delimiter).map_or(end, |at| self.pos + 1 + at + 1);
    }

    fn begin(&mut self) {
        let Some(range) = self.group(true) else {
            return;
        };
        let name = self.text[range].trim().to_string();
        if VERBATIM.contains(&name.as_str()) {
            let end = format!("\\end{{{name}}}");
            self.pos = self.text[self.pos..].find(&end).map_or(self.bytes.len(), |at| self.pos + at + end.len());
            return;
        }
        if name == "refsection"
            && let Some(range) = self.optional()
        {
            let text = &self.text[range.clone()];
            for (offset, piece) in pieces(text) {
                let at = range.start + offset;
                self.scan.bibliographies.push(BibSource { name: piece.to_string(), range: at..at + piece.len(), kind: BibKind::Resource, remote: false, foreign: false });
            }
        }
        self.envs.push(Frame { float: FLOATS.contains(&name.as_str()), name, caption: None, labels: Vec::new() });
    }

    fn end(&mut self) {
        let Some(range) = self.group(true) else {
            return;
        };
        let name = self.text[range].trim();
        if let Some(index) = self.envs.iter().rposition(|frame| frame.name == name) {
            while self.envs.len() > index {
                let frame = self.envs.pop().expect("a frame above the index");
                self.close_frame(frame);
            }
        }
    }

    /// Gives the labels of a float the caption that came after them.
    fn close_frame(&mut self, frame: Frame) {
        if let Some(caption) = frame.caption {
            for index in frame.labels {
                self.scan.labels[index].caption.get_or_insert_with(|| caption.clone());
            }
        }
    }

    /// Skips `\iffalse … \fi`, counting the `\if…` inside.
    fn skip_iffalse(&mut self) {
        let mut depth = 1usize;
        while let Some(byte) = self.peek() {
            match byte {
                b'%' => self.pos = self.line_end(self.pos),
                b'\\' => {
                    self.pos += 1;
                    let start = self.pos;
                    while self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
                        self.pos += 1;
                    }
                    if self.pos == start {
                        self.pos = (self.pos + 1).min(self.bytes.len());
                        while !self.text.is_char_boundary(self.pos) {
                            self.pos += 1;
                        }
                        continue;
                    }
                    let name = &self.text[start..self.pos];
                    if name == "fi" {
                        depth -= 1;
                        if depth == 0 {
                            return;
                        }
                    } else if name.starts_with("if") {
                        depth += 1;
                    }
                }
                _ => self.pos += 1,
            }
        }
    }

    fn include(&mut self, kind: IncludeKind, bare: bool) {
        if let Some(range) = self.group(true) {
            let path = self.text[range.clone()].trim().to_string();
            if !path.is_empty() && !path.contains('#') {
                self.scan.includes.push(Include { path, range, kind });
            }
            return;
        }
        // `\input chapter` without braces, as TeX allows.
        if !bare || !matches!(self.peek(), Some(b' ' | b'\t')) {
            return;
        }
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
        let start = self.pos;
        while self.peek().is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'\\' | b'{' | b'}' | b'%')) {
            self.pos += 1;
        }
        if self.pos > start {
            self.scan.includes.push(Include { path: self.text[start..self.pos].to_string(), range: start..self.pos, kind });
        }
    }

    fn bib_sources(&mut self, kind: BibKind) {
        let options = self.optional().map(|range| self.text[range].split_whitespace().collect::<String>().to_ascii_lowercase()).unwrap_or_default();
        let remote = options.contains("location=remote");
        let foreign = options.split(',').any(|option| option.strip_prefix("datatype=").is_some_and(|kind| kind != "bibtex"));
        let Some(range) = self.group(true) else {
            return;
        };
        let text = &self.text[range.clone()];
        for (offset, piece) in pieces(text) {
            let at = range.start + offset;
            self.scan.bibliographies.push(BibSource { name: piece.to_string(), range: at..at + piece.len(), kind, remote, foreign });
        }
    }

    fn label(&mut self) {
        self.optional();
        let Some(range) = self.group(true) else {
            return;
        };
        let name = self.text[range.clone()].trim();
        if name.is_empty() || name.contains(['#', '\\']) {
            return;
        }
        let at = range.start + (self.text[range.clone()].len() - self.text[range.clone()].trim_start().len());
        let index = self.scan.labels.len();
        let frame = self.envs.iter_mut().rev().find(|frame| !TRANSPARENT.contains(&frame.name.as_str()));
        let (env, caption, before_caption) = match frame {
            Some(frame) => {
                frame.labels.push(index);
                (frame.name.clone(), frame.caption.clone(), frame.float && frame.caption.is_none())
            }
            None => match &self.section {
                Some((kind, title)) => (kind.clone(), Some(title.clone()), false),
                None => (String::new(), None, false),
            },
        };
        self.scan.labels.push(Label { name: name.to_string(), range: at..at + name.len(), env, caption, before_caption });
    }

    fn caption(&mut self, of: bool) {
        self.star();
        if of {
            self.group(false);
        }
        self.optional();
        let Some(range) = self.group(false) else {
            return;
        };
        let caption = crate::text::clean(&self.text[range]);
        let caption: String = caption.chars().take(120).collect();
        // `\captionof` inside a minipage counts for that minipage too.
        if of && let Some(frame) = self.envs.last_mut() {
            frame.caption.get_or_insert_with(|| caption.clone());
        }
        if let Some(frame) = self.envs.iter_mut().rev().find(|frame| frame.float) {
            frame.caption.get_or_insert(caption);
        }
    }

    fn cite(&mut self, name: &str) {
        self.star();
        if is_multicite(name) {
            // `\cites(pre)(post)[p1]{a}[p2]{b}`
            for _ in 0..2 {
                self.delimited(b'(', b')', true);
            }
            loop {
                self.optional();
                self.optional();
                let Some(range) = self.group(true) else {
                    break;
                };
                push_keys(&mut self.scan.cites, &self.text[range.clone()], range.start, name);
            }
            return;
        }
        self.optional();
        self.optional();
        let Some(range) = self.group(true) else {
            return;
        };
        let keys = &self.text[range.clone()];
        if name == "nocite" && keys.trim() == "*" {
            self.scan.cite_all = true;
            return;
        }
        push_keys(&mut self.scan.cites, keys, range.start, name);
    }

    fn reference(&mut self, name: &str) {
        self.star();
        self.optional();
        let Some(range) = self.group(true) else {
            return;
        };
        if LIST_REFS.contains(&name) {
            push_keys(&mut self.scan.refs, &self.text[range.clone()], range.start, name);
        } else {
            push_one(&mut self.scan.refs, &self.text[range.clone()], range.start, name);
        }
        if RANGE_REFS.contains(&name)
            && let Some(range) = self.group(true)
        {
            push_one(&mut self.scan.refs, &self.text[range.clone()], range.start, name);
        }
    }
}

/// One label, which may itself contain a comma.
fn push_one(into: &mut Vec<Key>, text: &str, offset: usize, command: &str) {
    let trimmed = text.trim();
    if !trimmed.is_empty() && !trimmed.contains(['#', '\\']) {
        let at = offset + (text.len() - text.trim_start().len());
        into.push(Key { name: trimmed.to_string(), range: at..at + trimmed.len(), command: command.to_string() });
    }
}

/// The comma separated names in `text`, trimmed, with their offsets.
fn pieces(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = 0;
    text.split(',').filter_map(move |piece| {
        let offset = start + (piece.len() - piece.trim_start().len());
        start += piece.len() + 1;
        let piece = piece.trim();
        (!piece.is_empty()).then_some((offset, piece))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(keys: &[Key]) -> Vec<&str> {
        keys.iter().map(|key| key.name.as_str()).collect()
    }

    #[test]
    fn every_kind_of_cite_is_read() {
        let text = r"\cite{a} \cite*{b} \citep[see][p.~3]{c, d} \citet*{e}
\parencite[12]{f} \textcite{g} \Textcite{h} \footcite{i} \autocite[cf.][]{j}
\supercite{k} \smartcite{l} \fullcite{m} \citetitle{n} \citeauthor*{o} \citeyear{p}
\cites(Pre)(Post)[1]{q}[2]{r,s} \autocites{t}{u}
\citestyle{plain} \citetext{not a key}
\nocite{v} \citealp{w,
  x}";
        let scan = scan(text);
        assert_eq!(names(&scan.cites), ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x"]);
        assert!(!scan.cite_all);
        let d = &scan.cites[3];
        assert_eq!((&text[d.range.clone()], d.command.as_str()), ("d", "citep"));
        assert!(super::scan(r"\nocite{*}").cite_all);
    }

    #[test]
    fn comments_verbatim_and_iffalse_hide_commands() {
        let text = r"% \cite{commented}
\% \cite{after-escaped-percent}
\verb|\cite{verb}| \verb*+\cite{star}+
\begin{verbatim}
\cite{verbatim}
\end{verbatim}
\begin{lstlisting}[language=TeX]
\label{listing}
\end{lstlisting}
\iffalse \cite{off} \ifx a b \cite{nested} \fi \cite{still-off} \fi
\cite{visible}";
        let scan = scan(text);
        assert_eq!(names(&scan.cites), ["after-escaped-percent", "visible"]);
        assert!(scan.labels.is_empty());
    }

    #[test]
    fn a_key_being_typed_does_not_swallow_the_file() {
        let scan = scan("\\cite{smi\n\nNext paragraph \\label{x} \\cite{b}");
        assert_eq!(names(&scan.cites), ["b"]);
        assert_eq!(scan.labels.len(), 1);
    }

    #[test]
    fn references_and_their_lists() {
        let text = r"\ref{a} \eqref{b} \cref{c,d} \Cref{ e } \crefrange{f}{g} \autoref{h} \pageref{i} \nameref{j} \hyperref[k]{text} \vref{l} \zcref[S]{m, n} \ref{o,p}";
        let scan = scan(text);
        assert_eq!(names(&scan.refs), ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o,p"]);
    }

    #[test]
    fn labels_know_their_float_and_caption() {
        let text = r"\section{Setup}\label{sec:setup}
\begin{figure}
  \centering
  \label{fig:early}
  \caption{Results of the {GPU} run}
  \label{fig:ok}
\end{figure}
\begin{table}\begin{center}\caption{T}\label{tab:t}\end{center}\end{table}
\begin{equation} x \label{eq:x} \end{equation}
\newcommand{\mylabel}[1]{\label{#1}}";
        let scan = scan(text);
        let labels: Vec<(&str, &str, Option<&str>, bool)> =
            scan.labels.iter().map(|label| (label.name.as_str(), label.env.as_str(), label.caption.as_deref(), label.before_caption)).collect();
        assert_eq!(
            labels,
            [
                ("sec:setup", "section", Some("Setup"), false),
                ("fig:early", "figure", Some("Results of the GPU run"), true),
                ("fig:ok", "figure", Some("Results of the GPU run"), false),
                ("tab:t", "table", Some("T"), false),
                ("eq:x", "equation", None, false),
            ]
        );
    }

    #[test]
    fn includes_bibliographies_and_packages() {
        let text = r"% !TEX root = ../main.tex
\documentclass{article}
\usepackage[backend=biber, style=authoryear]{biblatex}
\usepackage{hyperref,cleveref}
\addbibresource{refs/main.bib}
\addbibresource[location=remote]{https://example.org/x.bib}
\addbibresource[datatype=ris]{x.ris}
\bibliography{refs/a, refs/b}
\input{chapters/intro} \input chapters/plain
\include{chapters/two} \subfile{chapters/three}
\import{parts/}{four} \subimport{five/}{five.tex}
\externaldocument[A-]{appendix}
\begin{refsection}[sec.bib]\end{refsection}";
        let scan = scan(text);
        assert!(scan.document);
        assert_eq!(scan.root.as_deref(), Some("../main.tex"));
        let packages: Vec<&str> = scan.packages.iter().map(|package| package.name.as_str()).collect();
        assert_eq!(packages, ["biblatex", "hyperref", "cleveref"]);
        assert_eq!(scan.packages[0].options, "backend=biber, style=authoryear");
        let bibs: Vec<(&str, BibKind, bool, bool)> = scan.bibliographies.iter().map(|bib| (bib.name.as_str(), bib.kind, bib.remote, bib.foreign)).collect();
        assert_eq!(
            bibs,
            [
                ("refs/main.bib", BibKind::Resource, false, false),
                ("https://example.org/x.bib", BibKind::Resource, true, false),
                ("x.ris", BibKind::Resource, false, true),
                ("refs/a", BibKind::Bibliography, false, false),
                ("refs/b", BibKind::Bibliography, false, false),
                ("sec.bib", BibKind::Resource, false, false),
            ]
        );
        assert_eq!(&text[scan.bibliographies[4].range.clone()], "refs/b");
        let includes: Vec<(&str, &IncludeKind)> = scan.includes.iter().map(|include| (include.path.as_str(), &include.kind)).collect();
        assert_eq!(
            includes,
            [
                ("chapters/intro", &IncludeKind::Input),
                ("chapters/plain", &IncludeKind::Input),
                ("chapters/two", &IncludeKind::Include),
                ("chapters/three", &IncludeKind::Subfile),
                ("four", &IncludeKind::Import { directory: "parts/".into(), relative: false }),
                ("five.tex", &IncludeKind::Import { directory: "five/".into(), relative: true }),
            ]
        );
        assert_eq!(scan.external_documents, [External { name: "appendix".into(), prefix: "A-".into() }]);
    }

    #[test]
    fn subfiles_name_their_main_file_and_bibitems_define_keys() {
        let scan = scan("\\documentclass[../thesis.tex]{subfiles}\n\\begin{thebibliography}{9}\\bibitem[Knuth]{knuth84} Knuth.\\bibitem{lamport}\\end{thebibliography}");
        assert_eq!(scan.root.as_deref(), Some("../thesis.tex"));
        assert_eq!(names(&scan.bibitems), ["knuth84", "lamport"]);
    }

    #[test]
    fn cite_commands_are_told_apart() {
        for name in ["cite", "citep", "Citet", "parencite", "Textcite", "footcitetext", "nocite", "cites", "Parencites", "citeauthor", "citeurl"] {
            assert!(is_cite(name), "{name}");
        }
        for name in ["citestyle", "citetext", "DeclareCiteCommand", "defcitealias", "section"] {
            assert!(!is_cite(name), "{name}");
        }
        assert!(is_multicite("cites") && is_multicite("autocites") && !is_multicite("cite"));
    }
}
