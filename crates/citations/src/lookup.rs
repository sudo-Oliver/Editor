//! What the name under the pointer or the cursor is, and where it is defined:
//! for hovering over `\cite{key}` or `\ref{label}` and jumping to the entry
//! or the label.

use std::{ops::Range, path::PathBuf};

use crate::{
    index::{CitationIndex, IndexedEntry, LabelDef},
    project::Language,
    markdown, tex, typst,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    /// A key in a citation.
    Cite,
    /// A label in a reference.
    Ref,
    /// Typst's `@name`, which is either.
    Either,
    /// The name in `\label{}` or `<label>`.
    Label,
}

/// A name in a piece of text, with its range there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub kind: TargetKind,
    pub name: String,
    pub range: Range<usize>,
}

/// The key or label at `offset` of `text`, which is a line or a few, read
/// as the scanners read whole files.
pub fn target_at(language: Language, text: &str, offset: usize) -> Option<Target> {
    let scan = match language {
        Language::Latex => tex::scan(text),
        Language::Typst => typst::scan(text),
        Language::Markdown => markdown::scan(text),
    };
    let found = |kind: TargetKind| move |key: &crate::scan::Key| (kind, key.name.clone(), key.range.clone());
    scan.cites
        .iter()
        .map(found(TargetKind::Cite))
        .chain(scan.refs.iter().map(found(TargetKind::Ref)))
        .chain(scan.either.iter().map(found(TargetKind::Either)))
        .chain(scan.labels.iter().map(|label| (TargetKind::Label, label.name.clone(), label.range.clone())))
        .find(|(_, _, range)| range.start <= offset && offset <= range.end)
        .map(|(kind, name, range)| Target { kind, name, range })
}

/// What a name stands for in the document.
pub enum Definition<'a> {
    Entry(&'a IndexedEntry),
    Label(&'a [LabelDef]),
}

impl Definition<'_> {
    /// The file and byte range to jump to.
    pub fn place(&self) -> Option<(PathBuf, Range<usize>, usize)> {
        match self {
            Definition::Entry(entry) => Some((entry.path.to_path_buf(), entry.key_range.clone(), entry.line)),
            Definition::Label(definitions) => definitions.first().map(|definition| (definition.path.to_path_buf(), definition.label.range.clone(), definition.label.line)),
        }
    }
}

impl CitationIndex {
    /// The entry or the label a target names. In Typst a label wins over
    /// an entry of the same name, as it does when Typst compiles.
    pub fn definition(&self, target: &Target) -> Option<Definition<'_>> {
        let label = || Some(self.label(&target.name)).filter(|definitions| !definitions.is_empty()).map(Definition::Label);
        let entry = || self.entry(&target.name).map(Definition::Entry);
        match target.kind {
            TargetKind::Cite => entry(),
            TargetKind::Ref | TargetKind::Label => label(),
            TargetKind::Either => label().or_else(entry),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_under_the_pointer() {
        let line = r"See \citep[p.~3]{smith2020, vaswani2017} and \cref{fig:a} \label{sec:x}";
        let at = |needle: &str| target_at(Language::Latex, line, line.find(needle).unwrap() + 1);
        assert_eq!(at("vaswani").map(|target| (target.kind, target.name)), Some((TargetKind::Cite, "vaswani2017".into())));
        assert_eq!(at("fig:a").map(|target| (target.kind, target.name)), Some((TargetKind::Ref, "fig:a".into())));
        assert_eq!(at("sec:x").map(|target| (target.kind, target.name)), Some((TargetKind::Label, "sec:x".into())));
        assert_eq!(at("See"), None);
        let typst = "As @knuth84 shows in <intro>.";
        assert_eq!(target_at(Language::Typst, typst, 6).map(|target| (target.kind, target.range)), Some((TargetKind::Either, 4..11)));
        assert_eq!(target_at(Language::Typst, typst, 23).map(|target| target.kind), Some(TargetKind::Label));
    }
}
