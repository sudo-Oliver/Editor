//! Reads `.synctex.gz` files and maps between source lines and PDF positions.
//!
//! All public coordinates are PDF points with the origin at the top left of the page.

use std::{
    collections::HashMap,
    io::Read,
    path::{Component, Path, PathBuf},
};

use flate2::read::GzDecoder;

/// Scaled TeX points per PDF point: 65536 sp/pt * 72.27 pt/in / 72 bp/in.
const SP_PER_POINT: f32 = 65781.76;
/// Height of a highlight when the record has no box height.
const DEFAULT_HEIGHT: f32 = 10.0;
/// Width of a highlight for a line with a single record.
const MIN_WIDTH: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Node {
    page: usize,
    tag: u32,
    line: usize,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    depth: f32,
    is_box: bool,
}

/// A rectangle on a zero-based page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfLocation {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Debug, Default)]
pub struct SyncTex {
    inputs: HashMap<u32, PathBuf>,
    nodes: Vec<Node>,
    /// Leaf records by tag and line, then file order. Auto-follow looks up a line
    /// on the UI thread, and a long document has millions of records.
    leaves: Vec<u32>,
}

impl SyncTex {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut text = String::new();
        GzDecoder::new(std::fs::File::open(path)?).read_to_string(&mut text)?;
        Ok(Self::parse(&text))
    }

    pub fn parse(text: &str) -> Self {
        let mut synctex = Self::default();
        let mut page = 0usize;
        let mut in_content = false;
        for line in text.lines() {
            if !in_content {
                if let Some(rest) = line.strip_prefix("Input:")
                    && let Some((tag, path)) = rest.split_once(':')
                    && let Ok(tag) = tag.parse()
                {
                    synctex.inputs.insert(tag, normalize(Path::new(path)));
                } else if line == "Content:" {
                    in_content = true;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("Input:") {
                // Inputs can also appear inside the content section.
                if let Some((tag, path)) = rest.split_once(':')
                    && let Ok(tag) = tag.parse()
                {
                    synctex.inputs.insert(tag, normalize(Path::new(path)));
                }
                continue;
            }
            let Some(kind) = line.chars().next() else {
                continue;
            };
            if kind == '{' {
                if let Ok(number) = line[1..].parse::<usize>() {
                    page = number.saturating_sub(1);
                }
                continue;
            }
            if !matches!(kind, '[' | '(' | 'h' | 'v' | 'x' | 'k' | 'g' | '$') {
                continue;
            }
            if let Some(node) = parse_node(kind, &line[1..], page) {
                synctex.nodes.push(node);
            }
        }
        // Boxes span whole paragraphs and pages, so only leaf records locate a line.
        synctex.leaves = (0..synctex.nodes.len() as u32).filter(|index| !synctex.nodes[*index as usize].is_box).collect();
        // A stable sort keeps the file order within a line.
        synctex.leaves.sort_by_key(|index| {
            let node = &synctex.nodes[*index as usize];
            (node.tag, node.line)
        });
        synctex
    }

    fn tag_of(&self, file: &Path) -> Option<u32> {
        let file = normalize(file);
        self.inputs
            .iter()
            .find(|(_, path)| **path == file)
            .map(|(tag, _)| *tag)
    }

    /// Finds where a one-based source line ends up. A line without output, such
    /// as a blank one, uses the next line of the file that has output.
    pub fn forward(&self, file: &Path, line: usize) -> Option<PdfLocation> {
        let tag = self.tag_of(file)?;
        let node = |index: &u32| &self.nodes[*index as usize];
        let start = self.leaves.partition_point(|index| node(index).tag < tag);
        let end = self.leaves.partition_point(|index| node(index).tag <= tag);
        let in_file = &self.leaves[start..end];
        let next = in_file.partition_point(|index| node(index).line < line);
        let target = in_file.get(next).or(in_file.last()).map(|index| node(index).line)?;
        let first_of_line = in_file.partition_point(|index| node(index).line < target);
        let past_line = in_file.partition_point(|index| node(index).line <= target);
        let records = in_file[first_of_line..past_line].iter().map(node);
        let first = node(&in_file[first_of_line]);
        // Union of the records of this line that share the first record's row.
        let same_row = records.filter(|node| node.page == first.page && (node.y - first.y).abs() < 1.0);
        let (mut left, mut right) = (f32::MAX, f32::MIN);
        let mut height = 0f32;
        for node in same_row {
            left = left.min(node.x);
            // Widths of glue and kerns are not text extents, so only positions count.
            right = right.max(node.x);
            height = height.max(node.height);
        }
        let height = if height > 0.0 { height } else { DEFAULT_HEIGHT };
        Some(PdfLocation {
            page: first.page,
            x: left,
            y: first.y - height,
            width: (right - left).max(MIN_WIDTH),
            height,
        })
    }

    /// Finds the source line for a position on a zero-based page.
    pub fn backward(&self, page: usize, x: f32, y: f32) -> Option<SourceLocation> {
        let on_page = || self.nodes.iter().filter(|node| node.page == page);
        // The tightest box under the point gives the row.
        let row = on_page()
            .filter(|node| node.is_box && node.contains(x, y))
            .min_by(|left, right| left.area().total_cmp(&right.area()));
        let best = match row {
            Some(row) => on_page()
                .filter(|node| !node.is_box && row.contains_row(node))
                .min_by(|left, right| (left.x - x).abs().total_cmp(&(right.x - x).abs()))
                .unwrap_or(row),
            None => on_page().min_by(|left, right| left.distance(x, y).total_cmp(&right.distance(x, y)))?,
        };
        Some(SourceLocation {
            file: self.inputs.get(&best.tag)?.clone(),
            line: best.line,
        })
    }
}

impl Node {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x
            && x <= self.x + self.width
            && y >= self.y - self.height
            && y <= self.y + self.depth
    }

    fn contains_row(&self, other: &Node) -> bool {
        other.x >= self.x
            && other.x <= self.x + self.width
            && other.y >= self.y - self.height
            && other.y <= self.y + self.depth
    }

    fn area(&self) -> f32 {
        self.width * (self.height + self.depth)
    }

    fn distance(&self, x: f32, y: f32) -> f32 {
        (self.x - x).powi(2) + (self.y - y).powi(2)
    }
}

/// Parses `tag,line:x,y` and the optional `:width,height,depth`.
fn parse_node(kind: char, record: &str, page: usize) -> Option<Node> {
    let mut parts = record.split(':');
    let (tag, line) = parts.next()?.split_once(',')?;
    let (x, y) = parts.next()?.split_once(',')?;
    let mut size = parts
        .next()
        .into_iter()
        .flat_map(|size| size.split(','))
        .map(|value| value.parse::<f32>().unwrap_or(0.0) / SP_PER_POINT);
    Some(Node {
        page,
        tag: tag.parse().ok()?,
        line: line.parse().ok()?,
        x: x.parse::<f32>().ok()? / SP_PER_POINT,
        y: y.parse::<f32>().ok()? / SP_PER_POINT,
        width: size.next().unwrap_or(0.0),
        height: size.next().unwrap_or(0.0),
        depth: size.next().unwrap_or(0.0),
        is_box: matches!(kind, '[' | '(' | 'h' | 'v'),
    })
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "SyncTeX Version:1\nInput:1:/p/./main.tex\nInput:6:/p/./sub/part.tex\nOutput:pdf\nContent:\n!678\n{1\n[1,8:4736286,46220574:26673152,41484288,0\n(1,3:8799518,8865054:22609920,455111,0\nx1,3:11257121,8865054\ng1,3:11475574,8865054\n)\n(6,1:8799518,10000000:22609920,455111,0\nx6,1:9000000,10000000\n)\n]\n}1\n{2\n(1,9:8799518,8865054:22609920,455111,0\nx1,9:9000000,8865054\n)\n}2\n";

    #[test]
    fn forward_finds_page_and_row_in_included_file() {
        let synctex = SyncTex::parse(SAMPLE);
        let location = synctex.forward(Path::new("/p/sub/part.tex"), 1).unwrap();
        assert_eq!(location.page, 0);
        assert!((location.y + location.height - 10000000.0 / SP_PER_POINT).abs() < 0.01);
    }

    #[test]
    fn forward_uses_next_line_with_output() {
        let synctex = SyncTex::parse(SAMPLE);
        assert_eq!(synctex.forward(Path::new("/p/main.tex"), 5).unwrap().page, 1);
    }

    #[test]
    fn forward_past_the_end_uses_the_last_line_of_that_file() {
        let synctex = SyncTex::parse(SAMPLE);
        assert_eq!(synctex.forward(Path::new("/p/main.tex"), 100).unwrap().page, 1);
        assert_eq!(synctex.forward(Path::new("/p/sub/part.tex"), 100).unwrap().page, 0);
    }

    #[test]
    fn forward_joins_the_records_of_a_row_in_file_order() {
        let text = "Input:1:/a.tex\nContent:\n{1\nx1,2:100,700\nx1,3:50,700\nx1,2:300,700\nx1,2:200,9000000\n}1\n";
        let location = SyncTex::parse(text).forward(Path::new("/a.tex"), 2).unwrap();
        assert!((location.x - 100.0 / SP_PER_POINT).abs() < 0.01);
        assert_eq!(location.width, MIN_WIDTH);
    }

    #[test]
    #[ignore = "prints a timing, run with --ignored --nocapture --release"]
    fn forward_timing_on_a_long_document() {
        let mut text = String::from("Input:1:/a.tex\nContent:\n{1\n");
        for record in 0..2_000_000 {
            text.push_str(&format!("x1,{}:{},{}\n", record / 20, record, record));
        }
        let synctex = SyncTex::parse(&text);
        let start = std::time::Instant::now();
        for line in 0..1000 {
            assert!(synctex.forward(Path::new("/a.tex"), line * 90).is_some());
        }
        println!("forward: {:?} per lookup", start.elapsed() / 1000);
    }

    #[test]
    fn backward_returns_line_under_point() {
        let synctex = SyncTex::parse(SAMPLE);
        let x = 9100000.0 / SP_PER_POINT;
        let y = 10000000.0 / SP_PER_POINT - 2.0;
        let location = synctex.backward(0, x, y).unwrap();
        assert_eq!(location, SourceLocation { file: "/p/sub/part.tex".into(), line: 1 });
    }

    #[test]
    fn unknown_file_has_no_location() {
        assert!(SyncTex::parse(SAMPLE).forward(Path::new("/other.tex"), 1).is_none());
    }
}
