//! Builds made-up projects at the edges of what LaTeX users write: a book of a
//! thousand pages, thousands of references, broken input, odd file names and a
//! build stopped halfway. Every project is generated here in a temporary folder.
//! Never runs by itself, since it needs TeX Live and takes minutes:
//!
//!   cargo test -p build --test corpus -- --ignored --nocapture

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use build::{BuildEvent, BuildManager, BuildOutcome, BuildRequest, Diagnostic, Engine, Severity};
use tempfile::TempDir;

/// A generous limit for one build, so that a hang fails instead of waiting forever.
const BUILD_LIMIT: Duration = Duration::from_secs(600);

struct Project {
    _directory: TempDir,
    /// The temporary folder, which can hold more than the project.
    outer: PathBuf,
    root: PathBuf,
}

impl Project {
    /// `files` are relative to `root`, which is relative to the temporary folder.
    fn new(root: &str, files: &[(&str, String)]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let outer = directory.path().canonicalize().unwrap();
        let root = outer.join(root);
        std::fs::create_dir_all(&root).unwrap();
        let project = Self { _directory: directory, outer, root };
        for (path, text) in files {
            project.write(path, text);
        }
        project
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn request(&self, main: &str, engine: Engine) -> BuildRequest {
        BuildRequest { root: self.root.clone(), main: self.root.join(main), engine, clean: false }
    }

    /// Every file outside the output folders, to show that a build changed none.
    fn sources(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(directory).unwrap().flatten() {
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    if entry.file_name() != ".build" {
                        walk(&path, files);
                    }
                } else {
                    files.insert(path.clone(), std::fs::read(&path).unwrap());
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(&self.outer, &mut files);
        files
    }
}

/// The outcome and the output lines of one build.
fn run(manager: &BuildManager, request: &BuildRequest) -> (BuildOutcome, Vec<String>) {
    let mut events = manager.start(request.clone());
    let started = Instant::now();
    let mut output = Vec::new();
    loop {
        assert!(started.elapsed() < BUILD_LIMIT, "the build of {} hangs", request.main.display());
        match events.blocking_recv() {
            Some(BuildEvent::Finished(outcome)) => return (outcome, output),
            Some(BuildEvent::Output(line)) => output.push(line),
            Some(BuildEvent::Cancelled) | None => panic!("the build stopped"),
            Some(BuildEvent::Started) => {}
        }
    }
}

/// Builds and checks that no source file changed.
fn build(project: &Project, manager: &BuildManager, request: &BuildRequest, label: &str) -> BuildOutcome {
    let before = project.sources();
    let (outcome, _) = run(manager, request);
    report(label, &outcome);
    assert!(project.sources() == before, "{label}: the build changed a source file");
    outcome
}

fn report(label: &str, outcome: &BuildOutcome) {
    let pages = outcome.pdf.as_deref().and_then(pages).map_or("-".to_string(), |pages| pages.to_string());
    println!("{label:<40} {:>7.2} s  success={} pages={pages}", outcome.duration.as_secs_f64(), outcome.success);
    for diagnostic in &outcome.diagnostics {
        println!("    {:?} {:?}:{:?} {}", diagnostic.severity, diagnostic.file, diagnostic.line, diagnostic.message.lines().next().unwrap_or_default());
    }
}

/// The page count from `pdfinfo`, when it is installed.
fn pages(pdf: &Path) -> Option<usize> {
    let output = Command::new("pdfinfo").arg(pdf).output().ok()?;
    String::from_utf8_lossy(&output.stdout).lines().find_map(|line| line.strip_prefix("Pages:")?.trim().parse().ok())
}

fn assert_pages(outcome: &BuildOutcome, at_least: usize) {
    let pdf = outcome.pdf.as_deref().expect("a PDF");
    if let Some(count) = pages(pdf) {
        assert!(count >= at_least, "{count} pages, expected at least {at_least}");
    }
}

fn errors(outcome: &BuildOutcome) -> Vec<&Diagnostic> {
    outcome.diagnostics.iter().filter(|diagnostic| diagnostic.severity == Severity::Error).collect()
}

/// Made-up prose, the same for the same seed.
fn filler(seed: usize, words: usize) -> String {
    const WORDS: &[&str] = &[
        "filler", "words", "stand", "in", "for", "a", "real", "text", "about", "nothing", "much", "while", "lines",
        "break", "and", "pages", "fill", "with", "sentences", "that", "say", "little", "but", "take", "space",
    ];
    let mut text = String::new();
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    for index in 0..words {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        text.push_str(WORDS[(state >> 33) % WORDS.len()]);
        text.push(if index % 17 == 16 { '.' } else { ' ' });
        if index % 17 == 16 {
            text.push(' ');
        }
    }
    text.push_str(".\n");
    text
}

/// A book of `\include`d chapters of `sections` sections, each about a page long.
fn book(chapters: usize, sections: usize) -> Vec<(String, String)> {
    let mut main = String::from("\\documentclass{report}\n\\begin{document}\n\\tableofcontents\n");
    let mut files = Vec::new();
    for chapter in 1..=chapters {
        main.push_str(&format!("\\include{{chapters/chapter{chapter:02}}}\n"));
        let mut text = format!("\\chapter{{Chapter {chapter}}}\\label{{ch:{chapter}}}\n");
        if chapter > 1 {
            text.push_str(&format!("As chapter~\\ref{{ch:{}}} on page~\\pageref{{ch:{}}} said.\n\n", chapter - 1, chapter - 1));
        }
        for section in 0..sections {
            text.push_str(&format!("\\section{{Section {chapter}.{section}}}\n"));
            for paragraph in 0..3 {
                text.push_str(&filler(chapter * 1000 + section * 10 + paragraph, 140));
                text.push('\n');
            }
        }
        files.push((format!("chapters/chapter{chapter:02}.tex"), text));
    }
    main.push_str("\\end{document}\n");
    files.push(("main.tex".to_string(), main));
    files
}

fn as_files(files: &[(String, String)]) -> Vec<(&str, String)> {
    files.iter().map(|(path, text)| (path.as_str(), text.clone())).collect()
}

fn bib(entries: usize) -> String {
    let surnames = [r#"M{\"u}ller"#, "Schmidt", "{van der Berg}", "O'Neil", "Nakamura", r"Dvo{\v r}{\'a}k"];
    let mut text = String::new();
    for index in 0..entries {
        let surname = surnames[index % surnames.len()];
        text.push_str(&format!(
            "@article{{key{index},\n  author = {{{surname}, Alex and Other, Bo}},\n  title = {{Filler Title Number {index}}},\n  journal = {{Journal of Filler}},\n  volume = {},\n  pages = {{1--{}}},\n  year = {},\n}}\n\n",
            index % 50 + 1,
            index % 30 + 2,
            1950 + index % 70
        ));
    }
    text
}

#[test]
#[ignore]
fn a_book_of_forty_included_chapters() {
    let files = book(40, 31);
    let project = Project::new("book", &as_files(&files));
    let manager = BuildManager::new();
    let request = project.request("main.tex", Engine::PdfLatex);
    let clean = build(&project, &manager, &BuildRequest { clean: true, ..request.clone() }, "book: clean");
    assert!(clean.success && errors(&clean).is_empty());
    assert_pages(&clean, 900);
    let unchanged = build(&project, &manager, &request, "book: unchanged");
    assert!(unchanged.success);

    let edited = project.root.join("chapters/chapter20.tex");
    let original = std::fs::read_to_string(&edited).unwrap();
    for index in 1..=2 {
        project.write("chapters/chapter20.tex", &format!("{original}\nAn edit, number {index}.\n"));
        let outcome = build(&project, &manager, &request, &format!("book: edit in chapter 20 ({index})"));
        assert!(outcome.success);
        assert_pages(&outcome, 900);
    }
    std::fs::write(&edited, original).unwrap();
}

#[test]
#[ignore]
fn five_thousand_references_with_bibtex_and_biber() {
    let bibtex = "\\documentclass{article}\n\\begin{document}\nSee \\cite{key1} and \\cite{key4999}.\n\\nocite{*}\n\\bibliographystyle{plain}\n\\bibliography{refs}\n\\end{document}\n";
    let biblatex = "\\documentclass{article}\n\\usepackage[backend=biber]{biblatex}\n\\addbibresource{refs.bib}\n\\begin{document}\nSee \\cite{key1} and \\cite{key4999}.\n\\nocite{*}\n\\printbibliography\n\\end{document}\n";
    for (label, main, marker) in [("bibtex", bibtex, "\\bibitem"), ("biber", biblatex, "\\entry{")] {
        let project = Project::new("refs", &[("main.tex", main.to_string()), ("refs.bib", bib(5000))]);
        let manager = BuildManager::new();
        let outcome = build(&project, &manager, &project.request("main.tex", Engine::PdfLatex), &format!("5000 references: {label}"));
        assert!(outcome.success, "{label}");
        assert_pages(&outcome, 100);
        let bbl = std::fs::read_to_string(project.root.join(".build/main.bbl")).unwrap();
        assert_eq!(bbl.matches(marker).count(), 5000, "{label}");
        let again = build(&project, &manager, &project.request("main.tex", Engine::PdfLatex), &format!("5000 references: {label}, again"));
        assert!(again.success);
    }
}

#[test]
#[ignore]
fn broken_bibliography_entries_warn_and_the_pdf_is_complete() {
    let bib = "@article{good, author = {Filler, Alex}, title = {Fine}, journal = {Journal of Filler}, year = 2001}\n\
               @article{nojournal, author = {Filler, Bo}, title = {No Journal}, year = 2002}\n\
               @article{nocomma, author = {Filler, Cy} title = {Missing Comma}, journal = {J}, year = 2003}\n\
               @book{after, author = {Filler, Di}, title = {After the Broken Ones}, publisher = {Filler Press}, year = 2004}\n";
    let main = "\\documentclass{article}\n\\begin{document}\n\\cite{good} \\cite{nojournal} \\cite{nocomma} \\cite{after}\n\\bibliographystyle{plain}\n\\bibliography{refs}\n\\end{document}\n";
    let project = Project::new("broken", &[("main.tex", main.to_string()), ("refs.bib", bib.to_string())]);
    let manager = BuildManager::new();
    let outcome = build(&project, &manager, &project.request("main.tex", Engine::PdfLatex), "broken .bib entries");
    assert!(outcome.success);
    assert_pages(&outcome, 1);
    assert!(outcome.diagnostics.iter().any(|diagnostic| diagnostic.severity == Severity::Warning && diagnostic.message.contains("BibTeX")), "a warning about the bibliography");
    let bbl = std::fs::read_to_string(project.root.join(".build/main.bbl")).unwrap();
    assert!(bbl.contains("After the Broken Ones"));
}

#[test]
#[ignore]
fn refsections_glossaries_index_and_nomenclature() {
    let main = "\\documentclass{article}\n\
                \\usepackage[backend=biber]{biblatex}\n\
                \\addbibresource{refs.bib}\n\
                \\usepackage{makeidx}\n\\makeindex\n\
                \\usepackage{glossaries}\n\\makeglossaries\n\
                \\usepackage{nomencl}\n\\makenomenclature\n\
                \\newglossaryentry{filler}{name=filler,description={made-up text}}\n\
                \\begin{document}\n\
                \\begin{refsection}\nFirst part \\cite{key1}\\index{alpha} \\gls{filler}\\nomenclature{$x$}{a variable}.\n\\printbibliography[heading=subbibliography]\n\\end{refsection}\n\
                \\begin{refsection}\nSecond part \\cite{key2}\\index{beta}.\n\\printbibliography[heading=subbibliography]\n\\end{refsection}\n\
                \\printglossaries\n\\printnomenclature\n\\printindex\n\
                \\end{document}\n";
    let project = Project::new("parts", &[("main.tex", main.to_string()), ("refs.bib", bib(3))]);
    let manager = BuildManager::new();
    let outcome = build(&project, &manager, &project.request("main.tex", Engine::PdfLatex), "refsection, glossaries, index, nomencl");
    assert!(outcome.success);
    let output = project.root.join(".build");
    let bbl = std::fs::read_to_string(output.join("main.bbl")).unwrap();
    assert!(bbl.contains("\\refsection{1}") && bbl.contains("\\refsection{2}"), "two reference sections");
    for (extension, tool) in [("ind", "makeindex"), ("gls", "makeglossaries"), ("nls", "makeindex for nomencl")] {
        assert!(output.join(format!("main.{extension}")).is_file(), "{tool} did not run");
    }
}

#[test]
#[ignore]
fn names_with_spaces_umlauts_and_signs() {
    let main = "\\documentclass{article}\n\\begin{document}\n\\section{Start}\\label{start}\n\\input{kapitel über alles}\n\\end{document}\n";
    let chapter = "Ein Kapitel, siehe Abschnitt~\\ref{start}.\n";
    let project = Project::new("Arbeit äöü #1 & mehr", &[("Haupt datei ä.tex", main.to_string()), ("kapitel über alles.tex", chapter.to_string())]);
    let manager = BuildManager::new();
    let request = project.request("Haupt datei ä.tex", Engine::PdfLatex);
    let outcome = build(&project, &manager, &request, "names with spaces, umlauts, # and &");
    assert!(outcome.success);
    assert_pages(&outcome, 1);
    assert!(outcome.synctex.is_some());
    // An error in the chapter points at it, by its odd name.
    project.write("kapitel über alles.tex", "Ein Kapitel.\n\\undefinedfillercommand\n");
    let outcome = build(&project, &manager, &request, "an error in a file with an odd name");
    let error = errors(&outcome).into_iter().next().expect("an error");
    assert_eq!((error.file.as_deref(), error.line), (Some(project.root.join("kapitel über alles.tex").as_path()), Some(2)));
}

#[test]
#[ignore]
fn missing_files_and_references_are_precise_and_keep_the_pdf() {
    let good = "\\documentclass{article}\n\\usepackage{graphicx}\n\\begin{document}\nFine.\n\\end{document}\n";
    let project = Project::new("missing", &[("main.tex", good.to_string())]);
    let manager = BuildManager::new();
    let request = project.request("main.tex", Engine::PdfLatex);
    assert!(build(&project, &manager, &request, "missing: good first").success);
    let main = project.root.join("main.tex");
    let still_there = |outcome: &BuildOutcome| {
        let pdf = outcome.pdf.clone().or(outcome.last_good_pdf.clone()).expect("a PDF to show");
        assert!(pdf.is_file());
    };

    project.write("main.tex", "\\documentclass{article}\n\\usepackage{graphicx}\n\\begin{document}\nFine.\n\\includegraphics{missing-figure}\n\\end{document}\n");
    let outcome = build(&project, &manager, &request, "missing image");
    assert!(!outcome.success);
    let error = errors(&outcome).into_iter().next().expect("an error");
    assert!(error.message.contains("missing-figure"), "{}", error.message);
    assert_eq!((error.file.as_deref(), error.line), (Some(main.as_path()), Some(5)));
    still_there(&outcome);

    project.write("main.tex", "\\documentclass{article}\n\\begin{document}\nSee \\cite{nowhere}.\n\\bibliographystyle{plain}\n\\bibliography{not-there}\n\\end{document}\n");
    let outcome = build(&project, &manager, &request, "missing .bib");
    still_there(&outcome);
    assert!(outcome.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("nowhere")), "the missing citation is named");

    project.write("main.tex", "\\documentclass{article}\n\\begin{document}\nFine.\n\nSee section~\\ref{sec:nowhere}.\n\\end{document}\n");
    let outcome = build(&project, &manager, &request, "undefined reference");
    assert!(outcome.success);
    let warning = outcome.diagnostics.iter().find(|diagnostic| diagnostic.message.contains("sec:nowhere")).expect("a warning about the reference");
    assert_eq!((warning.severity, warning.file.as_deref(), warning.line), (Severity::Warning, Some(main.as_path()), Some(5)));
}

#[test]
#[ignore]
fn a_syntax_error_in_chapter_thirty_points_there() {
    let mut files = book(40, 1);
    let chapter = files.iter_mut().find(|(path, _)| path == "chapters/chapter30.tex").unwrap();
    let mut lines: Vec<&str> = chapter.1.lines().collect();
    lines.insert(4, "Here \\undefinedfillercommand{} breaks.");
    chapter.1 = lines.join("\n");
    let project = Project::new("error", &as_files(&files));
    let manager = BuildManager::new();
    let request = project.request("main.tex", Engine::PdfLatex);
    for label in ["error in chapter 30", "error in chapter 30, again"] {
        let outcome = build(&project, &manager, &request, label);
        assert!(!outcome.success);
        let error = errors(&outcome).into_iter().next().expect("an error");
        assert_eq!((error.file.as_deref(), error.line), (Some(project.root.join("chapters/chapter30.tex").as_path()), Some(5)), "{}", error.message);
    }
}

#[test]
#[ignore]
fn a_label_that_never_settles_stops() {
    // Each run gives the label the number of the run before, so LaTeX always asks for one more.
    let main = "\\documentclass{article}\n\\makeatletter\n\\providecommand\\fillerruns{0}\n\
                \\AtEndDocument{\\immediate\\write\\@auxout{\\gdef\\string\\fillerruns{\\the\\numexpr\\fillerruns+1\\relax}}}\n\
                \\begin{document}\n\\def\\@currentlabel{\\fillerruns}\\label{moving}\nRun \\ref{moving}.\n\\end{document}\n";
    let project = Project::new("moving", &[("main.tex", main.to_string())]);
    let manager = BuildManager::new();
    let request = project.request("main.tex", Engine::PdfLatex);
    // latexmk stops after five runs, the editor's own reruns after three.
    for (label, most) in [("unsettled label: first", 5), ("unsettled label: second", 3), ("unsettled label: third", 3)] {
        let before = project.sources();
        let (outcome, output) = run(&manager, &request);
        report(label, &outcome);
        assert!(project.sources() == before);
        let runs = output.iter().filter(|line| line.starts_with("This is pdfTeX")).count();
        println!("    {runs} runs of pdfTeX");
        assert!(outcome.pdf.is_some());
        assert!(runs <= most, "{runs} runs");
        assert!(outcome.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("Rerun")), "the label is still moving");
        // The label keeps its number. A format, which ends with \makeatother, once lost it.
        let aux = std::fs::read_to_string(project.root.join(".build/main.aux")).unwrap();
        assert!(!aux.contains("\\newlabel{moving}{{}"), "{aux}");
    }
}

#[test]
#[ignore]
fn subfiles_and_input_from_outside_the_root() {
    let main = "\\documentclass{article}\n\\usepackage{subfiles}\n\\begin{document}\n\\input{../shared/common}\n\\subfile{chapters/one}\n\\end{document}\n";
    let one = "\\documentclass[../main.tex]{subfiles}\n\\begin{document}\n\\section{One}\nThe chapter, \\input{\\subfix{../../shared/common}}\n\\end{document}\n";
    let project = Project::new("project", &[("main.tex", main.to_string()), ("chapters/one.tex", one.to_string())]);
    std::fs::create_dir_all(project.outer.join("shared")).unwrap();
    std::fs::write(project.outer.join("shared/common.tex"), "Shared filler text.\n").unwrap();
    let manager = BuildManager::new();
    let whole = build(&project, &manager, &project.request("main.tex", Engine::PdfLatex), "subfiles: whole");
    assert!(whole.success && errors(&whole).is_empty());
    let alone = build(&project, &manager, &project.request("chapters/one.tex", Engine::PdfLatex), "subfiles: chapter alone");
    assert!(alone.success && errors(&alone).is_empty());
    assert_eq!(alone.pdf.as_deref(), Some(project.root.join("chapters/.build/one.pdf").as_path()));
}

#[test]
#[ignore]
fn lualatex_with_fontspec_and_a_tikz_chapter() {
    let main = "\\documentclass{report}\n\\usepackage{fontspec}\n\\setmainfont{TeX Gyre Pagella}\n\\usepackage{tikz}\n\\begin{document}\n\\chapter{Text}\nÄußerst grüne Füller: ſ, ﬁ, € and “quotes”.\n\\include{tikz}\n\\end{document}\n";
    let mut tikz = String::from("\\chapter{Pictures}\n");
    for picture in 0..150 {
        tikz.push_str(&format!(
            "\\begin{{tikzpicture}}\n\\foreach \\x in {{0,...,40}} {{ \\draw[blue!{}] (\\x/8,0) -- ({{\\x/8}},{{sin(\\x*{} r)}}) node[circle,fill,inner sep=0.5pt] {{}}; }}\n\\node at (2,1.5) {{Picture {picture}}};\n\\end{{tikzpicture}}\n\n",
            picture % 80 + 20,
            picture % 7 + 1
        ));
    }
    let project = Project::new("lua", &[("main.tex", main.to_string()), ("tikz.tex", tikz)]);
    let manager = BuildManager::new();
    let request = project.request("main.tex", Engine::LuaLatex);
    let outcome = build(&project, &manager, &request, "lualatex + fontspec + tikz");
    assert!(outcome.success && errors(&outcome).is_empty());
    assert_pages(&outcome, 20);
    let again = build(&project, &manager, &request, "lualatex + fontspec + tikz, again");
    assert!(again.success);
}

#[test]
#[ignore]
fn a_cancelled_build_leaves_nothing_behind() {
    // A name no other process has, to look for leftovers by.
    let stem = format!("cancelcorpus{}", std::process::id());
    let slow = "\\documentclass{article}\n\\begin{document}\n\\newcount\\fillercount\n\\loop\\advance\\fillercount by 1 \\ifnum\\fillercount<400000000 \\repeat\nDone.\n\\end{document}\n";
    let main = format!("{stem}.tex");
    let project = Project::new("cancel", &[(main.as_str(), slow.to_string())]);
    let manager = BuildManager::new();
    let request = project.request(&main, Engine::PdfLatex);
    let leftovers = || {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("powershell");
            command.args(["-NoProfile", "-Command", &format!("Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -like '*{stem}*' -and $_.Name -ne 'powershell.exe' }} | ForEach-Object {{ $_.ProcessId }}")]);
            command
        } else {
            let mut command = Command::new("pgrep");
            command.args(["-f", &stem]);
            command
        };
        command.output().map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string()).unwrap_or_default()
    };

    let mut events = manager.start(request.clone());
    loop {
        match events.blocking_recv() {
            Some(BuildEvent::Output(_)) => break,
            Some(BuildEvent::Started) => {}
            other => panic!("{other:?} before any output"),
        }
    }
    std::thread::sleep(Duration::from_secs(2));
    assert!(!leftovers().is_empty(), "the compiler runs");
    let cancelled_at = Instant::now();
    manager.cancel();
    loop {
        match events.blocking_recv() {
            Some(BuildEvent::Cancelled) => break,
            Some(BuildEvent::Output(_)) => {}
            other => panic!("{other:?} instead of the cancellation"),
        }
    }
    println!("cancelled after {:.2} s", cancelled_at.elapsed().as_secs_f64());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(leftovers(), "", "processes left behind");

    project.write(&main, "\\documentclass{article}\n\\begin{document}\nQuick.\n\\end{document}\n");
    let outcome = build(&project, &manager, &request, "after a cancelled build");
    assert!(outcome.success && errors(&outcome).is_empty());
    assert_pages(&outcome, 1);
}
