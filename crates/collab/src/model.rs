use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use editor_core::TextChange;
use serde::{Deserialize, Serialize};
use yrs::{
    Any, Assoc, ClientID, Doc, GetString, IndexedSequence, Map, MapRef, Observable, OffsetKind, Options, Out, ReadTxn, StateVector, StickyIndex, Text,
    TextRef, Transact, TransactionMut,
    types::Delta,
    updates::{decoder::Decode, encoder::Encode},
};

/// Origin of transactions made here, so that observers skip them.
const LOCAL: &str = "local";

/// Origin of text changes made by a review here, which the local editor has
/// to apply like changes from someone else.
const REVIEW: &str = "review";

/// Changes from other editors, per file, in the order they apply.
pub type RemoteChanges = Vec<(Arc<str>, Vec<TextChange>)>;

/// A note on a range of a file, or a reply to one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub path: String,
    /// Where the text is now. Empty once the text is gone.
    pub range: Range<usize>,
    pub author: String,
    pub color: u32,
    pub text: String,
    /// Seconds since the Unix epoch.
    pub created: u64,
    pub resolved: bool,
    /// The comment this one answers.
    pub parent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    Inserted,
    /// Struck through, with the text it covers, which stays until accepted.
    Deleted(String),
}

/// An edit made while changes are tracked, waiting for a decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub id: String,
    pub path: String,
    /// The inserted or the struck text.
    pub range: Range<usize>,
    pub author: String,
    pub color: u32,
    pub kind: SuggestionKind,
    pub created: u64,
}

/// One step of a tracked edit, in the coordinates of the shared text right
/// before it. Deletions only strike text through until someone decides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackedOp {
    Insert { at: usize, text: String },
    /// Text that goes away for good, inside one's own suggested insertion.
    Remove { range: Range<usize> },
    /// Text suggested for deletion, which stays until accepted.
    Strike { range: Range<usize> },
}

/// Who makes a comment or a suggestion.
#[derive(Clone, Debug)]
pub struct Author {
    pub name: String,
    pub color: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredComment {
    path: String,
    start: Vec<u8>,
    end: Vec<u8>,
    author: String,
    color: u32,
    text: String,
    created: u64,
    resolved: bool,
    parent: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredSuggestion {
    path: String,
    start: Vec<u8>,
    end: Option<Vec<u8>>,
    author: String,
    color: u32,
    /// Set for a deletion. The text it covers stays in the file.
    deleted: Option<String>,
    created: u64,
}

/// The shared state of one project: the text of every shared file, files
/// that are not text, comments and tracked changes.
pub struct Project {
    doc: Doc,
    /// Lists the shared paths, since root texts cannot be listed.
    files: MapRef,
    /// Images and other files that are not text, by path.
    blobs: MapRef,
    comments: MapRef,
    suggestions: MapRef,
    texts: HashMap<Arc<str>, TextRef>,
    known_blobs: std::collections::HashSet<String>,
    remote: Arc<Mutex<RemoteChanges>>,
    /// Comments or suggestions changed since the last `take_annotations_changed`.
    annotations_changed: Arc<AtomicBool>,
    next_id: std::cell::Cell<u64>,
}

impl Project {
    pub fn new(client_id: u64) -> Self {
        let doc = Doc::with_options(Options {
            client_id: ClientID::new(client_id & ((1 << 53) - 1)),
            offset_kind: OffsetKind::Bytes,
            ..Options::default()
        });
        let files = doc.get_or_insert_map("files");
        let blobs = doc.get_or_insert_map("blobs");
        let comments = doc.get_or_insert_map("comments");
        let suggestions = doc.get_or_insert_map("suggestions");
        let annotations_changed = Arc::new(AtomicBool::new(false));
        for map in [&comments, &suggestions] {
            let changed = annotations_changed.clone();
            map.observe(LOCAL, move |_, _| changed.store(true, Ordering::Relaxed));
        }
        Self {
            doc,
            files,
            blobs,
            comments,
            suggestions,
            texts: HashMap::new(),
            known_blobs: Default::default(),
            remote: Arc::default(),
            annotations_changed,
            next_id: std::cell::Cell::new(0),
        }
    }

    /// Shares a file with this text. Returns the update for the others. A
    /// file someone already shared keeps its text, which may have moved on.
    pub fn add_file(&mut self, path: &str, content: &str) -> Vec<u8> {
        let text = self.text(path);
        let mut txn = self.doc.transact_mut_with(LOCAL);
        if self.files.contains_key(&txn, path) {
            return Vec::new();
        }
        self.files.insert(&mut txn, path, true);
        text.insert(&mut txn, 0, content);
        txn.encode_update_v1()
    }

    /// Shares a file that is not text, such as an image, as it is now.
    pub fn add_blob(&mut self, path: &str, bytes: Vec<u8>) -> Vec<u8> {
        self.known_blobs.insert(path.to_string());
        let mut txn = self.doc.transact_mut_with(LOCAL);
        self.blobs.insert(&mut txn, path, Any::Buffer(bytes.into()));
        txn.encode_update_v1()
    }

    pub fn paths(&self) -> Vec<String> {
        let txn = self.doc.transact();
        let mut paths: Vec<String> = self.files.keys(&txn).map(str::to_string).collect();
        paths.sort();
        paths
    }

    pub fn content(&mut self, path: &str) -> String {
        let text = self.text(path);
        text.get_string(&self.doc.transact())
    }

    /// Records local edits to a file. Returns the update for the others.
    pub fn edit(&mut self, path: &str, changes: &[TextChange]) -> Vec<u8> {
        let text = self.text(path);
        let mut txn = self.doc.transact_mut_with(LOCAL);
        for change in changes {
            if change.removed > 0 {
                text.remove_range(&mut txn, change.start as u32, change.removed as u32);
            }
            if !change.inserted.is_empty() {
                text.insert(&mut txn, change.start as u32, &change.inserted);
            }
        }
        txn.encode_update_v1()
    }

    /// Records edits made while changes are tracked: insertions become
    /// suggestions, and deletions only strike text through. Returns the
    /// update for the others.
    pub fn edit_tracked(&mut self, path: &str, ops: &[TrackedOp], author: &Author) -> Vec<u8> {
        let text = self.text(path);
        let mut txn = self.doc.transact_mut_with(LOCAL);
        for op in ops {
            match op {
                TrackedOp::Insert { at, text: inserted } => {
                    let start = *at as u32;
                    // Typing on at the end of one's own insertion grows it.
                    let grows = self
                        .own_suggestions(&txn, path, author)
                        .into_iter()
                        .find(|(_, stored, range)| stored.deleted.is_none() && range.end == start && range.start < range.end);
                    text.insert(&mut txn, start, inserted);
                    let end = start + inserted.len() as u32;
                    match grows {
                        Some((id, mut stored, _)) => {
                            stored.end = Some(sticky(&text, &txn, end, Assoc::After));
                            self.store_suggestion(&mut txn, &id, &stored);
                        }
                        None => {
                            let stored = StoredSuggestion {
                                path: path.to_string(),
                                start: sticky(&text, &txn, start, Assoc::After),
                                end: Some(sticky(&text, &txn, end, Assoc::After)),
                                author: author.name.clone(),
                                color: author.color,
                                deleted: None,
                                created: now(),
                            };
                            let id = self.new_id();
                            self.store_suggestion(&mut txn, &id, &stored);
                        }
                    }
                }
                TrackedOp::Remove { range } => text.remove_range(&mut txn, range.start as u32, range.len() as u32),
                TrackedOp::Strike { range } => {
                    let (mut start, mut end) = (range.start as u32, range.end as u32);
                    // Deleting next to one's own struck text makes one deletion of it.
                    for (id, stored, other) in self.own_suggestions(&txn, path, author) {
                        if stored.deleted.is_some() && other.start <= end && start <= other.end {
                            start = start.min(other.start);
                            end = end.max(other.end);
                            self.suggestions.remove(&mut txn, &id);
                        }
                    }
                    if start < end {
                        let stored = StoredSuggestion {
                            path: path.to_string(),
                            start: sticky(&text, &txn, start, Assoc::After),
                            // Text typed right after it does not join the deletion.
                            end: Some(sticky(&text, &txn, end, Assoc::Before)),
                            author: author.name.clone(),
                            color: author.color,
                            deleted: Some(String::new()),
                            created: now(),
                        };
                        let id = self.new_id();
                        self.store_suggestion(&mut txn, &id, &stored);
                    }
                }
            }
        }
        txn.encode_update_v1()
    }

    /// Keeps a suggestion: an insertion stays, struck text goes. A text
    /// change shows up in `take_remote` for the local editor too.
    pub fn accept(&mut self, id: &str) -> Vec<u8> {
        self.decide(id, true)
    }

    /// Takes a suggestion back: an insertion goes, struck text stays.
    pub fn reject(&mut self, id: &str) -> Vec<u8> {
        self.decide(id, false)
    }

    fn decide(&mut self, id: &str, accept: bool) -> Vec<u8> {
        let Some(stored) = self.suggestion(id) else {
            return Vec::new();
        };
        let text = self.text(&stored.path);
        let mut txn = self.doc.transact_mut_with(REVIEW);
        // Accepting a deletion and rejecting an insertion both remove the text.
        if accept == stored.deleted.is_some()
            && let Some(start) = offset(&txn, &stored.start)
            && let Some(end) = stored.end.as_ref().and_then(|end| offset(&txn, end)).filter(|end| *end > start)
        {
            text.remove_range(&mut txn, start, end - start);
        }
        self.suggestions.remove(&mut txn, id);
        txn.encode_update_v1()
    }

    pub fn suggestions(&self) -> Vec<Suggestion> {
        let txn = self.doc.transact();
        let mut contents: HashMap<String, String> = HashMap::new();
        let mut suggestions: Vec<Suggestion> = self
            .suggestions
            .iter(&txn)
            .filter_map(|(id, value)| {
                let stored: StoredSuggestion = decode(&value)?;
                let start = offset(&txn, &stored.start)? as usize;
                let end = stored.end.as_ref().and_then(|end| offset(&txn, end)).map_or(start, |end| (end as usize).max(start));
                // Nothing is left to decide on text that is all gone.
                if start == end {
                    return None;
                }
                let kind = match stored.deleted {
                    Some(_) => {
                        let content = contents
                            .entry(stored.path.clone())
                            .or_insert_with(|| txn.get_text(format!("file:{}", stored.path)).map(|text| text.get_string(&txn)).unwrap_or_default());
                        SuggestionKind::Deleted(slice(content, start, end - start))
                    }
                    None => SuggestionKind::Inserted,
                };
                Some(Suggestion {
                    id: id.to_string(),
                    path: stored.path,
                    range: start..end,
                    author: stored.author,
                    color: stored.color,
                    kind,
                    created: stored.created,
                })
            })
            .collect();
        suggestions.sort_by(|a, b| (&a.path, a.range.start, a.created).cmp(&(&b.path, b.range.start, b.created)));
        suggestions
    }

    /// Applies an update that may only suggest: the text outside suggested
    /// insertions has to stay as it was. The update stays applied either
    /// way, since a CRDT cannot take it back, so a caller that must not keep
    /// it works on a copy from `from_state`.
    pub fn apply_suggestions_only(&mut self, update: &[u8]) -> Result<(), String> {
        let before = self.reviewed_texts();
        self.apply(update)?;
        let after = self.reviewed_texts();
        for (path, text) in &after {
            if before.get(path) != Some(text) {
                return Err(format!("{path} changed outside of suggestions"));
            }
        }
        if before.len() != after.len() {
            return Err("A file was removed".to_string());
        }
        Ok(())
    }

    /// Every file's text with the suggested insertions left out, which is
    /// what someone who may only suggest must not change.
    fn reviewed_texts(&self) -> HashMap<String, String> {
        let mut texts: HashMap<String, String> = {
            let txn = self.doc.transact();
            self.files
                .keys(&txn)
                .map(|path| (path.to_string(), txn.get_text(format!("file:{path}")).map(|text| text.get_string(&txn)).unwrap_or_default()))
                .collect()
        };
        let mut insertions = self.suggestions();
        insertions.retain(|suggestion| suggestion.kind == SuggestionKind::Inserted);
        // From the end, so that cutting one does not move the next.
        insertions.sort_by_key(|suggestion| std::cmp::Reverse(suggestion.range.start));
        for suggestion in insertions {
            if let Some(text) = texts.get_mut(&suggestion.path)
                && suggestion.range.end <= text.len()
                && text.is_char_boundary(suggestion.range.start)
                && text.is_char_boundary(suggestion.range.end)
            {
                text.replace_range(suggestion.range.clone(), "");
            }
        }
        texts
    }

    /// The whole shared state, to keep on disk or hand to someone.
    pub fn encode_state(&self) -> Vec<u8> {
        self.doc.transact().encode_state_as_update_v1(&StateVector::default())
    }

    /// A project from `encode_state`. Its content counts as known, so it
    /// reports no changes of its own.
    pub fn from_state(client_id: u64, state: &[u8]) -> Result<Self, String> {
        let mut project = Self::new(client_id);
        project.apply(state)?;
        project.forget_events();
        Ok(project)
    }

    /// Drops the changes waiting to be taken, for a copy that nobody shows.
    pub fn forget_events(&mut self) {
        self.remote.lock().unwrap().clear();
        let txn = self.doc.transact();
        for path in self.blobs.keys(&txn) {
            if !self.known_blobs.contains(path) {
                self.known_blobs.insert(path.to_string());
            }
        }
        self.annotations_changed.store(false, Ordering::Relaxed);
    }

    /// Every shared file that is not text, with its bytes.
    pub fn blobs(&self) -> Vec<(String, Vec<u8>)> {
        let txn = self.doc.transact();
        self.blobs
            .iter(&txn)
            .filter_map(|(path, value)| match value {
                Out::Any(Any::Buffer(bytes)) => Some((path.to_string(), bytes.to_vec())),
                _ => None,
            })
            .collect()
    }

    /// Every shared text file with its text.
    pub fn files(&mut self) -> Vec<(String, String)> {
        self.paths().into_iter().map(|path| {
            let content = self.content(&path);
            (path, content)
        }).collect()
    }

    /// Comments on `range` of a file, or a reply when `parent` is set.
    pub fn add_comment(&mut self, path: &str, range: Range<usize>, text: &str, parent: Option<String>, author: &Author) -> Vec<u8> {
        let file = self.text(path);
        let mut txn = self.doc.transact_mut_with(LOCAL);
        let length = file.len(&txn);
        let start = (range.start as u32).min(length);
        let end = (range.end as u32).clamp(start, length);
        let stored = StoredComment {
            path: path.to_string(),
            start: sticky(&file, &txn, start, Assoc::After),
            // Text typed at the end of the range does not join it.
            end: sticky(&file, &txn, end, Assoc::Before),
            author: author.name.clone(),
            color: author.color,
            text: text.to_string(),
            created: now(),
            resolved: false,
            parent,
        };
        let id = self.new_id();
        self.comments.insert(&mut txn, id, encode(&stored));
        txn.encode_update_v1()
    }

    pub fn set_resolved(&mut self, id: &str, resolved: bool) -> Vec<u8> {
        let mut txn = self.doc.transact_mut_with(LOCAL);
        let stored = self.comments.get(&txn, id).and_then(|value| decode::<StoredComment>(&value));
        if let Some(mut stored) = stored {
            stored.resolved = resolved;
            self.comments.insert(&mut txn, id, encode(&stored));
        }
        txn.encode_update_v1()
    }

    /// Removes a comment with its replies.
    pub fn delete_comment(&mut self, id: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut_with(LOCAL);
        let replies: Vec<String> = self
            .comments
            .iter(&txn)
            .filter(|(_, value)| decode::<StoredComment>(value).is_some_and(|stored| stored.parent.as_deref() == Some(id)))
            .map(|(key, _)| key.to_string())
            .collect();
        for reply in replies {
            self.comments.remove(&mut txn, &reply);
        }
        self.comments.remove(&mut txn, id);
        txn.encode_update_v1()
    }

    pub fn comments(&self) -> Vec<Comment> {
        let txn = self.doc.transact();
        let mut comments: Vec<Comment> = self
            .comments
            .iter(&txn)
            .filter_map(|(id, value)| {
                let stored: StoredComment = decode(&value)?;
                let start = offset(&txn, &stored.start).unwrap_or(0) as usize;
                let end = offset(&txn, &stored.end).map_or(start, |end| (end as usize).max(start));
                Some(Comment {
                    id: id.to_string(),
                    path: stored.path,
                    range: start..end,
                    author: stored.author,
                    color: stored.color,
                    text: stored.text,
                    created: stored.created,
                    resolved: stored.resolved,
                    parent: stored.parent,
                })
            })
            .collect();
        comments.sort_by(|a, b| (&a.path, a.range.start, a.created).cmp(&(&b.path, b.range.start, b.created)));
        comments
    }

    /// Whether comments or suggestions changed since the last call.
    pub fn take_annotations_changed(&self) -> bool {
        self.annotations_changed.swap(false, Ordering::Relaxed)
    }

    /// Merges an update from another editor. The changes it made to the
    /// text wait in `take_remote`, new files that are not text in `take_blobs`.
    pub fn apply(&mut self, update: &[u8]) -> Result<(), String> {
        let update = yrs::Update::decode_v1(update).map_err(|error| error.to_string())?;
        let new_paths: Vec<Arc<str>> = {
            let mut txn = self.doc.transact_mut();
            txn.apply_update(update).map_err(|error| error.to_string())?;
            self.files.keys(&txn).filter(|path| !self.texts.contains_key(*path)).map(Arc::from).collect()
        };
        // A file shared by someone else arrives as a whole, even when empty.
        for path in new_paths {
            let content = self.content(&path);
            self.remote.lock().unwrap().push((
                path,
                vec![TextChange::new(0, 0, content)],
            ));
        }
        Ok(())
    }

    /// Files that are not text and arrived since the last call.
    pub fn take_blobs(&mut self) -> Vec<(String, Vec<u8>)> {
        let txn = self.doc.transact();
        let mut blobs = Vec::new();
        for (path, value) in self.blobs.iter(&txn) {
            if self.known_blobs.contains(path) {
                continue;
            }
            if let Out::Any(Any::Buffer(bytes)) = value {
                blobs.push((path.to_string(), bytes.to_vec()));
            }
        }
        for (path, _) in &blobs {
            self.known_blobs.insert(path.clone());
        }
        blobs
    }

    pub fn take_remote(&mut self) -> RemoteChanges {
        std::mem::take(&mut self.remote.lock().unwrap())
    }

    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// Everything that an editor with this state vector is missing.
    pub fn diff(&self, state_vector: &[u8]) -> Result<Vec<u8>, String> {
        let state_vector = StateVector::decode_v1(state_vector).map_err(|error| error.to_string())?;
        Ok(self.doc.transact().encode_diff_v1(&state_vector))
    }

    /// The root text of a file, watched for changes from other editors.
    fn text(&mut self, path: &str) -> TextRef {
        if let Some(text) = self.texts.get(path) {
            return text.clone();
        }
        let path: Arc<str> = path.into();
        let text = self.doc.get_or_insert_text(format!("file:{path}"));
        let remote = self.remote.clone();
        let key = path.clone();
        text.observe(LOCAL, move |txn, event| {
            if txn.origin().is_some_and(|origin| origin.as_ref() == LOCAL.as_bytes()) {
                return;
            }
            let changes = delta_to_changes(event.delta(txn));
            if !changes.is_empty() {
                remote.lock().unwrap().push((key.clone(), changes));
            }
        });
        self.texts.insert(path, text.clone());
        text
    }

    fn new_id(&self) -> String {
        self.next_id.set(self.next_id.get() + 1);
        format!("{}-{}", self.doc.client_id(), self.next_id.get())
    }

    fn suggestion(&self, id: &str) -> Option<StoredSuggestion> {
        let txn = self.doc.transact();
        decode(&self.suggestions.get(&txn, id)?)
    }

    /// The suggestions of `author` in a file, with where they are now.
    fn own_suggestions(&self, txn: &TransactionMut, path: &str, author: &Author) -> Vec<(String, StoredSuggestion, Range<u32>)> {
        self.suggestions
            .iter(txn)
            .filter_map(|(id, value)| {
                let stored: StoredSuggestion = decode(&value)?;
                if stored.path != path || stored.author != author.name {
                    return None;
                }
                let start = offset(txn, &stored.start)?;
                let end = stored.end.as_ref().and_then(|end| offset(txn, end)).unwrap_or(start).max(start);
                Some((id.to_string(), stored, start..end))
            })
            .collect()
    }

    fn store_suggestion(&self, txn: &mut TransactionMut, id: &str, stored: &StoredSuggestion) {
        self.suggestions.insert(txn, id, encode(stored));
    }
}

/// The end of the text has nothing after it to stick to, so a position there
/// sticks to what comes before.
fn sticky(text: &TextRef, txn: &TransactionMut, index: u32, assoc: Assoc) -> Vec<u8> {
    text.sticky_index(txn, index, assoc)
        .or_else(|| text.sticky_index(txn, index, Assoc::Before))
        .map(|sticky| sticky.encode_v1())
        .unwrap_or_default()
}

fn offset<T: ReadTxn>(txn: &T, sticky: &[u8]) -> Option<u32> {
    StickyIndex::decode_v1(sticky).ok()?.get_offset(txn).map(|offset| offset.index)
}

fn encode<T: Serialize>(value: &T) -> Any {
    Any::String(serde_json::to_string(value).unwrap().into())
}

fn decode<T: for<'de> Deserialize<'de>>(value: &Out) -> Option<T> {
    match value {
        Out::Any(Any::String(json)) => serde_json::from_str(json).ok(),
        _ => None,
    }
}

/// `length` bytes of `text` from `start`, or less where the text is shorter.
fn slice(text: &str, start: usize, length: usize) -> String {
    let start = start.min(text.len());
    let end = (start + length).min(text.len());
    text.get(start..end).unwrap_or_default().to_string()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_secs())
}

fn delta_to_changes(delta: &[Delta]) -> Vec<TextChange> {
    let mut changes = Vec::new();
    let mut position = 0usize;
    for part in delta {
        match part {
            Delta::Retain(length, _) => position += *length as usize,
            Delta::Deleted(length) => changes.push(TextChange::new(position, *length as usize, String::new())),
            Delta::Inserted(Out::Any(Any::String(text)), _) => {
                changes.push(TextChange::new(position, 0, text.to_string()));
                position += text.len();
            }
            Delta::Inserted(..) => {}
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(start: usize, removed: usize, inserted: &str) -> TextChange {
        TextChange::new(start, removed, inserted)
    }

    /// Plays the remote changes onto a plain string, as the editor would.
    fn replay(text: &mut String, changes: &[TextChange]) {
        for change in changes {
            text.replace_range(change.start..change.start + change.removed, &change.inserted);
        }
    }

    #[test]
    fn concurrent_edits_converge_and_arrive_as_changes() {
        let mut a = Project::new(1);
        let mut b = Project::new(2);
        let first = a.add_file("main.tex", "Hallo Welt");
        b.apply(&first).unwrap();
        let mut b_text = String::new();
        for (path, changes) in b.take_remote() {
            assert_eq!(&*path, "main.tex");
            replay(&mut b_text, &changes);
        }
        assert_eq!(b_text, "Hallo Welt");

        let mut a_text = "Hallo Welt".to_string();
        let a_changes = [change(5, 0, " schöne")];
        replay(&mut a_text, &a_changes);
        let from_a = a.edit("main.tex", &a_changes);
        let b_changes = [change(6, 4, "Erde"), change(10, 0, "!")];
        replay(&mut b_text, &b_changes);
        let from_b = b.edit("main.tex", &b_changes);

        a.apply(&from_b).unwrap();
        b.apply(&from_a).unwrap();
        for (_, changes) in a.take_remote() {
            replay(&mut a_text, &changes);
        }
        for (_, changes) in b.take_remote() {
            replay(&mut b_text, &changes);
        }
        assert_eq!(a_text, "Hallo schöne Erde!");
        assert_eq!(a_text, b_text);
        assert_eq!(a.content("main.tex"), b.content("main.tex"));
        assert!(a.take_remote().is_empty());
    }

    #[test]
    fn diff_brings_a_late_editor_up_to_date() {
        let mut a = Project::new(1);
        a.add_file("main.tex", "x");
        a.add_file("refs.bib", "@book{}");
        a.edit("main.tex", &[change(1, 0, "yz")]);
        let mut late = Project::new(3);
        late.apply(&a.diff(&late.state_vector()).unwrap()).unwrap();
        assert_eq!(late.paths(), ["main.tex", "refs.bib"]);
        assert_eq!(late.content("main.tex"), "xyz");
        assert_eq!(late.take_remote().len(), 2);
    }

    /// Three editors type at random and get each other's updates late and
    /// out of order. All end on the same text, in the CRDT and in the buffer.
    #[test]
    fn random_editing_converges_in_the_editor() {
        use editor_core::{EditorState, Language, Selection};

        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut random = move |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        let start = "\\section{Einleitung}\nText mit Ümlauten.\n";
        let mut peers: Vec<(Project, EditorState)> = (1..=3).map(|id| (Project::new(id), EditorState::new("", Language::Plain))).collect();
        let first = peers[0].0.add_file("main.tex", start);
        peers[0].1 = EditorState::new(start, Language::Plain);
        let mut inbox: Vec<Vec<Vec<u8>>> = vec![Vec::new(), vec![first.clone()], vec![first]];
        for (_, state) in &mut peers {
            state.set_change_log(true);
        }
        for round in 0..400 {
            let peer = random(3);
            let (project, state) = &mut peers[peer];
            let waiting = inbox[peer].len();
            if random(3) == 0 && waiting > 0 {
                let update = inbox[peer].remove(random(waiting));
                project.apply(&update).unwrap();
                for (_, changes) in project.take_remote() {
                    state.apply_remote(&changes);
                }
                continue;
            }
            let length = state.buffer().len();
            let at = state.buffer().clip(random(length + 1));
            let end = state.buffer().clip((at + random(4)).min(length));
            state.set_selections(vec![Selection::new(at, end)]);
            match random(4) {
                0 if round % 7 == 0 => {
                    state.undo();
                }
                0 | 1 => state.type_text(["a", "ß", "\n", "{}"][random(4)]),
                _ => state.type_text(""),
            }
            let changes = state.take_changes();
            if changes.is_empty() {
                continue;
            }
            let update = project.edit("main.tex", &changes);
            for (other, inbox) in inbox.iter_mut().enumerate() {
                if other != peer {
                    inbox.push(update.clone());
                }
            }
        }
        for peer in 0..3 {
            for update in std::mem::take(&mut inbox[peer]) {
                peers[peer].0.apply(&update).unwrap();
            }
            let (project, state) = &mut peers[peer];
            for (_, changes) in project.take_remote() {
                state.apply_remote(&changes);
            }
        }
        let expected = peers[0].0.content("main.tex");
        assert_ne!(expected, start);
        for (project, state) in &mut peers {
            assert_eq!(project.content("main.tex"), expected);
            assert_eq!(state.buffer().to_string(), expected);
        }
    }

    fn ada() -> Author {
        Author {
            name: "Ada".into(),
            color: 1,
        }
    }

    #[test]
    fn comments_follow_their_text_on_every_editor() {
        let mut a = Project::new(1);
        let mut b = Project::new(2);
        b.apply(&a.add_file("main.tex", "Ein Satz hier.")).unwrap();
        b.apply(&a.add_comment("main.tex", 4..8, "Besser?", None, &ada())).unwrap();
        let id = a.comments()[0].id.clone();
        b.apply(&a.add_comment("main.tex", 0..0, "Ja", Some(id.clone()), &ada())).unwrap();
        // Text before the comment and right at its end.
        a.apply(&b.edit("main.tex", &[change(0, 0, ">> "), change(11, 0, "!")])).unwrap();
        for project in [&a, &b] {
            let comments = project.comments();
            assert_eq!(comments.len(), 2);
            let first = comments.iter().find(|comment| comment.parent.is_none()).unwrap();
            assert_eq!(first.range, 7..11);
        }
        assert!(b.take_annotations_changed());
        b.apply(&a.set_resolved(&id, true)).unwrap();
        assert!(b.comments().iter().any(|comment| comment.resolved));
        b.apply(&a.delete_comment(&id)).unwrap();
        assert!(b.comments().is_empty());
    }

    /// Edits `path` of `project` as a tracked author would, keeping the
    /// editor's text in `editor`, and returns the update.
    fn tracked(project: &mut Project, editor: &mut String, path: &str, changes: Vec<TextChange>) -> Vec<u8> {
        let own: Vec<_> = project
            .suggestions()
            .into_iter()
            .filter(|suggestion| suggestion.kind == SuggestionKind::Inserted && suggestion.author == "Ada")
            .map(|suggestion| suggestion.range)
            .collect();
        let mut with_text = Vec::new();
        for mut change in changes {
            change.removed_text = editor[change.start..change.start + change.removed].to_string();
            replay(editor, std::slice::from_ref(&change));
            with_text.push(change);
        }
        let result = crate::track::track(&with_text, &own);
        replay(editor, &result.restores);
        project.edit_tracked(path, &result.ops, &ada())
    }

    #[test]
    fn tracked_typing_and_deleting_make_one_suggestion_each() {
        let mut a = Project::new(1);
        let mut b = Project::new(2);
        b.apply(&a.add_file("main.tex", "Hallo Welt")).unwrap();
        let mut editor = "Hallo Welt".to_string();
        for (at, ch) in [(5, ","), (6, "x")] {
            b.apply(&tracked(&mut a, &mut editor, "main.tex", vec![change(at, 0, ch)])).unwrap();
        }
        // Taking back typed text shrinks the insertion instead.
        b.apply(&tracked(&mut a, &mut editor, "main.tex", vec![change(6, 1, "")])).unwrap();
        // Backspace over "lt" from the end: the text stays, struck through.
        b.apply(&tracked(&mut a, &mut editor, "main.tex", vec![change(10, 1, "")])).unwrap();
        b.apply(&tracked(&mut a, &mut editor, "main.tex", vec![change(9, 1, "")])).unwrap();
        assert_eq!(a.content("main.tex"), "Hallo, Welt");
        assert_eq!(editor, "Hallo, Welt");
        let suggestions = b.suggestions();
        assert_eq!(suggestions.len(), 2, "{suggestions:?}");
        assert_eq!(suggestions[0].kind, SuggestionKind::Inserted);
        assert_eq!(suggestions[0].range, 5..6);
        assert_eq!(suggestions[1].kind, SuggestionKind::Deleted("lt".into()));
        assert_eq!(suggestions[1].range, 9..11);

        // The reviewer rejects the insertion and accepts the deletion.
        b.take_remote();
        let reject = b.reject(&suggestions[0].id);
        let accept = b.accept(&suggestions[1].id);
        let mut b_text = "Hallo, Welt".to_string();
        for (_, changes) in b.take_remote() {
            replay(&mut b_text, &changes);
        }
        assert_eq!(b_text, "Hallo We");
        a.apply(&reject).unwrap();
        a.apply(&accept).unwrap();
        assert_eq!(a.content("main.tex"), "Hallo We");
        assert!(a.suggestions().is_empty());
    }

    #[test]
    fn rejected_deletions_leave_the_text_as_it_is() {
        let mut a = Project::new(1);
        a.add_file("main.tex", "abc");
        let mut editor = "abc".to_string();
        tracked(&mut a, &mut editor, "main.tex", vec![change(1, 1, "")]);
        let id = a.suggestions()[0].id.clone();
        a.reject(&id);
        assert_eq!(a.content("main.tex"), "abc");
        assert!(a.suggestions().is_empty());
    }

    #[test]
    fn files_that_are_not_text_arrive_once() {
        let mut a = Project::new(1);
        let mut b = Project::new(2);
        b.apply(&a.add_blob("figures/frog.png", vec![1, 2, 3])).unwrap();
        assert_eq!(b.take_blobs(), vec![("figures/frog.png".to_string(), vec![1, 2, 3])]);
        assert!(b.take_blobs().is_empty());
        assert!(a.take_blobs().is_empty());
    }
}
