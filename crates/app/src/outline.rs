use editor_core::{Buffer, Language};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub level: usize,
    pub title: String,
    pub line: usize,
}

const LATEX_LEVELS: &[(&str, usize)] = &[
    ("\\part", 0),
    ("\\chapter", 1),
    ("\\section", 2),
    ("\\subsection", 3),
    ("\\subsubsection", 4),
];

/// Lists document headings. Lines are scanned on their own, so a heading that
/// spans several lines keeps only its first line.
pub fn outline(buffer: &Buffer, language: Language) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    for line in 0..buffer.line_count() {
        let text = buffer.line_text(line);
        let trimmed = text.trim_start();
        let heading = match language {
            Language::Latex => latex_heading(trimmed),
            Language::Typst => marker_heading(trimmed, '='),
            Language::Markdown => marker_heading(trimmed, '#'),
            Language::Plain => None,
        };
        if let Some((level, title)) = heading {
            let title = match language {
                Language::Latex => clean_latex(&title),
                _ => clean_markup(&title, language),
            };
            items.push(OutlineItem { level, title, line });
        }
    }
    items
}

fn latex_heading(line: &str) -> Option<(usize, String)> {
    for (command, level) in LATEX_LEVELS {
        let Some(rest) = line.strip_prefix(command) else {
            continue;
        };
        let rest = rest.strip_prefix('*').unwrap_or(rest);
        let rest = rest.strip_prefix('{')?;
        let mut depth = 1;
        for (index, ch) in rest.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((*level, rest[..index].to_string()));
                    }
                }
                _ => {}
            }
        }
        return Some((*level, rest.to_string()));
    }
    None
}

fn marker_heading(line: &str, marker: char) -> Option<(usize, String)> {
    let level = line.chars().take_while(|ch| *ch == marker).count();
    if level == 0 {
        return None;
    }
    let title = line[level..].strip_prefix(' ')?;
    Some((level, title.trim().to_string()))
}

/// Keeps the text of `\\emph{x}`, and the name of `\\LaTeX` or `\\alpha`.
fn clean_latex(title: &str) -> String {
    let mut result = String::new();
    let mut chars = title.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let mut name = String::new();
                while let Some(letter) = chars.next_if(char::is_ascii_alphabetic) {
                    name.push(letter);
                }
                if name.is_empty() {
                    match chars.next() {
                        Some('\\') | None => result.push(' '),
                        Some(escaped) => result.push(escaped),
                    }
                } else {
                    // `\\LaTeX{}` is a name, but `\\emph{x}` only wraps its argument.
                    let mut ahead = chars.clone();
                    if ahead.next() != Some('{') || ahead.next() == Some('}') {
                        result.push_str(&name);
                    }
                }
            }
            '{' | '}' | '$' => {}
            '~' => result.push(' '),
            other => result.push(other),
        }
    }
    result.trim().to_string()
}

/// Removes Typst and Markdown emphasis, labels and link targets. The text
/// inside `$…$` stays as written, because `_` and `*` mean something there.
fn clean_markup(title: &str, language: Language) -> String {
    let mut title = title.trim();
    if language == Language::Typst
        && let Some(position) = title.rfind(" <")
        && title.ends_with('>')
    {
        title = &title[..position];
    }
    let characters: Vec<char> = title.trim_end_matches([' ', '#']).chars().collect();
    let mut result = String::new();
    let mut in_math = false;
    let mut index = 0;
    while index < characters.len() {
        let ch = characters[index];
        let next = characters.get(index + 1);
        index += 1;
        match ch {
            '\\' if next.is_some() => {
                result.extend(next);
                index += 1;
            }
            '$' => in_math = !in_math,
            _ if in_math => result.push(ch),
            '*' | '`' => {}
            '_' => {
                let inside_word = result.chars().last().is_some_and(char::is_alphanumeric)
                    && next.is_some_and(|next| next.is_alphanumeric());
                if inside_word {
                    result.push('_');
                }
            }
            '[' if language == Language::Markdown => {}
            ']' if language == Language::Markdown && next == Some(&'(') => {
                while index < characters.len() && characters[index] != ')' {
                    index += 1;
                }
                index += 1;
            }
            _ => result.push(ch),
        }
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_latex_headings() {
        let buffer = Buffer::new("\\section{A \\emph{b}}\ntext\n  \\subsection*{C}\n\\sectionx{no}\n");
        let items = outline(&buffer, Language::Latex);
        assert_eq!(
            items,
            vec![
                OutlineItem { level: 2, title: "A b".into(), line: 0 },
                OutlineItem { level: 3, title: "C".into(), line: 2 },
            ]
        );
    }

    #[test]
    fn finds_marker_headings() {
        let typst = Buffer::new("= One\n== Two\n=no\n");
        assert_eq!(outline(&typst, Language::Typst).len(), 2);
        let markdown = Buffer::new("# One\n#tag\n### Three\n");
        assert_eq!(outline(&markdown, Language::Markdown)[1].level, 3);
    }

    #[test]
    fn titles_lose_their_markup() {
        assert_eq!(clean_markup("*1.* Nicht immer", Language::Typst), "1. Nicht immer");
        assert_eq!(clean_markup("Beispiel zu $N$ und $P$", Language::Typst), "Beispiel zu N und P");
        assert_eq!(clean_markup("_kurz_ $x_1 * y$ snake_case <sec:a>", Language::Typst), "kurz x_1 * y snake_case");
        assert_eq!(clean_markup("**Bold** [Pandoc](https://pandoc.org) `raw` ##", Language::Markdown), "Bold Pandoc raw");
        assert_eq!(clean_latex("The \\LaTeX{} \\textbf{way} of $\\alpha$~\\& more"), "The LaTeX way of alpha & more");
    }
}
