//! The `.aux` file of the last LaTeX run: the number and page each label got.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Aux {
    /// Each label with its number, such as `3.2`, and its page.
    pub labels: Vec<(String, String, String)>,
    /// `\@input{chapter.aux}`: the files `\include` writes its labels to.
    pub inputs: Vec<String>,
}

pub fn parse(text: &str) -> Aux {
    let mut aux = Aux::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("\\newlabel{") {
            let Some((name, rest)) = group_end(rest).map(|end| (&rest[..end], &rest[end + 1..])) else {
                continue;
            };
            // cleveref and others keep their own entries next to the label.
            if name.contains('@') {
                continue;
            }
            let Some(inner) = rest.strip_prefix('{').and_then(|rest| group_end(rest).map(|end| &rest[..end])) else {
                continue;
            };
            let mut groups = Vec::new();
            let mut rest = inner;
            while let Some(after) = rest.strip_prefix('{') {
                let Some(end) = group_end(after) else {
                    break;
                };
                groups.push(&after[..end]);
                rest = &after[end + 1..];
                if groups.len() == 2 {
                    break;
                }
            }
            let number = groups.first().map(|number| crate::text::clean(number)).unwrap_or_default();
            let page = groups.get(1).map(|page| crate::text::clean(page)).unwrap_or_default();
            aux.labels.push((name.to_string(), number, page));
        } else if let Some(rest) = line.strip_prefix("\\@input{")
            && let Some(end) = group_end(rest)
        {
            aux.inputs.push(rest[..end].to_string());
        }
    }
    aux
}

/// Where the group whose `{` came just before `text` closes.
fn group_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(index),
            b'}' => depth -= 1,
            _ => {}
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_get_numbers_and_pages() {
        let text = r"\relax
\providecommand\hyper@newdestlabel[2]{}
\newlabel{sec:intro}{{1}{1}{Introduction}{section.1}{}}
\newlabel{sec:intro@cref}{{[section][1][]1}{[1][1][]1}}
\newlabel{fig:a}{{3.2}{12}}
\newlabel{eq:x}{{\relax 4}{\textbf {7}}}
\@input{chapters/two.aux}
";
        let aux = parse(text);
        assert_eq!(
            aux.labels,
            [("sec:intro".into(), "1".into(), "1".into()), ("fig:a".into(), "3.2".into(), "12".into()), ("eq:x".into(), "4".into(), "7".into())]
        );
        assert_eq!(aux.inputs, ["chapters/two.aux"]);
    }
}
