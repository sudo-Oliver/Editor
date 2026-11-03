use crate::Repository;

pub const PAGE_SIZE: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Such as `2 hours ago`.
    pub age: String,
    /// Such as `HEAD -> main` and `origin/main`.
    pub refs: Vec<String>,
    pub subject: String,
}

impl Repository {
    /// One page of history across all branches, children before parents.
    pub fn log(&self, skip: usize) -> Result<Vec<Commit>, String> {
        let args = [
            "log".to_string(),
            "--topo-order".to_string(),
            "--branches".to_string(),
            "--remotes".to_string(),
            "--tags".to_string(),
            "HEAD".to_string(),
            format!("--max-count={PAGE_SIZE}"),
            format!("--skip={skip}"),
            "--format=%H%x00%P%x00%an%x00%ar%x00%D%x00%s".to_string(),
        ];
        Ok(self.run(args)?.lines().filter_map(parse_commit).collect())
    }
}

impl Repository {
    /// Changes when any branch, tag or HEAD moves, without reading history.
    pub fn refs_fingerprint(&self) -> Result<String, String> {
        let refs = self.run(["for-each-ref", "--format=%(objectname) %(refname)"])?;
        let head = self.run(["rev-parse", "--verify", "-q", "HEAD"]).unwrap_or_default();
        Ok(format!("{head}{refs}"))
    }
}

fn parse_commit(line: &str) -> Option<Commit> {
    let mut fields = line.splitn(6, '\0');
    let hash = fields.next()?.to_string();
    let parents = fields.next()?.split_whitespace().map(ToString::to_string).collect();
    let author = fields.next()?.to_string();
    let age = fields.next()?.to_string();
    let refs = fields.next()?.split(", ").filter(|name| !name.is_empty()).map(ToString::to_string).collect();
    let subject = fields.next()?.to_string();
    Some(Commit { hash, parents, author, age, refs, subject })
}

/// How one row of the history graph is drawn. Columns are lanes from the left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane of the commit dot.
    pub column: usize,
    /// Lanes that run past this commit.
    pub through: Vec<usize>,
    /// Lanes that end in the dot, from the top edge of the row.
    pub incoming: Vec<usize>,
    /// Lanes that start at the dot, to the bottom edge of the row.
    pub outgoing: Vec<usize>,
}

/// Assigns commits to lanes one at a time, so that a new page continues the graph.
#[derive(Clone, Debug, Default)]
pub struct Lanes {
    /// The commit that each lane is waiting for.
    waiting: Vec<Option<String>>,
}

impl Lanes {
    pub fn next_row(&mut self, hash: &str, parents: &[String]) -> GraphRow {
        let incoming: Vec<usize> = (0..self.waiting.len())
            .filter(|lane| self.waiting[*lane].as_deref() == Some(hash))
            .collect();
        for lane in &incoming {
            self.waiting[*lane] = None;
        }
        let column = incoming.first().copied().unwrap_or_else(|| self.free_lane());
        let through = (0..self.waiting.len())
            .filter(|lane| self.waiting[*lane].is_some())
            .collect();

        let mut outgoing = Vec::with_capacity(parents.len());
        for (index, parent) in parents.iter().enumerate() {
            let existing = self.waiting.iter().position(|waiting| waiting.as_deref() == Some(parent));
            let lane = match existing {
                Some(lane) => lane,
                // The first parent continues straight down.
                None if index == 0 => column,
                None => self.free_lane(),
            };
            if lane == self.waiting.len() {
                self.waiting.push(None);
            }
            self.waiting[lane] = Some(parent.clone());
            outgoing.push(lane);
        }
        while self.waiting.last().is_some_and(Option::is_none) {
            self.waiting.pop();
        }
        GraphRow { column, through, incoming, outgoing }
    }

    fn free_lane(&self) -> usize {
        self.waiting.iter().position(Option::is_none).unwrap_or(self.waiting.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(commits: &[(&str, &[&str])]) -> Vec<GraphRow> {
        let mut lanes = Lanes::default();
        commits
            .iter()
            .map(|(hash, parents)| {
                let parents: Vec<String> = parents.iter().map(ToString::to_string).collect();
                lanes.next_row(hash, &parents)
            })
            .collect()
    }

    fn row(column: usize, through: &[usize], incoming: &[usize], outgoing: &[usize]) -> GraphRow {
        GraphRow { column, through: through.to_vec(), incoming: incoming.to_vec(), outgoing: outgoing.to_vec() }
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        assert_eq!(
            rows(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]),
            vec![row(0, &[], &[], &[0]), row(0, &[], &[0], &[0]), row(0, &[], &[0], &[])]
        );
    }

    #[test]
    fn two_branches_join_at_their_common_parent() {
        assert_eq!(
            rows(&[("x", &["a"]), ("y", &["a"]), ("a", &[])]),
            vec![row(0, &[], &[], &[0]), row(1, &[0], &[], &[0]), row(0, &[], &[0], &[])]
        );
    }

    #[test]
    fn merge_opens_a_lane_and_it_closes_again() {
        assert_eq!(
            rows(&[("m", &["a", "b"]), ("b", &["r"]), ("a", &["r"]), ("r", &[])]),
            vec![
                row(0, &[], &[], &[0, 1]),
                row(1, &[0], &[1], &[1]),
                row(0, &[1], &[0], &[1]),
                row(1, &[], &[1], &[]),
            ]
        );
    }

    #[test]
    fn octopus_merge_opens_several_lanes() {
        let rows = rows(&[("m", &["a", "b", "c"]), ("c", &[]), ("b", &[]), ("a", &[])]);
        assert_eq!(rows[0].outgoing, vec![0, 1, 2]);
        assert_eq!(rows[1], row(2, &[0, 1], &[2], &[]));
        assert_eq!(rows[2], row(1, &[0], &[1], &[]));
        assert_eq!(rows[3], row(0, &[], &[0], &[]));
    }

    #[test]
    fn freed_lane_is_reused() {
        let rows = rows(&[("a", &[]), ("b", &[])]);
        assert_eq!((rows[0].column, rows[1].column), (0, 0));
    }

    #[test]
    fn parses_refs_and_subject_with_separators() {
        let commit = parse_commit("abc\0p q\0Ada L\x002 hours ago\0HEAD -> main, tag: v1\0fix: a, b").unwrap();
        assert_eq!(commit.parents, vec!["p", "q"]);
        assert_eq!(commit.refs, vec!["HEAD -> main", "tag: v1"]);
        assert_eq!(commit.subject, "fix: a, b");
        assert!(parse_commit("root\0\0Ada\0now\0\0s").unwrap().parents.is_empty());
    }
}
