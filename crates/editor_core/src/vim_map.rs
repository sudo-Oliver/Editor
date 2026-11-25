//! The user's own vim mappings, written like the map commands of a `.vimrc`:
//! `inoremap jk <Esc>`, `nnoremap H ^` or `nmap <leader>w :w<CR>`.

use std::time::{Duration, Instant};

use crate::vim::{Key, Mode};

/// How long an unfinished mapping waits for its next key, like `timeoutlen`.
pub const TIMEOUT: Duration = Duration::from_millis(1000);

/// A mapping that expands deeper than this runs nothing, like vim's
/// `maxmapdepth` and its "recursive mapping" error.
const MAX_DEPTH: usize = 100;

/// The modes a mapping can belong to. Command mode has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapMode {
    Normal,
    Visual,
    Insert,
}

impl MapMode {
    pub fn of(mode: Mode) -> Option<Self> {
        match mode {
            Mode::Normal => Some(Self::Normal),
            Mode::Visual | Mode::VisualLine => Some(Self::Visual),
            Mode::Insert => Some(Self::Insert),
            Mode::Command => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub modes: Vec<MapMode>,
    pub lhs: Vec<Key>,
    pub rhs: Vec<Key>,
    /// `map` rather than `noremap`: the keys of `rhs` are mapped again.
    pub remap: bool,
}

/// A line of the file that was left out, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// One-based.
    pub line: usize,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mappings {
    mappings: Vec<Mapping>,
}

impl Mappings {
    /// Reads map commands and `let mapleader`. The leader is `Space` unless
    /// the file names another, since the built-in commands use `Space` too.
    pub fn parse(text: &str) -> (Self, Vec<Problem>) {
        let mut mappings: Vec<Mapping> = Vec::new();
        let mut problems = Vec::new();
        let mut leader = vec![Key::Char(' ')];
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('"') {
                continue;
            }
            let problem = |message: String| Problem { line: index + 1, message };
            let (command, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            if command == "let" {
                match parse_leader(rest) {
                    Some(keys) => leader = keys,
                    None => problems.push(problem(format!("only `let mapleader` is supported: {line}"))),
                }
                continue;
            }
            let Some((modes, remap)) = map_command(command) else {
                problems.push(problem(format!("not a map command: {line}")));
                continue;
            };
            let mut rest = rest.trim_start();
            while let Some(after) = ["<silent>", "<nowait>", "<buffer>", "<unique>"].iter().find_map(|option| strip_prefix_ignore_case(rest, option)) {
                rest = after.trim_start();
            }
            let (lhs, rhs) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let (lhs, rhs) = (parse_keys(lhs, &leader), parse_keys(rhs.trim(), &leader));
            if lhs.is_empty() || rhs.is_empty() {
                problems.push(problem(format!("a mapping needs keys and what they do: {line}")));
                continue;
            }
            // A later mapping of the same keys wins, like in vim.
            for mode in &modes {
                for mapping in &mut mappings {
                    if mapping.lhs == lhs {
                        mapping.modes.retain(|other| other != mode);
                    }
                }
            }
            mappings.retain(|mapping| !mapping.modes.is_empty());
            mappings.push(Mapping { modes, lhs, rhs, remap });
        }
        (Self { mappings }, problems)
    }

    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }

    pub fn len(&self) -> usize {
        self.mappings.len()
    }

    fn in_mode(&self, mode: MapMode) -> impl Iterator<Item = &Mapping> {
        self.mappings.iter().filter(move |mapping| mapping.modes.contains(&mode))
    }

    fn exact(&self, mode: MapMode, keys: &[Key]) -> Option<&Mapping> {
        self.in_mode(mode).find(|mapping| mapping.lhs == keys)
    }

    /// Whether some mapping is longer than `keys` and starts with them.
    fn extends(&self, mode: MapMode, keys: &[Key]) -> bool {
        self.in_mode(mode).any(|mapping| mapping.lhs.len() > keys.len() && mapping.lhs.starts_with(keys))
    }

    /// The keys a mapping stands for, with its `rhs` mapped again for `map`,
    /// or `None` when it maps itself without end.
    fn expand(&self, mapping: &Mapping, mode: MapMode) -> Option<Vec<Key>> {
        let mut out = Vec::new();
        self.expand_into(mapping, mode, 0, &mut out).then_some(out)
    }

    fn expand_into(&self, mapping: &Mapping, mode: MapMode, depth: usize, out: &mut Vec<Key>) -> bool {
        if depth >= MAX_DEPTH {
            return false;
        }
        if !mapping.remap {
            out.extend(&mapping.rhs);
            return true;
        }
        let mut rest = mapping.rhs.as_slice();
        while !rest.is_empty() {
            // The longest mapping at the start of what is left. A mapping whose
            // right side starts with its own keys does not map them again.
            let found = (1..=rest.len()).rev().find_map(|length| {
                let keys = &rest[..length];
                self.exact(mode, keys).filter(|inner| !(inner.lhs == mapping.lhs && rest.len() == mapping.rhs.len())).map(|inner| (inner, length))
            });
            match found {
                Some((inner, length)) => {
                    if !self.expand_into(inner, mode, depth + 1, out) {
                        return false;
                    }
                    rest = &rest[length..];
                }
                None => {
                    out.push(rest[0]);
                    rest = &rest[1..];
                }
            }
        }
        true
    }
}

/// What the mapper did with a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Keys to run now, in order, instead of the key. Empty when the key
    /// starts a mapping.
    Run(Vec<Key>),
    /// In insert mode: the key types itself as usual.
    Type,
    /// In insert mode: a mapping ended with the key. The editor removes the
    /// `erase` characters typed before it and runs `keys` instead.
    Insert { erase: usize, keys: Vec<Key> },
}

/// Collects the keys of an unfinished mapping. In normal and visual mode they
/// wait; in insert mode they are typed at once and taken back when the mapping
/// is complete, so that typing never lags.
#[derive(Debug, Default)]
pub struct Mapper {
    pending: Vec<Key>,
    mode: Option<MapMode>,
    last: Option<Instant>,
}

impl Mapper {
    /// Whether keys wait for more, so that a timer should call `timeout`.
    pub fn is_waiting(&self) -> bool {
        !self.pending.is_empty() && self.mode != Some(MapMode::Insert)
    }

    /// The waiting keys, for the status bar.
    pub fn pending(&self) -> &[Key] {
        if self.is_waiting() { &self.pending } else { &[] }
    }

    /// Forgets waiting keys, after a mouse click for example.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.mode = None;
    }

    pub fn feed(&mut self, mappings: &Mappings, mode: Mode, key: Key, now: Instant) -> Step {
        let Some(map_mode) = MapMode::of(mode).filter(|_| !mappings.is_empty()) else {
            let mut keys = self.flush(mappings);
            // While inserting, a key types itself, as it does with mappings:
            // the editor's own typing brings suggestions and the input method.
            if keys.is_empty() && mode == Mode::Insert {
                return Step::Type;
            }
            keys.push(key);
            return Step::Run(keys);
        };
        let mut before = Vec::new();
        let late = self.last.is_some_and(|last| now.duration_since(last) > TIMEOUT);
        if self.mode != Some(map_mode) || late {
            before = self.flush(mappings);
        }
        self.mode = Some(map_mode);
        self.last = Some(now);
        if map_mode == MapMode::Insert {
            return self.feed_insert(mappings, key, before);
        }
        self.pending.push(key);
        before.extend(self.resolve(mappings, map_mode, false));
        Step::Run(before)
    }

    /// Runs the waiting keys once nothing followed them in time.
    pub fn timeout(&mut self, mappings: &Mappings, now: Instant) -> Vec<Key> {
        if self.last.is_some_and(|last| now.duration_since(last) >= TIMEOUT) { self.flush(mappings) } else { Vec::new() }
    }

    fn flush(&mut self, mappings: &Mappings) -> Vec<Key> {
        let mode = self.mode.take();
        match mode {
            Some(MapMode::Insert) | None => {
                // Typed already.
                self.pending.clear();
                Vec::new()
            }
            Some(mode) => self.resolve(mappings, mode, true),
        }
    }

    /// Runs what the waiting keys come to. Without `finish`, keys that may
    /// still become a longer mapping keep waiting.
    fn resolve(&mut self, mappings: &Mappings, mode: MapMode, finish: bool) -> Vec<Key> {
        let mut out = Vec::new();
        while !self.pending.is_empty() {
            if !finish && mappings.extends(mode, &self.pending) {
                break;
            }
            // The longest mapping at the start, or else the first key as it is.
            let found = (1..=self.pending.len()).rev().find_map(|length| mappings.exact(mode, &self.pending[..length]).map(|mapping| (mapping, length)));
            match found {
                Some((mapping, length)) => {
                    out.extend(mappings.expand(mapping, mode).unwrap_or_default());
                    self.pending.drain(..length);
                }
                None => out.push(self.pending.remove(0)),
            }
        }
        if self.pending.is_empty() {
            self.mode = None;
        }
        out
    }

    fn feed_insert(&mut self, mappings: &Mappings, key: Key, before: Vec<Key>) -> Step {
        self.pending.push(key);
        // Drop keys from the front until the rest can still become a mapping.
        while !self.pending.is_empty()
            && !mappings.extends(MapMode::Insert, &self.pending)
            && mappings.exact(MapMode::Insert, &self.pending).is_none()
        {
            self.pending.remove(0);
        }
        let mut keys = before;
        if mappings.extends(MapMode::Insert, &self.pending) || self.pending.is_empty() {
            if keys.is_empty() {
                return Step::Type;
            }
            // Keys that waited in another mode go first; in insert mode, vim types the key.
            keys.push(key);
            return Step::Run(keys);
        }
        let mapping = mappings.exact(MapMode::Insert, &self.pending).expect("checked above");
        let erase = self.pending[..self.pending.len() - 1].iter().filter(|key| matches!(key, Key::Char(_))).count();
        keys.extend(mappings.expand(mapping, MapMode::Insert).unwrap_or_default());
        self.reset();
        Step::Insert { erase, keys }
    }
}

fn map_command(command: &str) -> Option<(Vec<MapMode>, bool)> {
    use MapMode::*;
    Some(match command {
        "map" => (vec![Normal, Visual], true),
        "nmap" => (vec![Normal], true),
        "vmap" | "xmap" => (vec![Visual], true),
        "imap" => (vec![Insert], true),
        "noremap" | "no" => (vec![Normal, Visual], false),
        "nnoremap" | "nno" => (vec![Normal], false),
        "vnoremap" | "vno" | "xnoremap" | "xno" => (vec![Visual], false),
        "inoremap" | "ino" => (vec![Insert], false),
        _ => return None,
    })
}

/// The keys of `let mapleader = ","` or `let mapleader = "\<Space>"`.
fn parse_leader(rest: &str) -> Option<Vec<Key>> {
    let value = rest.trim().strip_prefix("mapleader")?.trim_start().strip_prefix('=')?.trim();
    let quote = value.chars().next().filter(|ch| *ch == '"' || *ch == '\'')?;
    let inner = value[1..].strip_suffix(quote)?;
    let inner = inner.strip_prefix('\\').filter(|rest| rest.starts_with('<')).unwrap_or(inner);
    let keys = parse_keys(inner, &[]);
    (!keys.is_empty()).then_some(keys)
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

/// Vim's key notation: plain characters, and names such as `<Esc>`, `<CR>`,
/// `<Space>`, `<Leader>` or `<C-s>` in any case. Other `<` stay a character.
pub fn parse_keys(text: &str, leader: &[Key]) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if ch == '<'
            && let Some(end) = rest.find('>')
            && let Some(named) = named_key(&rest[1..end], leader)
        {
            keys.extend(named);
            rest = &rest[end + 1..];
            continue;
        }
        keys.push(Key::Char(ch));
        rest = &rest[ch.len_utf8()..];
    }
    keys
}

fn named_key(name: &str, leader: &[Key]) -> Option<Vec<Key>> {
    let lower = name.to_ascii_lowercase();
    let key = match lower.as_str() {
        "esc" => Key::Esc,
        "cr" | "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "bs" => Key::Backspace,
        "del" => Key::Delete,
        "space" => Key::Char(' '),
        "lt" => Key::Char('<'),
        "bar" => Key::Char('|'),
        "bslash" => Key::Char('\\'),
        "leader" => return Some(leader.to_vec()),
        _ => {
            let letter = lower.strip_prefix("c-")?;
            let mut chars = letter.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) if ch.is_ascii_graphic() => Key::Ctrl(ch),
                _ => return None,
            }
        }
    };
    Some(vec![key])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Language,
        state::EditorState,
        vim::{Input, Vim},
    };

    fn keys(text: &str) -> Vec<Key> {
        parse_keys(text, &[Key::Char(' ')])
    }

    fn feed_all(mapper: &mut Mapper, mappings: &Mappings, mode: Mode, text: &str, now: Instant) -> Vec<Key> {
        let mut out = Vec::new();
        for key in keys(text) {
            match mapper.feed(mappings, mode, key, now) {
                Step::Run(run) => out.extend(run),
                Step::Type | Step::Insert { .. } => panic!("not in insert mode"),
            }
        }
        out
    }

    #[test]
    fn notation_names_keys() {
        assert_eq!(parse_keys("<Esc>:w<CR><c-S><lt>x<Space>", &[]), vec![Key::Esc, Key::Char(':'), Key::Char('w'), Key::Enter, Key::Ctrl('s'), Key::Char('<'), Key::Char('x'), Key::Char(' ')]);
        assert_eq!(parse_keys("a<b", &[]), keys("a<b"));
        assert_eq!(parse_keys("<leader>w", &[Key::Char(',')]), keys(",w"));
    }

    #[test]
    fn the_file_is_read_like_a_vimrc() {
        let text = "\" comment\nlet mapleader = \",\"\nnnoremap <silent> <leader>w :w<CR>\ninoremap jk <Esc>\nmap H ^\nset number\nnmap x\n";
        let (mappings, problems) = Mappings::parse(text);
        assert_eq!(mappings.mappings.len(), 3);
        assert_eq!(mappings.mappings[0], Mapping { modes: vec![MapMode::Normal], lhs: keys(",w"), rhs: keys(":w<CR>"), remap: false });
        assert_eq!(mappings.mappings[2].modes, vec![MapMode::Normal, MapMode::Visual]);
        assert_eq!(problems.iter().map(|problem| problem.line).collect::<Vec<_>>(), [6, 7]);
    }

    #[test]
    fn the_leader_can_be_space_by_name() {
        let (mappings, _) = Mappings::parse("let mapleader = \"\\<Space>\"\nnnoremap <Leader>q :q<CR>\n");
        assert_eq!(mappings.mappings[0].lhs, keys(" q"));
    }

    #[test]
    fn a_later_mapping_replaces_an_earlier_one() {
        let (mappings, _) = Mappings::parse("nnoremap x a\nnnoremap x b\n");
        assert_eq!(mappings.mappings.len(), 1);
        assert_eq!(mappings.mappings[0].rhs, keys("b"));
    }

    #[test]
    fn normal_mode_keys_wait_for_a_longer_mapping() {
        let (mappings, _) = Mappings::parse("nnoremap <leader>w :w<CR>\n");
        let mut mapper = Mapper::default();
        let now = Instant::now();
        assert_eq!(mapper.feed(&mappings, Mode::Normal, Key::Char(' '), now), Step::Run(Vec::new()));
        assert!(mapper.is_waiting());
        assert_eq!(mapper.feed(&mappings, Mode::Normal, Key::Char('w'), now), Step::Run(keys(":w<CR>")));
        // A built-in leader command still gets its keys.
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, " f", now), keys(" f"));
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "j", now), keys("j"));
    }

    #[test]
    fn waiting_keys_run_after_the_timeout() {
        let (mappings, _) = Mappings::parse("nnoremap gx dd\nnnoremap g ~\n");
        let mut mapper = Mapper::default();
        let now = Instant::now();
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "g", now), Vec::new());
        assert_eq!(mapper.timeout(&mappings, now), Vec::new());
        // The shorter mapping wins once the longer one cannot come.
        assert_eq!(mapper.timeout(&mappings, now + TIMEOUT), keys("~"));
        assert!(!mapper.is_waiting());
        // A late key does not finish the mapping either.
        feed_all(&mut mapper, &mappings, Mode::Normal, "g", now);
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "x", now + TIMEOUT * 2), keys("~x"));
    }

    #[test]
    fn map_maps_its_keys_again_but_noremap_does_not() {
        let (mappings, _) = Mappings::parse("nnoremap a b\nnmap c a\nnnoremap d a\nnmap j jzz\n");
        let mut mapper = Mapper::default();
        let now = Instant::now();
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "c", now), keys("b"));
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "d", now), keys("a"));
        // A mapping that starts with its own keys does not map them again.
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "j", now), keys("jzz"));
    }

    #[test]
    fn endless_mappings_run_nothing() {
        let (mappings, _) = Mappings::parse("nmap a b\nnmap b a\nnmap j gj\n");
        let mut mapper = Mapper::default();
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "a", Instant::now()), Vec::new());
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "j", Instant::now()), Vec::new());
    }

    #[test]
    fn without_mappings_inserting_types_as_usual() {
        let mut mapper = Mapper::default();
        let now = Instant::now();
        assert_eq!(mapper.feed(&Mappings::default(), Mode::Insert, Key::Char('\\'), now), Step::Type);
        assert_eq!(mapper.feed(&Mappings::default(), Mode::Normal, Key::Char('j'), now), Step::Run(vec![Key::Char('j')]));
    }

    #[test]
    fn insert_mode_takes_back_the_typed_keys() {
        let (mappings, _) = Mappings::parse("inoremap jk <Esc>\ninoremap <C-s> <Esc>:w<CR>\n");
        let mut mapper = Mapper::default();
        let now = Instant::now();
        assert_eq!(mapper.feed(&mappings, Mode::Insert, Key::Char('j'), now), Step::Type);
        assert!(!mapper.is_waiting());
        assert_eq!(mapper.feed(&mappings, Mode::Insert, Key::Char('k'), now), Step::Insert { erase: 1, keys: vec![Key::Esc] });
        // `jjk` still ends in `jk`.
        for key in keys("jj") {
            mapper.feed(&mappings, Mode::Insert, key, now);
        }
        assert_eq!(mapper.feed(&mappings, Mode::Insert, Key::Char('k'), now), Step::Insert { erase: 1, keys: vec![Key::Esc] });
        assert_eq!(mapper.feed(&mappings, Mode::Insert, Key::Ctrl('s'), now), Step::Insert { erase: 0, keys: keys("<Esc>:w<CR>") });
        // Too slow is no mapping.
        mapper.feed(&mappings, Mode::Insert, Key::Char('j'), now);
        assert_eq!(mapper.feed(&mappings, Mode::Insert, Key::Char('k'), now + TIMEOUT * 2), Step::Type);
    }

    /// Like the editor: typed text goes in directly, other keys through vim.
    fn type_with(mappings: &Mappings, text: &str) -> (EditorState, Vim) {
        let mut state = EditorState::new("", Language::Plain);
        let mut vim = Vim::default();
        let mut mapper = Mapper::default();
        let input = Input::default();
        for key in keys(text) {
            let step = mapper.feed(mappings, vim.mode(), key, Instant::now());
            let run = match step {
                Step::Type => {
                    if let Key::Char(ch) = key {
                        state.type_text(ch.encode_utf8(&mut [0; 4]));
                    }
                    continue;
                }
                Step::Run(run) => run,
                Step::Insert { erase, keys } => std::iter::repeat_n(Key::Backspace, erase).chain(keys).collect(),
            };
            for key in run {
                vim.key(key, &mut state, &input);
            }
        }
        (state, vim)
    }

    #[test]
    fn jk_leaves_insert_mode_without_a_trace() {
        let (mappings, _) = Mappings::parse("inoremap jk <Esc>\nnnoremap <leader>d dd\n");
        let (state, vim) = type_with(&mappings, "oabjk");
        assert_eq!(state.buffer().to_string(), "\nab");
        assert_eq!(vim.mode(), Mode::Normal);
        let (state, _) = type_with(&mappings, "oabjk d");
        assert_eq!(state.buffer().to_string(), "");
        // A lone `j` stays text.
        let (state, vim) = type_with(&mappings, "ojx");
        assert_eq!(state.buffer().to_string(), "\njx");
        assert_eq!(vim.mode(), Mode::Insert);
    }

    #[test]
    fn mappings_belong_to_their_mode() {
        let (mappings, _) = Mappings::parse("inoremap jk <Esc>\nvnoremap y \"+y\n");
        let mut mapper = Mapper::default();
        let now = Instant::now();
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Normal, "jk", now), keys("jk"));
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Visual, "y", now), keys("\"+y"));
        assert_eq!(feed_all(&mut mapper, &mappings, Mode::Command, "y", now), keys("y"));
    }
}
