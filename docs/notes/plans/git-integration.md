# Plan: Git-Integration ausbauen (nativ)

## Context
Ziel: Overleafs Git/GitHub-Anbindung übertreffen. Vor allem offline (im Zug), schnell und mit freier Wahl des Hostings: GitHub, GitLab, KIT-GitLab, NAS.

Git ist **schon nativ** drin:
- `crates/git` ruft die git-CLI auf: Status (porcelain v2), Stage/Unstage, Commit, Branches, Push (setzt Upstream), Pull nur ff-only, Log mit Graph-Lanes.
- `crates/app/src/git_panel.rs` zeigt Änderungen, Commit-Feld, Push/Pull/Refresh, Branch-Picker, History-Graph und Ahead/Behind-Anzeige.

Es fehlt:
- Statusfarben im Dateibaum
- Diff-Marker im Gutter
- Shortcuts
- Auto-Commit-Message
- Hintergrund-Sync
- Token-Login
- Konfliktauflösung (Pull verweist heute auf ein Terminal)
- Commit-Details

**Entscheidung:** nativ ausbauen, kein Add-on. Ein Add-on hat keinen Zugriff auf Dateibaum und Gutter.

**Bewusst kein `git2`:**
- libgit2 bringt C-Build und OpenSSL mit, also Bloat.
- Die CLI-Lösung läuft schon im Hintergrund und nutzt SSH-Agent und Credential-Helper des Systems.
- Die Lean-Regel gilt (keine neuen Crates).

**Nutzerwahl:**
- Sync: Fetch und ff-Pull automatisch, Push nur per Klick.
- Konflikte: im Editor pro Block lösen.
- Extras: Gutter-Diff, Auto-Commit-Message, Commit-Details, Token-Login.

## 1. `crates/git` erweitern
Alles CLI über die vorhandenen `run` bzw. `run_on_network` (`crates/git/src/lib.rs`).

**Neu:**
- `fetch()`: `fetch --prune --quiet`.
- `pull_merge()`: `pull --no-rebase --no-edit`. Ergebnis ist `PullOutcome::{UpToDate, Merged, Conflicts(Vec<String>)}`. Der bisherige ff-only-`pull` bleibt für den Hintergrund.
- `merge_in_progress()` prüft auf `.git/MERGE_HEAD`.
- `abort_merge()` und `finish_merge()` (`commit --no-edit`).
- `file_at_head(path) -> Option<String>`: `show HEAD:<path>`. Ergibt `None` bei neuen Dateien oder ohne Commit.
- `commit_files(oid) -> Vec<(status, path)>`: `show --name-status --format= <oid>`.
- `commit_diff(oid, path) -> String`: `show --format= --patch <oid> -- <path>`.

**Auth:**
- Erkennen: stderr enthält „Authentication failed“, „could not read Username“ oder „403“. Dann `GitError::Auth { host }`. Das heißt: `Result<_, String>` wird für Netzwerkbefehle zu einem Enum mit `Auth` und `Other(String)`.
- `Program` bekommt `askpass: Option<PathBuf>`. Ist es gesetzt, kommen bei Netzwerkbefehlen `GIT_ASKPASS` und `SSH_ASKPASS` dazu, mit Pfad zum Editor-Binary plus Env `EDITOR_GIT_ASKPASS=1`.

## 2. Token-Login (HTTPS)
**Schlüsselbund:**
- `get_secret`/`set_secret` aus `crates/app/src/addons.rs:318-375` wandern nach `crates/app/src/secrets.rs`, mit Service als Parameter. Add-ons nutzen weiter `"editor-addon"`, Git `"editor-git"`, Account ist der Host.

**Askpass-Modus:**
- `main.rs` prüft ganz am Anfang, vor GPUI, ob `EDITOR_GIT_ASKPASS` gesetzt ist.
- Dann liest es die Prompt aus `argv[1]`:
  - „Username for 'https://host'“ ergibt den gespeicherten Benutzernamen, sonst `git`.
  - „Password for …“ ergibt das Token aus dem Schlüsselbund.
- Danach Ausgabe auf stdout und Exit, ohne Fenster und ohne GPUI.

**Dialog:**
- Bei `GitError::Auth` beim Push oder Pull öffnet sich `Dialog::GitSignIn { host }` mit Feldern für Benutzername (optional) und Token, dazu ein Hinweis-Link, wo man ein Token erzeugt.
- Speichern legt das Token im Schlüsselbund ab und wiederholt die Aktion.
- Das Token landet nie in Dateien und nie in der URL.

## 3. Hintergrund-Sync (Fetch + ff-Pull, kein Auto-Push)
In `crates/app/src/workspace.rs` mit einem neuen kleinen Modul `git_sync.rs`, im Stil von `collab_session.rs` als Kind-Modul von workspace.

- Läuft nur, wenn das Projekt ein Repo mit Upstream ist, als Timer alle 5 min. Nach Fehlern Backoff bis 30 min.
- Ablauf: `fetch`.
  - Liegt das Projekt zurück und **kein offenes Dokument ist dirty**, folgt `pull` (ff-only). Den Rest erledigen Watcher und Reload.
  - Geht ff nicht (divergiert), kommt nur ein Hinweis in Panel und Status: „Remote has new commits. Pull to merge.“
- Nie Auto-Push. Auth-Fehler im Hintergrund lösen keinen Dialog aus, nur eine stille Notiz im Panel.
- Setting `git_background_sync` (Standard: an) in `settings.rs` plus Schalter in den Editor-Einstellungen.
- Idle: zwischen den Timern keine Arbeit.

## 4. Statusfarben im Dateibaum
- Workspace hält `git_files: HashMap<PathBuf, FileState>` (`Modified`, `Added`/`Untracked`, `Staged`, `Conflicted`, `Deleted`).
- Aktualisiert wird beim vorhandenen Watcher-Refresh (`refresh_in_background`, `workspace.rs:~865`), und zwar mit `status()` im selben Background-Task. Heute läuft das nur bei sichtbarem Panel.
- Darstellung in `render_tree_rows` (`workspace.rs:~4383`):
  - Dateiname eingefärbt: modified Warnfarbe, neu grün, Konflikt rot, staged Akzent.
  - Rechts ein Buchstabe (M/A/U/C).
  - Ordner mit Änderungen bekommen einen Punkt.

## 5. Diff-Marker im Gutter
**Zeilen-Diff:** `crates/editor_core/src/line_diff.rs` (neu)
- Myers O(ND) über Zeilen-Hashes. Ergebnis `Vec<LineHunk { kind: Added | Modified | Deleted, lines: Range<usize> }>`.
- Über 20k Zeilen oder D > 2000 wird abgebrochen und es gibt keine Marker. Rein, testbar.

**EditorView** (`editor_view.rs`):
- `diff_base: Option<Arc<str>>`. Neuberechnung debounced 300 ms nach Edits auf dem Background-Executor (Snapshot via `buffer().to_string()`).
- `set_diff_base` setzt die Basis.
- Gezeichnet wird ein 3-px-Balken links im Gutter: grün neu, blau geändert, rotes Dreieck bei gelöschten Zeilen.

**Workspace:** Beim Öffnen eines Dokuments, nach Commit oder Pull und bei `GitPanelEvent::Changed` holt es `file_at_head` im Hintergrund und setzt die Basis.

## 6. Commit & Push, Shortcuts, Auto-Commit-Message
**Shortcuts** (`actions.rs`, `shortcuts.rs`):
- `⇧⌘G` öffnet das Git-Panel und fokussiert das Commit-Feld (vorher prüfen, ob die Belegung frei ist).
- `⌘↩` **im Commit-Feld** löst „Commit & Push“ aus. Das geschieht per `on_key_down` am Feld mit `stop_propagation`, sonst greift Recompile.
- Ist nichts gestaged, wird vorher alles gestaged.

**Commit & Push:**
- Neuer Button neben Commit.
- Ein Push-Fehler lässt den Commit lokal stehen, Status „Committed. Push failed: …“.
- Offline gilt dasselbe, später wird erneut per Klick gepusht.

**Auto-Commit-Message:** `git_panel.rs`, neue Funktion `suggest_message`, reiner Text-Teil testbar.
- Quellen sind die geänderten Dateien aus `status`, und für offene .tex/.typ-Dateien die Überschriften um geänderte Zeilen. Die kommen aus dem Zeilen-Diff gegen HEAD plus Outline-Headings (`Syntax::headings`).
- Beispiele:
  - „Update Introduction“
  - „Update Methods and 2 more sections“
  - „Add frog.jpg“
  - „Update main.tex, sample.bib“
- Der Vorschlag erscheint als Platzhalter im Feld. Commit mit leerem Feld nutzt ihn. Tab übernimmt ihn zum Bearbeiten.

## 7. Konflikte im Editor lösen
- **Pull:** Der manuelle Pull-Button nutzt `pull_merge`.
- **Konfliktblöcke:**
  - Bei Konflikten listet das Panel die Dateien, dazu „Abort merge“ und „Finish merge“. Finish ist erst aktiv, wenn keine Marker mehr da sind; es staged die Dateien und ruft `finish_merge`.
  - Neu in `crates/editor_core/src/conflicts.rs`: Parser findet `<<<<<<<`, optional `|||||||`, dann `=======` und `>>>>>>>`, und liefert Blöcke mit Bereichen für ours, base und theirs.
- **Darstellung:** Neue `DecorationKind`s `ConflictOurs` und `ConflictTheirs` färben die Blöcke grün und blau getönt, die Marker-Zeilen gedimmt.
- **Konfliktleiste:**
  - Über dem Editor sitzt eine Leiste nach dem Muster von `render_hint_strip` (`workspace.rs:~5106`).
  - Inhalt: „Conflict 1 of 3“ mit den Knöpfen **Keep mine**, **Keep theirs**, **Keep both** und **Next**.
  - Sie wirkt auf den Block am oder nach dem Cursor. Ersetzt wird per `replace_range`, also ist das per Undo umkehrbar.
- Nach jeder Auflösung wird neu geparst. Gibt es keine Blöcke mehr, kommt die Statusmeldung „All conflicts in main.tex solved“.

## 8. Commit-Details
- Ein Klick auf eine History-Zeile (`git_panel.rs` render history) klappt die geänderten Dateien auf (`commit_files`).
- Ein Klick auf eine Datei öffnet `Dialog::CommitDiff`: einen Read-only-`EditorView` mit dem Patch-Text. Hinzugefügte und entfernte Zeilen werden über die vorhandenen Decorations eingefärbt (`Inserted` für +, `Deleted` für -).
- Read-only kommt über `EditorState::set_read_only`, das es schon gibt.

## Kritische Dateien
| Bereich | Dateien |
|---|---|
| Git-Crate | `crates/git/src/{lib.rs, actions.rs, status.rs, log.rs}` |
| Editor-Kern | `crates/editor_core/src/{line_diff.rs (neu), conflicts.rs (neu), state.rs (DecorationKind), lib.rs}` |
| Git-UI | `crates/app/src/git_panel.rs`: Commit & Push, Vorschlag, Konflikte, Details |
| Workspace und Sync | `crates/app/src/workspace.rs` plus `git_sync.rs` (neu): Sync, Baumfarben, Diff-Basis, Konfliktleiste, Dialoge |
| Editor | `crates/app/src/editor_view.rs`: Gutter-Marker, Konfliktfarben |
| Askpass, Schlüsselbund | `crates/app/src/main.rs` (Askpass-Modus), `secrets.rs` (neu, aus `addons.rs`) |
| Einstellungen, Tasten | `crates/app/src/settings.rs`, `actions.rs`, `shortcuts.rs` |

Wiederverwendet werden:
- `run_on_network` (BatchMode-SSH, Timeouts)
- Watcher-Batching (`is_watched_path`)
- `Syntax::headings`
- das Hint-Strip-Muster
- `Decoration` und `set_read_only` aus der Collab-Arbeit
- `crate::atomic::write`

## Verifikation
**`cargo test -p git`**, mit echten Tempdir-Repos und einem lokalen Bare-Repo als Remote:
- fetch und ff-pull
- divergierender Pull ergibt `Conflicts`
- abort und finish
- `file_at_head`
- `commit_files` und `commit_diff`
- Auth-Erkennung über ein gefaktes stderr

**`cargo test -p editor_core`:**
- `line_diff`: Fuzz gegen Brute-Force-LCS, dazu Grenzfälle (leer, nur Löschen, CRLF).
- Konflikt-Parser: zwei- und dreiteilige Marker, kaputte Marker werden ignoriert.

**App-Tests:**
- `suggest_message`
- Askpass: das Binary mit Env aufrufen und gefakte Secrets per Test-Hook übergeben.

**GUI**, per CGEvent-Klick und -Tasten auf einer Scratchpad-Kopie mit lokalem Bare-Remote:
- Baumfarben nach Änderung
- Gutter-Balken
- `⇧⌘G` und `⌘↩` für Commit & Push
- Vorschlags-Text im Commit-Feld
- erzeugter Konflikt, Lösung über die Leiste, Finish merge
- Commit-Details-Dialog

**Messen:** Idle-CPU mit aktivem Sync-Timer, und dass Gutter-Diff und Status nicht pro Tastendruck laufen.

**Clippy:** mit und ohne `--no-default-features`.
