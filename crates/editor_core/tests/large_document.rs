//! Times the editor core on a very large document, such as a dissertation. Never
//! runs by itself: point it at a project, whose `.tex` files are joined and
//! repeated in memory. Nothing on disk changes.
//!
//!   EDITOR_BENCH_PROJECT=/path/to/project EDITOR_BENCH_REPEAT=10 \
//!     cargo test --release -p editor_core --test large_document -- --ignored --nocapture

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use editor_core::{EditorState, Language, Selection, word_count::count_words};

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

fn time<T>(label: &str, work: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = work();
    let elapsed = started.elapsed();
    println!("{label:<36} {:>9.2} ms", elapsed.as_secs_f64() * 1000.0);
    (value, elapsed)
}

fn parse(state: &mut EditorState) {
    if let Some(job) = state.start_parse() {
        let result = job.run();
        state.finish_parse(result);
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
    let text = body.repeat(repeat);
    println!("{} files, {repeat}x, {:.1} MB", files.len(), text.len() as f64 / 1_000_000.0);

    let (mut state, _) = time("open", || EditorState::new(&text, Language::Latex));
    println!("{} lines", state.buffer().line_count());
    time("first parse", || parse(&mut state));
    println!("{} headings, {} symbols", state.syntax().headings().len(), state.syntax().symbols().len());

    // Typing in the middle, with a parse after every key, as the editor does.
    let middle = state.buffer().line_start(state.buffer().line_count() / 2);
    state.set_selections(vec![Selection::cursor(middle)]);
    let mut slowest = Duration::ZERO;
    let (_, typing) = time("100 keys, each parsed", || {
        for character in "The quick brown fox jumps over the lazy dog. ".chars().cycle().take(100) {
            let started = Instant::now();
            state.type_text(&character.to_string());
            parse(&mut state);
            slowest = slowest.max(started.elapsed());
        }
    });
    println!("{:<36} {:>9.2} ms (average {:.2} ms)", "slowest key", slowest.as_secs_f64() * 1000.0, typing.as_secs_f64() * 10.0);

    let visible = middle..state.buffer().line_end(state.buffer().line_count() / 2 + 60);
    time("highlights for 60 visible lines", || state.syntax().highlights(state.buffer().rope(), visible.clone()).len());
    time("paste 100 KB", || {
        state.paste(&text[..100_000.min(text.len())]);
        parse(&mut state);
    });
    time("undo the paste", || {
        state.undo();
        parse(&mut state);
    });
    time("select all, delete", || {
        state.select_all();
        state.backspace();
        parse(&mut state);
    });
    time("undo the delete", || {
        state.undo();
        parse(&mut state);
    });
    assert_eq!(state.buffer().len(), text.len() + 100, "undo brings the whole text back");
    let main = root.join("main.tex");
    time("word count", || count_words(&main, &text, &root, &mut |_| None).total());
}
