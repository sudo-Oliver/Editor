//! Field text as people read it: names, accents, and the folded form search uses.

/// Splits at `and` outside of braces, so `{Barnes and Noble}` stays one name.
pub fn split_names(text: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let (mut depth, mut start) = (0usize, 0);
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            byte if depth == 0 && byte.is_ascii_whitespace() => {
                let word = &text[index + 1..];
                if word.len() > 4 && word[..3].eq_ignore_ascii_case("and") && word.as_bytes()[3].is_ascii_whitespace() {
                    names.push(text[start..index].trim());
                    index += 4;
                    start = index;
                    continue;
                }
            }
            _ => {}
        }
        index += 1;
    }
    names.push(text[start..].trim());
    names.into_iter().filter(|name| !name.is_empty()).collect()
}

/// The last name of `Last, First`, `First Last` or `First von Last`.
pub fn last_name(name: &str) -> String {
    if let Some(comma) = outside_braces(name, ',') {
        return clean(&name[..comma]);
    }
    let words = words(name);
    // The von part starts at the first lowercase word that is not the first.
    let from = words.iter().skip(1).position(|word| word.starts_with(|ch: char| ch.is_lowercase())).map_or(words.len().saturating_sub(1), |position| position + 1);
    clean(&words[from..].join(" "))
}

/// `First von Last` from any of the three forms BibTeX knows.
pub fn full_name(name: &str) -> String {
    let parts: Vec<&str> = split_outside_braces(name, ',');
    match parts.as_slice() {
        // `von Last, Jr, First`
        [last, junior, first, ..] => clean(&format!("{} {}, {}", first.trim(), last.trim(), junior.trim())),
        [last, first] => clean(&format!("{} {}", first.trim(), last.trim())),
        _ => clean(name),
    }
}

fn split_outside_braces(text: &str, wanted: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = outside_braces(rest, wanted) {
        parts.push(&rest[..at]);
        rest = &rest[at + wanted.len_utf8()..];
    }
    parts.push(rest);
    parts
}

fn outside_braces(text: &str, wanted: char) -> Option<usize> {
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ch if ch == wanted && depth == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let (mut depth, mut start) = (0usize, None);
    for (index, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ch if ch.is_whitespace() && depth == 0 => {
                if let Some(from) = start.take() {
                    words.push(&text[from..index]);
                }
                continue;
            }
            _ => {}
        }
        start.get_or_insert(index);
    }
    words.extend(start.map(|from| &text[from..]));
    words
}

/// `Müller`, `Müller and Schmidt` or `Müller et al.`, as natbib writes them.
pub fn authors(field: &str) -> String {
    let names = split_names(field);
    let others = names.last().is_some_and(|name| name.eq_ignore_ascii_case("others"));
    let last: Vec<String> = names.iter().filter(|name| !name.eq_ignore_ascii_case("others")).map(|name| last_name(name)).collect();
    match (last.as_slice(), others) {
        ([], _) => String::new(),
        ([one], false) => one.clone(),
        ([one, two], false) => format!("{one} and {two}"),
        ([first, ..], _) => format!("{first} et al."),
    }
}

/// Every name in full, as the detail of an entry lists them.
pub fn full_authors(field: &str) -> String {
    let names: Vec<String> =
        split_names(field).into_iter().map(|name| if name.eq_ignore_ascii_case("others") { "et al.".to_string() } else { full_name(name) }).collect();
    names.join(", ")
}

/// Accents with a symbol, as in `\"u`, and the letters they make.
const SYMBOL_ACCENTS: &[(char, &str, &str)] = &[
    ('"', "aeiouyAEIOU", "äëïöüÿÄËÏÖÜ"),
    ('\'', "aeiouycnszAEIOUYCNSZ", "áéíóúýćńśźÁÉÍÓÚÝĆŃŚŹ"),
    ('`', "aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
    ('^', "aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
    ('~', "anoANO", "ãñõÃÑÕ"),
    ('=', "aeiouAEIOU", "āēīōūĀĒĪŌŪ"),
    ('.', "zZ", "żŻ"),
];

/// Accents with a letter, as in `\c{c}`.
const LETTER_ACCENTS: &[(&str, &str, &str)] = &[
    ("c", "csCS", "çşÇŞ"),
    ("v", "cszrenCSZREN", "čšžřěňČŠŽŘĚŇ"),
    ("H", "ouOU", "őűŐŰ"),
    ("u", "agAG", "ăğĂĞ"),
    ("k", "aeAE", "ąęĄĘ"),
    ("r", "auAU", "åůÅŮ"),
];

/// Letters that are a command of their own.
const LETTERS: &[(&str, &str)] =
    &[("ss", "ß"), ("o", "ø"), ("O", "Ø"), ("aa", "å"), ("AA", "Å"), ("ae", "æ"), ("AE", "Æ"), ("oe", "œ"), ("OE", "Œ"), ("l", "ł"), ("L", "Ł"), ("i", "i"), ("j", "j")];

fn accented(base: &str, marked: &str, ch: char) -> char {
    base.chars().position(|candidate| candidate == ch).and_then(|index| marked.chars().nth(index)).unwrap_or(ch)
}

/// Whether `text` writes a letter with TeX, as in `{\"u}` or `\ss`, which
/// biber reads but UTF-8 says more plainly.
pub fn has_tex_accent(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(2).enumerate().any(|(index, pair)| {
        pair[0] == b'\\'
            && (SYMBOL_ACCENTS.iter().any(|(accent, ..)| *accent as u8 == pair[1]) || {
                let name: String = text[index + 1..].chars().take_while(char::is_ascii_alphabetic).collect();
                LETTER_ACCENTS.iter().any(|(command, ..)| *command == name) || LETTERS.iter().any(|(command, _)| *command == name && !matches!(*command, "i" | "j"))
            })
    })
}

/// Plain text from a field: accents resolved, braces and commands gone.
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // The letter after an accent, in braces or after a space.
    let argument = |chars: &mut std::iter::Peekable<std::str::Chars>| {
        while chars.next_if(|ch| *ch == '{' || ch.is_whitespace()).is_some() {}
        let ch = chars.next();
        // `\'{\i}` puts the accent on a dotless i.
        let ch = if ch == Some('\\') { chars.next() } else { ch };
        chars.next_if_eq(&'}');
        ch
    };
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let Some(&next) = chars.peek() else {
                    break;
                };
                if let Some((_, base, marked)) = SYMBOL_ACCENTS.iter().find(|(accent, ..)| *accent == next) {
                    chars.next();
                    out.extend(argument(&mut chars).map(|letter| accented(base, marked, letter)));
                } else if next.is_ascii_alphabetic() {
                    let mut name = String::new();
                    while let Some(letter) = chars.next_if(char::is_ascii_alphabetic) {
                        name.push(letter);
                    }
                    if let Some((_, base, marked)) = LETTER_ACCENTS.iter().find(|(command, ..)| *command == name) {
                        out.extend(argument(&mut chars).map(|letter| accented(base, marked, letter)));
                    } else if let Some((_, letter)) = LETTERS.iter().find(|(command, _)| *command == name) {
                        out.push_str(letter);
                        chars.next_if_eq(&' ');
                    }
                    // Any other command goes, and its argument stays as text.
                } else {
                    // `\&`, `\%` and their kind.
                    out.extend(chars.next());
                }
            }
            '{' | '}' => {}
            '~' => out.push(' '),
            '-' if chars.peek() == Some(&'-') => {
                chars.next();
                out.push(if chars.next_if_eq(&'-').is_some() { '—' } else { '–' });
            }
            ch if ch.is_whitespace() => {
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
            }
            ch => out.push(ch),
        }
    }
    out.trim_end().to_string()
}

/// Lowercase ASCII for searching, so that `mueller` and `muller` find Müller.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    fold_into(text, &mut out);
    out
}

/// [`fold`], appended to `out`.
pub fn fold_into(text: &str, out: &mut String) {
    for ch in text.chars().flat_map(char::to_lowercase) {
        match ch {
            'ä' => out.push_str("ae"),
            'ö' => out.push_str("oe"),
            'ü' => out.push_str("ue"),
            'ß' => out.push_str("ss"),
            ch if ch.is_ascii() => out.push(ch),
            ch => {
                let base = SYMBOL_ACCENTS
                    .iter()
                    .map(|(_, base, marked)| (*base, *marked))
                    .chain(LETTER_ACCENTS.iter().map(|(_, base, marked)| (*base, *marked)))
                    .find_map(|(base, marked)| marked.chars().position(|candidate| candidate == ch).and_then(|index| base.chars().nth(index)));
                out.push(base.unwrap_or(ch));
            }
        }
    }
}

/// The line and column, both from 0, of byte `offset`. The column counts chars.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = floor_char_boundary(text, offset);
    let before = &text[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count();
    let start = before.rfind('\n').map_or(0, |newline| newline + 1);
    (line, text[start..offset].chars().count())
}

pub(crate) fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_and_commands_become_text() {
        assert_eq!(clean(r#"G{\"o}del, \'{E}mile, \c{c}a, \v Sa, \ss{}e, \'{\i}, \emph{Big} \& more"#), "Gödel, Émile, ça, Ša, ße, í, Big & more");
        assert_eq!(clean("pages 3--4"), "pages 3–4");
    }

    #[test]
    fn folding_finds_umlauts_both_ways() {
        assert_eq!(fold("Müller Gödel Émile Straße"), "mueller goedel emile strasse");
        assert!(crate::fuzzy::score("muller", &fold("Müller")).is_some());
    }

    #[test]
    fn names_in_full_and_short() {
        assert_eq!(authors(r#"M{\"u}ller, Anna and Schmidt, Ben and Ott, Carl"#), "Müller et al.");
        assert_eq!(authors("Guido {van Rossum}"), "van Rossum");
        assert_eq!(authors("Ludwig van Beethoven and {Barnes and Noble}"), "van Beethoven and Barnes and Noble");
        assert_eq!(full_authors(r#"M{\"u}ller, Anna and Ben Schmidt and others"#), "Anna Müller, Ben Schmidt, et al.");
        assert_eq!(full_authors("King, Jr, Martin Luther"), "Martin Luther King, Jr");
    }

    #[test]
    fn tex_accents_are_noticed() {
        assert!(has_tex_accent(r#"M{\"u}ller"#));
        assert!(has_tex_accent(r"Stra{\ss}e"));
        assert!(has_tex_accent(r"\v{S}a"));
        assert!(!has_tex_accent(r"\emph{Big} \& more"));
        assert!(!has_tex_accent("Müller"));
    }

    #[test]
    fn lines_and_columns_count_from_zero() {
        let text = "ab\nüc\nd";
        assert_eq!(line_col(text, 0), (0, 0));
        assert_eq!(line_col(text, 3), (1, 0));
        assert_eq!(line_col(text, 6), (1, 2));
        assert_eq!(line_col(text, 99), (2, 1));
    }
}
