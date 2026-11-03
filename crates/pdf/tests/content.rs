//! Checks text and links against real pdfLaTeX output, because hyperref writes the
//! links that matter most. Skipped when pdflatex or PDFium is missing.

use std::{path::Path, process::Command};

use pdf::{LinkTarget, PageContent, PdfEvent, PdfWorker};

const DOCUMENT: &str = r"\documentclass{article}
\usepackage{hyperref}
\begin{document}
Hello selectable world, see Figure~\ref{fig} and \cite{book} and \href{https://example.org/}{the web}.
\newpage
\begin{figure}[h]\caption{A figure}\label{fig}\end{figure}
\newpage
\begin{thebibliography}{9}\bibitem{book} A book.\end{thebibliography}
\end{document}
";

fn pdflatex() -> Option<&'static str> {
    ["pdflatex", "/Library/TeX/texbin/pdflatex"]
        .into_iter()
        .find(|program| Command::new(program).arg("--version").output().is_ok())
}

fn content_of(bytes: Vec<u8>, library: &Path, page: usize) -> (Vec<(f32, f32)>, PageContent) {
    let (worker, mut events) = PdfWorker::spawn(library).unwrap();
    worker.load(1, bytes);
    let pages = match events.blocking_recv().unwrap() {
        PdfEvent::Loaded { pages, .. } => pages,
        other => panic!("{other:?}"),
    };
    worker.content(1, page);
    match events.blocking_recv().unwrap() {
        PdfEvent::PageContent { content, .. } => (pages, content),
        other => panic!("{other:?}"),
    }
}

#[test]
fn reads_text_and_hyperref_links() {
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib");
    let (Some(pdflatex), true) = (pdflatex(), library.is_dir()) else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("main.tex"), DOCUMENT).unwrap();
    // The second run resolves the references.
    for _ in 0..2 {
        let output = Command::new(pdflatex)
            .args(["-interaction=nonstopmode", "main.tex"])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    }
    let bytes = std::fs::read(directory.path().join("main.pdf")).unwrap();
    let (pages, content) = content_of(bytes, &library, 0);
    let (width, height) = pages[0];
    assert_eq!(pages.len(), 3);

    let text: String = content.chars.iter().map(|character| character.ch).collect();
    assert!(text.contains("Hello selectable world"), "{text}");

    // Text boxes must be inside the page, with the origin at the top left.
    let hello = content.chars.iter().find(|character| character.ch == 'H').unwrap().rect;
    assert!(hello.left >= 0.0 && hello.right <= width);
    assert!(hello.top > 0.0 && hello.top < hello.bottom && hello.bottom < height / 2.0, "{hello:?}");

    let target_page = |page| {
        content.links.iter().any(|link| matches!(link.target, LinkTarget::Page { page: found, .. } if found == page))
    };
    assert!(target_page(1), "no link to the figure: {:?}", content.links);
    assert!(target_page(2), "no link to the bibliography: {:?}", content.links);
    let uri = LinkTarget::Uri("https://example.org/".into());
    assert!(content.links.iter().any(|link| link.target == uri), "{:?}", content.links);

    // The link sits on the first text line, and its target is a position on the page.
    let link = content.links.iter().find(|link| link.target != uri).unwrap();
    assert!((link.rect.top - hello.top).abs() < 20.0, "{:?} vs {hello:?}", link.rect);
    for link in &content.links {
        if let LinkTarget::Page { y: Some(y), .. } = link.target {
            assert!(y >= 0.0 && y <= height, "{y}");
        }
    }
}
