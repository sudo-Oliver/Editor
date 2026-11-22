//! Text at the edges of what the editor core expects: one huge line, mixed line
//! endings, characters that span rope chunks, and very large undo steps. All
//! text is made up here.

use std::time::{Duration, Instant};

use editor_core::{EditorState, Language, Motion, Selection};
use unicode_segmentation::UnicodeSegmentation;

fn text(state: &EditorState) -> String {
    state.buffer().rope().to_string()
}

fn parse(state: &mut EditorState) {
    if let Some(job) = state.start_parse() {
        let result = job.run();
        state.finish_parse(result);
    }
}

/// Generous, since the tests also run unoptimized.
fn within(limit: Duration, label: &str, work: impl FnOnce()) {
    let started = Instant::now();
    work();
    let elapsed = started.elapsed();
    assert!(elapsed < limit, "{label} took {elapsed:?}");
}

#[test]
fn a_one_megabyte_line_stays_editable() {
    let line: String = "Filler words \\textbf{bold} and $x^2$ math. ".chars().cycle().take(1024 * 1024).collect();
    let mut state = EditorState::new(&line, Language::Latex);
    assert_eq!(state.buffer().line_count(), 1);
    within(Duration::from_secs(20), "parsing", || parse(&mut state));

    // Typing in the middle and at the end of the line, each key parsed.
    let middle = state.buffer().clip(line.len() / 2);
    state.set_selections(vec![Selection::cursor(middle)]);
    within(Duration::from_secs(20), "typing in the middle", || {
        for character in "typed".chars() {
            state.type_text(&character.to_string());
            parse(&mut state);
        }
    });
    state.move_selections(Motion::LineEnd, false);
    assert_eq!(state.selections().iter().next().unwrap().range(), state.buffer().len()..state.buffer().len());
    within(Duration::from_secs(20), "moving by characters and words", || {
        for _ in 0..20 {
            state.move_selections(Motion::Left, false);
            state.move_selections(Motion::WordLeft, false);
        }
    });
    let highlights = state.syntax().highlights(state.buffer().rope(), 0..4096);
    assert!(!highlights.is_empty());

    let mut expected = line.clone();
    expected.insert_str(middle, "typed");
    assert_eq!(text(&state), expected);
    while state.undo() {}
    assert_eq!(text(&state), line);
}

#[test]
fn line_endings_and_a_byte_order_mark_stay_except_for_the_edit() {
    let cases = [
        "\u{feff}\\section{One}\r\nFirst line.\r\nSecond line.\r\n",
        "plain\nunix\nendings\n",
        "\u{feff}mixed\r\nendings\nhere\r\nand\rthere\n",
        "no final newline\r\nat all",
    ];
    for original in cases {
        let mut state = EditorState::new(original, Language::Latex);
        // A word in the second line, replaced by typing over it.
        let start = state.buffer().line_start(1);
        let end = state.buffer().next_word_end(start);
        state.set_selections(vec![Selection::new(start, end)]);
        state.type_text("Edited");
        let mut expected = original.to_string();
        expected.replace_range(start..end, "Edited");
        assert_eq!(text(&state), expected, "{original:?}");
        // A new line in the middle of a line leaves the others alone.
        let after = state.buffer().line_end(1);
        state.set_selections(vec![Selection::cursor(after)]);
        state.newline();
        let changed = text(&state);
        assert!(changed.starts_with(&expected[..after]) && changed.ends_with(&expected[after..]), "{original:?} became {changed:?}");
        assert_eq!(original.starts_with('\u{feff}'), changed.starts_with('\u{feff}'));
        while state.undo() {}
        assert_eq!(text(&state), original);
        // Every line, as the editor shows it, lacks its ending. A lone `\r` ends no line.
        for line in 0..state.buffer().line_count() {
            let shown = state.buffer().line_text(line);
            assert!(!shown.contains('\n') && !shown.ends_with('\r'), "{original:?} line {line}");
        }
    }
}

/// Graphemes made of several characters, placed so that ropey's chunks, about a
/// kilobyte each, end inside some of them.
fn clusters() -> String {
    let pieces = ["e\u{301}", "👩‍👩‍👧", "🇩🇪", "a\u{308}\u{323}", "👍🏽", "\u{1100}\u{1161}\u{11a8}", "x"];
    let mut text = String::new();
    for index in 0..4000 {
        text.push_str(pieces[index % pieces.len()]);
        if index % 11 == 0 {
            text.push(' ');
        }
    }
    text
}

#[test]
fn clusters_across_rope_chunks_move_and_delete_whole() {
    let original = clusters();
    let mut state = EditorState::new(&original, Language::Latex);

    let mut split = 0;
    let mut offset = 0;
    let boundaries: Vec<usize> = original.grapheme_indices(true).map(|(index, _)| index).collect();
    for chunk in state.buffer().rope().chunks() {
        offset += chunk.len();
        if offset < original.len() && boundaries.binary_search(&offset).is_err() {
            split += 1;
        }
    }
    assert!(split > 0, "no chunk ends inside a cluster, so the test proves nothing");

    // Moving right visits every cluster start, and left every one back.
    let mut forward = vec![0];
    let mut position = 0;
    while position < original.len() {
        position = state.buffer().next_boundary(position);
        forward.push(position);
    }
    let mut expected = boundaries.clone();
    expected.push(original.len());
    assert_eq!(forward, expected);
    let mut backward = vec![original.len()];
    while position > 0 {
        position = state.buffer().previous_boundary(position);
        backward.push(position);
    }
    backward.reverse();
    assert_eq!(backward, expected);

    // Backspace after a cluster removes all of it.
    let mut removed = 0;
    for chunk_end in state.buffer().rope().chunks().scan(0, |end, chunk| {
        *end += chunk.len();
        Some(*end)
    }) {
        let Err(inside) = boundaries.binary_search(&chunk_end) else {
            continue;
        };
        if inside >= boundaries.len() || chunk_end >= original.len() {
            continue;
        }
        let (start, end) = (boundaries[inside - 1], boundaries[inside]);
        let mut probe = EditorState::new(&original, Language::Latex);
        probe.set_selections(vec![Selection::cursor(end)]);
        probe.backspace();
        let mut wanted = original.clone();
        wanted.replace_range(start..end, "");
        assert_eq!(text(&probe), wanted, "the cluster at {start}..{end}");
        // UTF-16 offsets, as the platform uses, land on the same places.
        assert_eq!(probe.buffer().utf16_to_offset(probe.buffer().offset_to_utf16(start)), start);
        removed += 1;
    }
    assert!(removed > 0);

    // Typing into the middle of a cluster's bytes ends up at a character boundary.
    let inside = original.find("👩").unwrap() + 2;
    let clipped = state.buffer().clip(inside);
    assert!(original.is_char_boundary(clipped));
    state.set_selections(vec![Selection::cursor(clipped)]);
    state.type_text("z");
    assert!(text(&state).contains('z'));
    state.undo();
    assert_eq!(text(&state), original);
}

#[test]
fn ten_megabytes_go_through_undo_and_redo() {
    let paragraph = "Lorem ipsum filler, \\emph{generated} for a test; äöü 🙂.\n";
    let large: String = paragraph.repeat(10 * 1024 * 1024 / paragraph.len() + 1);
    let base = "\\documentclass{article}\n\\begin{document}\n\\end{document}\n";
    let mut state = EditorState::new(base, Language::Latex);
    let at = state.buffer().line_start(2);
    state.set_selections(vec![Selection::cursor(at)]);
    within(Duration::from_secs(30), "pasting and undoing 10 MB", || {
        state.paste(&large);
        let pasted = text(&state);
        assert_eq!(pasted.len(), base.len() + large.len());
        for round in 0..3 {
            assert!(state.undo(), "round {round}");
            assert_eq!(text(&state), base);
            assert!(state.redo(), "round {round}");
            assert_eq!(state.buffer().len(), pasted.len());
        }
        assert_eq!(text(&state), pasted);

        // Deleting all of it is one step too, both ways.
        state.select_all();
        state.backspace();
        assert!(state.buffer().is_empty());
        assert!(state.undo());
        assert_eq!(text(&state), pasted);
        assert!(state.redo());
        assert!(state.buffer().is_empty());
        while state.undo() {}
        assert_eq!(text(&state), base);
    });
}
