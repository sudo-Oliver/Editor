//! Reads `.bib` files for the citation picker: who wrote what, when and where,
//! and the letter that tells two papers of one author and year apart.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::picker::PickerItem;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    /// `Müller`, `Müller and Schmidt` or `Müller et al.`, as natbib writes them.
    pub authors: String,
    pub year: String,
    pub title: String,
    pub venue: String,
}

/// The entries of `files` as picker rows, sorted by author, year and letter.
/// `bbl` is the bibliography of the last build, whose letters win.
pub fn citation_items(files: &[PathBuf], bbl: Option<&str>) -> Vec<PickerItem> {
    let mut entries: Vec<(PathBuf, Entry)> = Vec::new();
    for path in files {
        if let Ok(text) = std::fs::read_to_string(path) {
            entries.extend(parse(&text).into_iter().map(|entry| (path.clone(), entry)));
        }
    }
    let mut letters = computed_letters(entries.iter().map(|(_, entry)| entry));
    if let Some(bbl) = bbl {
        letters.extend(bbl_letters(bbl));
    }
    let mut items: Vec<(String, PickerItem)> = entries
        .into_iter()
        .map(|(path, entry)| {
            let letter = letters.get(&entry.key).copied().flatten();
            (fold(&format!("{} {} {}", entry.authors, entry.year, letter.unwrap_or(' '))), item(path, &entry, letter))
        })
        .collect();
    items.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.label.cmp(&right.1.label)));
    items.into_iter().map(|(_, item)| item).collect()
}

fn item(path: PathBuf, entry: &Entry, letter: Option<char>) -> PickerItem {
    let mut summary = entry.authors.clone();
    if !entry.year.is_empty() {
        if !summary.is_empty() {
            summary.push(' ');
        }
        summary.push_str(&entry.year);
        summary.extend(letter);
    }
    let detail = match (entry.title.is_empty(), entry.venue.is_empty()) {
        (false, false) => format!("{}. {}", entry.title.trim_end_matches('.'), entry.venue),
        (false, true) => entry.title.clone(),
        _ => entry.venue.clone(),
    };
    PickerItem {
        path,
        label: entry.key.clone(),
        position: None,
        summary: (!summary.is_empty()).then_some(summary),
        detail: (!detail.is_empty()).then_some(detail),
    }
}

/// Reads entries with fields in braces, quotes or bare, over several lines.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    // `@string{tug = "TUGBoat"}` names text that fields use bare.
    let mut macros: HashMap<String, String> = HashMap::new();
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        rest = &rest[at + 1..];
        let Some(open) = rest.find(['{', '(']) else {
            break;
        };
        let kind = rest[..open].trim().to_ascii_lowercase();
        if kind.is_empty() || !kind.chars().all(|ch| ch.is_ascii_alphabetic()) {
            continue;
        }
        let body = &rest[open + 1..];
        let mut end = body_end(body, rest.as_bytes()[open] == b'(');
        if end == body.len() {
            // Never closed: it ends where the next entry starts a line, as BibTeX recovers.
            end = body.find("\n@").map_or(end, |line| line + 1);
            rest = &body[end..];
        } else {
            rest = &body[end + 1..];
        }
        if kind == "string" {
            let defined = parse_fields(&body[..end], &macros);
            macros.extend(defined);
            continue;
        }
        if matches!(kind.as_str(), "comment" | "preamble") {
            continue;
        }
        let Some((key, fields)) = body[..end].split_once(',') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let fields: HashMap<String, String> = parse_fields(fields, &macros).into_iter().collect();
        let field = |names: &[&str]| names.iter().find_map(|name| fields.get(*name)).map(String::as_str).unwrap_or_default();
        let year = match field(&["year"]) {
            "" => field(&["date"]).chars().take(4).collect(),
            year => clean(year),
        };
        entries.push(Entry {
            key: key.to_string(),
            authors: authors(field(&["author", "editor"])),
            year,
            title: clean(field(&["title"])),
            venue: clean(field(&["journal", "journaltitle", "booktitle", "publisher", "school", "institution"])),
        });
    }
    entries
}

/// Where the entry that starts `body` ends, at the closing brace or parenthesis.
fn body_end(body: &str, parenthesis: bool) -> usize {
    let mut depth = 0usize;
    for (index, byte) in body.bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' if depth == 0 && !parenthesis => return index,
            b'}' => depth = depth.saturating_sub(1),
            b')' if depth == 0 && parenthesis => return index,
            _ => {}
        }
    }
    body.len()
}

/// The fields as lowercase names and raw values, with `#` joined.
fn parse_fields(text: &str, macros: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut rest = text;
    while let Some(equals) = rest.find('=') {
        let name = rest[..equals].trim_matches(|ch: char| ch.is_whitespace() || ch == ',').to_ascii_lowercase();
        rest = &rest[equals + 1..];
        let mut value = String::new();
        loop {
            rest = rest.trim_start();
            let (part, after) = match rest.as_bytes().first() {
                Some(b'{') => {
                    let end = body_end(&rest[1..], false) + 1;
                    (&rest[1..end], &rest[(end + 1).min(rest.len())..])
                }
                Some(b'"') => {
                    let end = quote_end(&rest[1..]) + 1;
                    (&rest[1..end], &rest[(end + 1).min(rest.len())..])
                }
                _ => {
                    let end = rest.find([',', '#']).unwrap_or(rest.len());
                    let bare = rest[..end].trim();
                    (macros.get(&bare.to_ascii_lowercase()).map_or(bare, String::as_str), &rest[end..])
                }
            };
            value.push_str(part);
            rest = after.trim_start();
            match rest.strip_prefix('#') {
                Some(after) => rest = after,
                None => break,
            }
        }
        fields.push((name, value));
        rest = rest.trim_start_matches(|ch: char| ch.is_whitespace() || ch == ',');
    }
    fields
}

/// The closing quote, which braces protect.
fn quote_end(text: &str) -> usize {
    let mut depth = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'"' if depth == 0 => return index,
            _ => {}
        }
    }
    text.len()
}

/// Splits at `and` outside of braces, so `{Barnes and Noble}` stays one name.
fn split_names(text: &str) -> Vec<&str> {
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
fn last_name(name: &str) -> String {
    if let Some(comma) = outside_braces(name, ',') {
        return clean(&name[..comma]);
    }
    let words = words(name);
    // The von part starts at the first lowercase word that is not the first.
    let from = words.iter().skip(1).position(|word| word.starts_with(|ch: char| ch.is_lowercase())).map_or(words.len().saturating_sub(1), |position| position + 1);
    clean(&words[from..].join(" "))
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

fn authors(field: &str) -> String {
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
    out
}

/// Letters for entries that share authors and year, in the order of their
/// keys, as plainnat and abbrvnat give them when all are cited.
fn computed_letters<'a>(entries: impl Iterator<Item = &'a Entry>) -> HashMap<String, Option<char>> {
    let mut groups: HashMap<(String, String), Vec<&Entry>> = HashMap::new();
    for entry in entries.filter(|entry| !entry.year.is_empty()) {
        groups.entry((fold(&entry.authors), entry.year.clone())).or_default().push(entry);
    }
    let mut letters = HashMap::new();
    for group in groups.values_mut().filter(|group| group.len() > 1) {
        group.sort_by(|left, right| left.key.cmp(&right.key));
        for (letter, entry) in ('a'..='z').zip(group.iter()) {
            letters.insert(entry.key.clone(), Some(letter));
        }
    }
    letters
}

/// The letters of the last build. natbib writes `\bibitem[Müller(2024{\natexlab{a}})]{key}`
/// or `\bibitem[{Müller(2024a)}...]{key}`,
/// biblatex `\entry{key}` with `\field{extradate}{1}`. A cited entry without
/// a letter has none in the PDF either.
fn bbl_letters(bbl: &str) -> HashMap<String, Option<char>> {
    let mut letters = HashMap::new();
    for chunk in bbl.split("\\bibitem[").skip(1) {
        let Some(close) = chunk.find("]{") else {
            continue;
        };
        let label = &chunk[..close];
        let Some(key) = chunk[close + 2..].split('}').next() else {
            continue;
        };
        // Only author-year labels carry a year in parentheses.
        let year = label.match_indices('(').find_map(|(index, _)| {
            let rest = &label[index + 1..];
            let digits = rest.get(..4).filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))?;
            let after = &rest[digits.len()..];
            after.strip_prefix("{\\natexlab{").unwrap_or(after).bytes().next()
        });
        if let Some(after) = year {
            letters.insert(key.trim().to_string(), after.is_ascii_lowercase().then_some(after as char));
        }
    }
    for chunk in bbl.split("\\entry{").skip(1) {
        let Some(key) = chunk.split('}').next() else {
            continue;
        };
        let body = chunk.split("\\endentry").next().unwrap_or_default();
        let extra = body
            .split("\\field{extradate}{")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .and_then(|number| number.trim().parse::<u8>().ok())
            .filter(|number| (1..=26).contains(number))
            .map(|number| (b'a' + number - 1) as char);
        letters.insert(key.trim().to_string(), extra);
    }
    letters
}

/// Where a LaTeX build leaves the bibliography of `main`.
pub fn bbl_path(main: &Path) -> Option<PathBuf> {
    Some(main.parent()?.join(".build").join(format!("{}.bbl", main.file_stem()?.to_string_lossy())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = r#"
@comment{ignored, author = {Nobody}}
@string{tug = "TUGBoat"}
@article{mueller2024b,
  author = {M{\"u}ller, Anna and Schmidt, Ben and Ott, Carl},
  title  = {GPUI Rendering
            Pipelines},
  year   = 2024,
  journal = tug # " Extra",
}
@inproceedings(mueller2024a,
  author = "Anna M{\"u}ller and Ben Schmidt and others",
  title = "Efficient {Rust} Architectures",
  date = {2024-03-01},
  booktitle = {Proc. of {RustConf}},
)
@book{rossum, author = {Guido {van Rossum}}, title = {Python}, year = {1995}}
@misc{beethoven, author = {Ludwig van Beethoven and {Barnes and Noble}}, title = {Op.~1 --- Trios}}
"#;

    #[test]
    fn entries_have_authors_years_titles_and_venues() {
        let entries = parse(BIB);
        let keys: Vec<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys, ["mueller2024b", "mueller2024a", "rossum", "beethoven"]);
        assert_eq!(entries[0].authors, "Müller et al.");
        assert_eq!(entries[0].title, "GPUI Rendering Pipelines");
        assert_eq!(entries[0].year, "2024");
        assert_eq!(entries[0].venue, "TUGBoat Extra");
        assert_eq!((entries[1].authors.as_str(), entries[1].year.as_str()), ("Müller et al.", "2024"));
        assert_eq!(entries[1].venue, "Proc. of RustConf");
        assert_eq!(entries[2].authors, "van Rossum");
        assert_eq!(entries[3].authors, "van Beethoven and Barnes and Noble");
        assert_eq!(entries[3].title, "Op. 1 — Trios");
    }

    #[test]
    fn accents_and_commands_become_text() {
        assert_eq!(clean(r#"G{\"o}del, \'{E}mile, \c{c}a, \v Sa, \ss{}e, \'{\i}, \emph{Big} \& more"#), "Gödel, Émile, ça, Ša, ße, í, Big & more");
        assert_eq!(clean("pages 3--4"), "pages 3–4");
    }

    #[test]
    fn one_author_and_year_get_letters_by_key() {
        let items = citation_items(&[], None);
        assert!(items.is_empty());
        let letters = computed_letters(parse(BIB).iter());
        assert_eq!(letters["mueller2024a"], Some('a'));
        assert_eq!(letters["mueller2024b"], Some('b'));
        assert!(!letters.contains_key("rossum"));
    }

    #[test]
    fn rows_show_author_year_title_and_venue() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("refs.bib");
        std::fs::write(&path, BIB).unwrap();
        let items = citation_items(std::slice::from_ref(&path), None);
        let summaries: Vec<Option<&str>> = items.iter().map(|item| item.summary.as_deref()).collect();
        assert_eq!(summaries, [Some("Müller et al. 2024a"), Some("Müller et al. 2024b"), Some("van Beethoven and Barnes and Noble"), Some("van Rossum 1995")]);
        assert_eq!(items[0].detail.as_deref(), Some("Efficient Rust Architectures. Proc. of RustConf"));
    }

    #[test]
    fn the_build_decides_the_letters() {
        let natbib = r#"\bibitem[{M{\"u}ller et~al.(2024a)M{\"u}ller, Schmidt, and Ott}]{mueller2024b}
\bibitem[{M{\"u}ller et~al.(2024)}]{mueller2024a}
\bibitem[M{\"u}ller(2024{\natexlab{b}})]{plainnat}
\bibitem[Gre93]{alpha}"#;
        let letters = bbl_letters(natbib);
        assert_eq!(letters["mueller2024b"], Some('a'));
        assert_eq!(letters["mueller2024a"], None);
        assert_eq!(letters["plainnat"], Some('b'));
        assert!(!letters.contains_key("alpha"));
        let biblatex = r"\entry{one}{article}{}
  \field{extradate}{2}
\endentry
\entry{two}{article}{}
\endentry";
        let letters = bbl_letters(biblatex);
        assert_eq!((letters["one"], letters["two"]), (Some('b'), None));
    }

    /// A made-up bibliography, as large as they grow over a career.
    fn large_bib(entries: usize) -> String {
        let surnames = [r#"M{\"u}ller"#, "Schmidt", "{van der Berg}", "O'Neil", "Nakamura", r"Dvo{\v r}{\'a}k"];
        let mut text = String::from("@string{jfill = \"Journal of Filler\"}\n");
        for index in 0..entries {
            let surname = surnames[index % surnames.len()];
            let year = 1950 + index % 70;
            text.push_str(&match index % 3 {
                0 => format!("@article{{key{index},\n  author = {{{surname}, Alex and Other, Bo}},\n  title = {{Filler Title {index}}},\n  journal = jfill,\n  year = {year},\n}}\n\n"),
                1 => format!("@book(key{index}, author = \"{surname}, Alex\", title = \"Filler {{Book}} {index}\", publisher = {{Filler Press}}, date = {{{year}-01-02}})\n"),
                _ => format!("@misc{{key{index}, editor = {{{surname}, Alex and Other, Bo and others}}, title = {{Note}} # \" \" # {{{index}}}}}\n"),
            });
        }
        text
    }

    #[test]
    fn twenty_thousand_entries_parse_and_sort() {
        let text = large_bib(20_000);
        let started = std::time::Instant::now();
        let entries = parse(&text);
        assert_eq!(entries.len(), 20_000);
        assert_eq!(entries[0].authors, "Müller and Other");
        assert_eq!(entries[0].venue, "Journal of Filler");
        assert_eq!((entries[1].authors.as_str(), entries[1].year.as_str()), ("Schmidt", "1951"));
        assert_eq!(entries[2].title, "Note 2");
        assert_eq!(entries[5].authors, "Dvořák et al.");

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.bib");
        std::fs::write(&path, &text).unwrap();
        let items = citation_items(std::slice::from_ref(&path), None);
        assert_eq!(items.len(), 20_000);
        let firsts: Vec<String> = items.iter().map(|item| fold(item.summary.as_deref().unwrap_or_default())).collect();
        assert!(firsts.windows(2).all(|pair| pair[0].split(' ').next() <= pair[1].split(' ').next()), "sorted by author");
        // Authors and years repeat, so the letters tell them apart.
        assert!(items.iter().any(|item| item.summary.as_deref().is_some_and(|summary| summary.ends_with('b'))));
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    }

    /// One entry without its closing brace must not take the rest of the file with it.
    #[test]
    fn an_unclosed_entry_loses_only_itself() {
        let text = "@article{first, title = {One}, year = 2001}\n@article{broken, title = {Never closed, year = 2002\n\n@book{after, title = {Two}, year = 2003}\n@misc{last, title = {Three}}\n";
        let keys: Vec<String> = parse(text).into_iter().map(|entry| entry.key).collect();
        assert_eq!(keys, ["first", "broken", "after", "last"]);
    }

    #[test]
    fn folding_finds_umlauts_both_ways() {
        assert_eq!(fold("Müller Gödel Émile Straße"), "mueller goedel emile strasse");
        assert!(crate::picker::fuzzy_score("muller", &fold("Müller")).is_some());
    }
}
