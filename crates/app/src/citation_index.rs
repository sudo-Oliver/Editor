//! Keeps the citation index of the document up to date: rebuilt off the main
//! thread a moment after an edit, a change from outside, a switch of tab or a
//! build, and never twice at once. Only files that changed are read again.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::Duration,
};

use build::find_main;
use citations::index::{self, CitationIndex, Inputs};
use gpui::{AppContext, Context};

use super::Workspace;
use crate::file_tree;

/// Edits come in bursts, so the rebuild waits for a pause.
const DELAY: Duration = Duration::from_millis(200);

/// Whether a file of this name can change what the index holds.
pub(super) fn matters(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| matches!(extension, "tex" | "ltx" | "typ" | "bib" | "yml" | "yaml" | "aux" | "bbl"))
}

/// A bibliography of the TeX installation, such as `IEEEabrv.bib`, asked of
/// `kpsewhich` once per name.
fn system_bib(name: &str) -> Option<PathBuf> {
    static FOUND: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    let found = FOUND.get_or_init(Default::default);
    if let Some(path) = found.lock().unwrap_or_else(PoisonError::into_inner).get(name) {
        return path.clone();
    }
    let path = build::find_tool("kpsewhich").and_then(|kpsewhich| Command::new(kpsewhich).arg(name).output().ok()).and_then(|output| {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (output.status.success() && !path.is_empty()).then(|| PathBuf::from(path))
    });
    found.lock().unwrap_or_else(PoisonError::into_inner).insert(name.to_string(), path.clone());
    path
}

impl Workspace {
    /// Rebuilds the index a moment from now, or once more after the running rebuild.
    pub(super) fn schedule_citations(&mut self, cx: &mut Context<Self>) {
        self.citations_stale = true;
        if self.citations_task.is_some() {
            return;
        }
        self.citations_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DELAY).await;
                let Ok(Some(job)) = this.update(cx, |this, cx| this.citation_job(cx)) else {
                    break;
                };
                let built = cx.background_spawn(job).await;
                let again = this
                    .update(cx, |this, cx| {
                        this.citations = Arc::new(built);
                        this.citations_ready = true;
                        cx.notify();
                        this.citations_stale
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
            }
            this.update(cx, |this, _| this.citations_task = None).ok();
        }));
    }

    /// What a rebuild needs, taken on the main thread: the main file, the
    /// open documents' text, which is copied only when it changed, and the cache.
    fn citation_job(&mut self, cx: &mut Context<Self>) -> Option<impl Future<Output = CitationIndex> + use<>> {
        self.citations_stale = false;
        let root = self.root.clone()?;
        let active = self.active_view().and_then(|view| view.read(cx).path.clone());
        let main = find_main(&root, &self.config, active.as_deref());
        let mut texts = HashMap::new();
        for document in &self.documents {
            let view = document.view.read(cx);
            let Some(path) = view.path.clone().filter(|path| matters(path)) else {
                continue;
            };
            let version = view.state.version();
            let text = match self.citation_texts.get(&path) {
                Some((known, text)) if *known == version => text.clone(),
                _ => Arc::from(view.state.buffer().to_string()),
            };
            texts.insert(path, (version, text));
        }
        let open: HashMap<PathBuf, Arc<str>> = texts.iter().map(|(path, (_, text))| (path.clone(), text.clone())).collect();
        self.citation_texts = texts;
        let cache = self.citation_cache.clone();
        Some(async move {
            let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
            let walk = || file_tree::scan(&root, &HashSet::new()).into_iter().filter(|entry| !entry.is_dir).map(|entry| entry.path).collect();
            let inputs = Inputs { root: &root, main, active, open: &open, tree: &walk, system_bib: &system_bib };
            index::build(&mut cache, &inputs)
        })
    }
}
