//! Checks text and links against real pdfLaTeX output, because hyperref writes the
//! links that matter most. Skipped when pdflatex or PDFium is missing.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use pdf::{LinkTarget, PageContent, PdfEvent, PdfWorker, Rect};

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

/// Builds a document, or `None` when the tools for this test are missing.
fn compile(document: &str) -> Option<Vec<u8>> {
    let pdflatex = pdflatex()?;
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("main.tex"), document).unwrap();
    // The second run resolves the references.
    for _ in 0..2 {
        let output = Command::new(pdflatex)
            .args(["-interaction=nonstopmode", "main.tex"])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    }
    Some(std::fs::read(directory.path().join("main.pdf")).unwrap())
}

fn library() -> Option<PathBuf> {
    Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib")).filter(|library| library.is_dir())
}

fn first_h(content: &PageContent) -> Rect {
    content.chars.iter().find(|character| character.ch == 'H').unwrap().rect
}

fn assert_close(left: f32, right: f32) {
    assert!((left - right).abs() < 0.5, "{left} is not {right}");
}

/// Marks page geometry, and leaves the rest of `DOCUMENT` the same.
fn with_page_attributes(attributes: &str) -> String {
    DOCUMENT.replace(r"\begin{document}", &format!("\\pdfpageattr{{{attributes}}}\n\\begin{{document}}"))
}

#[test]
fn a_crop_box_moves_the_origin() {
    let (Some(library), Some(plain)) = (library(), compile(DOCUMENT)) else {
        return;
    };
    let cropped = compile(&with_page_attributes("/CropBox [50 60 500 700]")).unwrap();
    let (plain_pages, plain) = content_of(plain, &library, 0);
    let (pages, cropped) = content_of(cropped, &library, 0);
    assert_close(pages[0].0, 450.0);
    assert_close(pages[0].1, 640.0);
    // The top of the crop box is 700 points up, the page ends at `plain_pages[0].1`.
    let (before, after) = (first_h(&plain), first_h(&cropped));
    assert_close(after.left, before.left - 50.0);
    assert_close(after.top, before.top - (plain_pages[0].1 - 700.0));
    let (before, after) = (plain.links[0].rect, cropped.links[0].rect);
    assert_close(after.left, before.left - 50.0);
    assert_close(after.top, before.top - (plain_pages[0].1 - 700.0));
}

#[test]
fn a_rotated_page_keeps_positions_on_screen() {
    let (Some(library), Some(plain)) = (library(), compile(DOCUMENT)) else {
        return;
    };
    // pdflscape marks landscape pages like this.
    let rotated = compile(&with_page_attributes("/Rotate 90")).unwrap();
    let (plain_pages, plain) = content_of(plain, &library, 0);
    let (pages, rotated) = content_of(rotated, &library, 0);
    let (width, height) = plain_pages[0];
    assert_close(pages[0].0, height);
    assert_close(pages[0].1, width);
    // Turning clockwise moves the top left corner of the page to the top right.
    let (before, after) = (first_h(&plain), first_h(&rotated));
    assert_close(after.left, height - before.bottom);
    assert_close(after.right, height - before.top);
    assert_close(after.top, before.left);
    assert_close(after.bottom, before.right);
    let (before, after) = (plain.links[0].rect, rotated.links[0].rect);
    assert_close(after.left, height - before.bottom);
    assert_close(after.top, before.left);
    // A target stays where the rotated page shows it.
    let target = |content: &PageContent, page| {
        content.links.iter().find_map(|link| match link.target {
            LinkTarget::Page { page: found, y } if found == page => y,
            _ => None,
        })
    };
    let y = target(&rotated, 1).expect("target with a position");
    assert!(y >= 0.0 && y <= width, "{y}");
    assert!(target(&plain, 1).is_some());
}

#[test]
#[ignore = "prints a timing, run with --ignored --nocapture --release"]
fn extraction_timing_on_a_long_document() {
    let Some(library) = library() else {
        panic!("PDFium is missing");
    };
    let sentence = "The quick brown fox jumps over the lazy dog, again and again, until the page is full. ";
    let body = format!("{}\n\n", sentence.repeat(400));
    let document = format!("\\documentclass{{article}}\n\\begin{{document}}\n{}\\end{{document}}\n", body.repeat(40));
    let bytes = compile(&document).expect("pdflatex is missing");
    let (worker, mut events) = PdfWorker::spawn(&library).unwrap();
    worker.load(1, bytes);
    let PdfEvent::Loaded { pages, .. } = events.blocking_recv().unwrap() else {
        panic!("not loaded");
    };
    let start = Instant::now();
    let mut characters = 0;
    for page in 0..pages.len() {
        worker.content(1, page);
        let PdfEvent::PageContent { content, .. } = events.blocking_recv().unwrap() else {
            panic!("no content");
        };
        characters += content.chars.len();
    }
    println!(
        "{} pages: {:?} per page, {} characters per page",
        pages.len(),
        start.elapsed() / pages.len() as u32,
        characters / pages.len(),
    );
}

#[test]
fn reads_text_and_hyperref_links() {
    let (Some(library), Some(bytes)) = (library(), compile(DOCUMENT)) else {
        return;
    };
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
