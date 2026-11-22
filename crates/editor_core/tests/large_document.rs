//! Times the editor core on a very large document, such as a dissertation. Never
//! runs by itself. The generated documents need nothing else:
//!
//!   cargo test --release -p editor_core --test large_document -- --ignored --nocapture
//!
//! A real project can stand in, whose `.tex` files are joined and repeated in
//! memory. Nothing on disk changes:
//!
//!   EDITOR_BENCH_PROJECT=/path/to/project EDITOR_BENCH_REPEAT=10 \
//!     cargo test --release -p editor_core --test large_document -- --ignored --nocapture

use std::{
    fmt::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use editor_core::{EditorState, Language, ParseTimings, Selection, word_count::count_words};

fn tex_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        let hidden = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if path.is_dir() && !hidden {
            tex_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "tex") {
            files.push(path);
        }
    }
}

/// Filler words, picked by a fixed sequence so every run times the same text.
struct Filler(u64);

impl Filler {
    const WORDS: &[&str] = &[
        "lorem", "ipsum", "dolor", "sit", "amet", "consectetur", "adipiscing", "elit", "sed", "do", "eiusmod",
        "tempor", "incididunt", "ut", "labore", "et", "dolore", "magna", "aliqua", "enim", "ad", "minim", "veniam",
        "quis", "nostrud", "exercitation", "ullamco", "laboris", "nisi", "aliquip", "ex", "ea", "commodo",
    ];

    fn next(&mut self, below: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize % below
    }

    fn words(&mut self, count: usize) -> String {
        let words: Vec<_> = (0..count).map(|_| Self::WORDS[self.next(Self::WORDS.len())]).collect();
        words.join(" ")
    }

    fn paragraph(&mut self, out: &mut String, label: &str) {
        for sentence in 0..4 + self.next(5) {
            let count = 8 + self.next(12);
            out.push_str(&self.words(count));
            match self.next(8) {
                0 => _ = write!(out, " $x_{{{sentence}}}^2 + y$"),
                1 => _ = write!(out, " \\emph{{{}}}", self.words(2)),
                2 => _ = write!(out, " see~\\ref{{{label}}}"),
                3 => _ = write!(out, " \\cite{{key{sentence}}}"),
                _ => {}
            }
            out.push_str(".\n");
        }
        out.push('\n');
    }
}

/// A LaTeX document of about `size` bytes, built like a thesis from filler text:
/// chapters, sections, labels, math, figures and tables.
fn generated_latex(size: usize) -> String {
    let mut filler = Filler(1);
    let mut out = String::from(
        "\\documentclass{book}\n\\usepackage{amsmath, graphicx}\n\\usepackage[round]{natbib}\n\\newcommand{\\vect}[1]{\\mathbf{#1}}\n\\begin{document}\n",
    );
    let mut chapter = 0;
    while out.len() < size {
        chapter += 1;
        _ = writeln!(out, "\\chapter{{{}}}\\label{{chap:{chapter}}}\n", filler.words(3));
        for section in 1..=8 {
            let label = format!("sec:{chapter}.{section}");
            _ = writeln!(out, "\\section{{{}}}\\label{{{label}}}\n", filler.words(4));
            for subsection in 1..=3 {
                _ = writeln!(out, "\\subsection*{{{}}}\n", filler.words(2));
                for _ in 0..6 {
                    filler.paragraph(&mut out, &label);
                }
                match subsection {
                    1 => _ = writeln!(
                        out,
                        "\\begin{{figure}}[ht]\n\\centering\n\\includegraphics{{figure}}\n\\caption{{{}}}\\label{{fig:{chapter}.{section}}}\n\\end{{figure}}\n",
                        filler.words(5)
                    ),
                    2 => _ = writeln!(
                        out,
                        "\\begin{{table}}[ht]\n\\caption{{{}}}\n\\begin{{tabular}}{{ll}}\na & b \\\\\nc & d\n\\end{{tabular}}\n\\end{{table}}\n",
                        filler.words(4)
                    ),
                    _ => _ = writeln!(
                        out,
                        "\\begin{{equation}}\\label{{eq:{chapter}.{section}}}\n\\vect{{a}} = \\sum_{{i=0}}^{{n}} x_i\n\\end{{equation}}\n"
                    ),
                }
            }
        }
    }
    out.push_str("\\printbibliography\n\\end{document}\n");
    out
}

fn time<T>(label: &str, work: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = work();
    let elapsed = started.elapsed();
    println!("{label:<36} {:>9.2} ms", elapsed.as_secs_f64() * 1000.0);
    (value, elapsed)
}

fn parse(state: &mut EditorState) -> ParseTimings {
    let Some(job) = state.start_parse() else {
        return ParseTimings::default();
    };
    let result = job.run();
    let timings = result.timings();
    state.finish_parse(result);
    timings
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn edit_large_document(text: &str, word_count_root: Option<&Path>) {
    println!("{:.1} MB", text.len() as f64 / 1_000_000.0);
    let (mut state, _) = time("open", || EditorState::new(text, Language::Latex));
    println!("{} lines", state.buffer().line_count());
    let (first, _) = time("first parse", || parse(&mut state));
    println!(
        "{:<36} {:>9.2} ms parse, {:.2} ms outline",
        "",
        milliseconds(first.parse),
        milliseconds(first.outline)
    );
    println!("{} headings, {} symbols", state.syntax().headings().len(), state.syntax().symbols().len());

    // Typing in the middle, with a parse after every key, as the editor does.
    let middle = state.buffer().line_start(state.buffer().line_count() / 2);
    state.set_selections(vec![Selection::cursor(middle)]);
    let mut slowest = Duration::ZERO;
    let (mut parsing, mut outline) = (Duration::ZERO, Duration::ZERO);
    let keys = 100;
    let (_, typing) = time("100 keys, each parsed", || {
        for character in "The quick brown fox jumps over the lazy dog. ".chars().cycle().take(keys) {
            let started = Instant::now();
            state.type_text(&character.to_string());
            let timings = parse(&mut state);
            parsing += timings.parse;
            outline += timings.outline;
            slowest = slowest.max(started.elapsed());
        }
    });
    let average = |duration: Duration| milliseconds(duration) / keys as f64;
    println!("{:<36} {:>9.2} ms (average {:.2} ms)", "slowest key", milliseconds(slowest), average(typing));
    println!("{:<36} {:>9.2} ms per key", "  of that parse", average(parsing));
    println!("{:<36} {:>9.2} ms per key", "  of that outline", average(outline));

    let visible = middle..state.buffer().line_end(state.buffer().line_count() / 2 + 60);
    time("highlights for 60 visible lines", || state.syntax().highlights(state.buffer().rope(), visible.clone()).len());
    let split = |(timings, _): (ParseTimings, Duration)| {
        println!("{:<36} {:>9.2} ms parse, {:.2} ms outline", "", milliseconds(timings.parse), milliseconds(timings.outline));
    };
    split(time("paste 100 KB", || {
        state.paste(&text[..100_000.min(text.len())]);
        parse(&mut state)
    }));
    split(time("undo the paste", || {
        state.undo();
        parse(&mut state)
    }));
    split(time("select all, delete", || {
        state.select_all();
        state.backspace();
        parse(&mut state)
    }));
    split(time("undo the delete", || {
        state.undo();
        parse(&mut state)
    }));
    assert_eq!(state.buffer().len(), text.len() + keys, "undo brings the whole text back");
    if let Some(root) = word_count_root {
        let main = root.join("main.tex");
        time("word count", || count_words(&main, text, root, &mut |_| None).total());
    }
}

#[test]
#[ignore]
fn editing_a_generated_document() {
    for size in [2_000_000, 10_000_000] {
        edit_large_document(&generated_latex(size), None);
        println!();
    }
}

#[test]
#[ignore]
fn editing_a_large_document() {
    let Some(root) = std::env::var_os("EDITOR_BENCH_PROJECT").map(PathBuf::from) else {
        eprintln!("EDITOR_BENCH_PROJECT is not set");
        return;
    };
    let repeat: usize = std::env::var("EDITOR_BENCH_REPEAT").ok().and_then(|value| value.parse().ok()).unwrap_or(10);
    let mut files = Vec::new();
    tex_files(&root, &mut files);
    let body: String = files.iter().filter_map(|path| std::fs::read_to_string(path).ok()).collect::<Vec<_>>().join("\n");
    println!("{} files, {repeat}x", files.len());
    edit_large_document(&body.repeat(repeat), Some(&root));
}
