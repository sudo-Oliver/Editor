//! The fuzzy match of the pickers and suggestions.

/// Rates a subsequence match. Runs of letters and starts of names count more.
/// It walks the candidate once and allocates nothing, since the file picker and
/// the suggestions call it for every entry on every key.
pub fn score(query: &str, candidate: &str) -> Option<i64> {
    let mut score = 0i64;
    let mut previous: Option<usize> = None;
    let mut chars = candidate.chars();
    // The index of the next char of `chars`, and the char before it.
    let (mut index, mut before) = (0usize, None);
    for wanted in query.chars().filter(|ch| !ch.is_whitespace()) {
        let wanted = lower(wanted);
        loop {
            let ch = chars.next()?;
            let (found, prior) = (index, before);
            index += 1;
            before = Some(ch);
            if lower(ch) != wanted {
                continue;
            }
            score += 1;
            if previous.is_some_and(|previous| previous + 1 == found) {
                score += 8;
            }
            if prior.is_none_or(|prior| matches!(prior, '/' | '\\' | '_' | '-' | ' ' | '.')) {
                score += 6;
            }
            previous = Some(found);
            break;
        }
    }
    // Among equal matches, shorter paths win.
    Some(score * 16 - (index + chars.count()) as i64)
}

/// The first char of the lowercase form, with a fast path for ASCII.
fn lower(ch: char) -> char {
    if ch.is_ascii() { ch.to_ascii_lowercase() } else { ch.to_lowercase().next().unwrap_or(ch) }
}
