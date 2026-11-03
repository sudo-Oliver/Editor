use crate::Repository;

/// Build output in any project directory stays out of the lists.
pub(crate) const WHOLE_REPOSITORY: [&str; 2] = [":/", ":(top,exclude,glob)**/.build/**"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` on a detached HEAD.
    pub branch: Option<String>,
    /// `None` before the first commit.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<FileChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the repository root.
    pub path: String,
    /// The earlier name of a renamed file.
    pub old_path: Option<String>,
    /// The letter of the change in the index.
    pub staged: Option<char>,
    /// The letter of the change in the working tree. `?` means untracked.
    pub unstaged: Option<char>,
    pub conflicted: bool,
}

impl Repository {
    pub fn status(&self) -> Result<Status, String> {
        let mut args = vec!["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all", "--"];
        args.extend(WHOLE_REPOSITORY);
        Ok(parse_status(&self.run(args)?))
    }
}

/// Reads `git status --porcelain=v2 --branch -z`.
fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    let mut records = text.split('\0');
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            parse_header(header, &mut status);
        } else if let Some(rest) = record.strip_prefix("1 ") {
            // XY sub mH mI mW hH hI path
            if let Some(file) = parse_change(rest, 7) {
                status.files.push(file);
            }
        } else if let Some(rest) = record.strip_prefix("2 ") {
            // XY sub mH mI mW hH hI score path, then the old path as its own record.
            if let Some(mut file) = parse_change(rest, 8) {
                file.old_path = records.next().map(ToString::to_string);
                status.files.push(file);
            }
        } else if let Some(rest) = record.strip_prefix("u ") {
            // XY sub m1 m2 m3 mW h1 h2 h3 path
            if let Some(path) = rest.splitn(10, ' ').nth(9) {
                status.files.push(FileChange {
                    path: path.to_string(),
                    old_path: None,
                    staged: None,
                    unstaged: None,
                    conflicted: true,
                });
            }
        } else if let Some(path) = record.strip_prefix("? ") {
            status.files.push(FileChange {
                path: path.to_string(),
                old_path: None,
                staged: None,
                unstaged: Some('?'),
                conflicted: false,
            });
        }
    }
    status
}

fn parse_header(header: &str, status: &mut Status) {
    let Some((name, value)) = header.split_once(' ') else {
        return;
    };
    match name {
        "branch.oid" if value != "(initial)" => status.oid = Some(value.to_string()),
        "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
        "branch.upstream" => status.upstream = Some(value.to_string()),
        "branch.ab" => {
            for part in value.split(' ') {
                if let Some(count) = part.strip_prefix('+') {
                    status.ahead = count.parse().unwrap_or(0);
                } else if let Some(count) = part.strip_prefix('-') {
                    status.behind = count.parse().unwrap_or(0);
                }
            }
        }
        _ => {}
    }
}

fn parse_change(rest: &str, fields_before_path: usize) -> Option<FileChange> {
    let mut parts = rest.splitn(fields_before_path + 1, ' ');
    let mut letters = parts.next()?.chars();
    let letter = |ch: char| (ch != '.').then_some(ch);
    let (staged, unstaged) = (letter(letters.next()?), letter(letters.next()?));
    let path = parts.nth(fields_before_path - 1)?;
    Some(FileChange { path: path.to_string(), old_path: None, staged, unstaged, conflicted: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASHES: &str = "N... 100644 100644 100644 aaa bbb";

    #[test]
    fn reads_branch_headers() {
        let status = parse_status("# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -3\0");
        assert_eq!(status.oid.as_deref(), Some("abc"));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 3));
    }

    #[test]
    fn unborn_and_detached_have_no_values() {
        assert_eq!(parse_status("# branch.oid (initial)\0# branch.head main\0").oid, None);
        assert_eq!(parse_status("# branch.oid abc\0# branch.head (detached)\0").branch, None);
    }

    #[test]
    fn reads_changes_with_spaces_and_line_breaks() {
        let text = format!("1 M. {HASHES} a b.tex\01 .M {HASHES} line\nbreak.tex\0? new file.tex\0");
        let files = parse_status(&text).files;
        assert_eq!((files[0].path.as_str(), files[0].staged, files[0].unstaged), ("a b.tex", Some('M'), None));
        assert_eq!((files[1].path.as_str(), files[1].staged, files[1].unstaged), ("line\nbreak.tex", None, Some('M')));
        assert_eq!((files[2].path.as_str(), files[2].unstaged), ("new file.tex", Some('?')));
    }

    #[test]
    fn rename_takes_the_next_record_as_old_path() {
        let text = format!("2 R. {HASHES} R100 new name.tex\0old name.tex\0? other\0");
        let files = parse_status(&text).files;
        assert_eq!(files[0].path, "new name.tex");
        assert_eq!(files[0].old_path.as_deref(), Some("old name.tex"));
        assert_eq!(files[0].staged, Some('R'));
        assert_eq!(files[1].path, "other");
    }

    #[test]
    fn conflict_is_marked() {
        let files = parse_status("u UU N... 100644 100644 100644 100644 a b c my file.tex\0").files;
        assert!(files[0].conflicted);
        assert_eq!(files[0].path, "my file.tex");
    }
}
