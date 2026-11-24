# Performance- und Bloat-Audit: Umsetzung

## Context
User will "absolute peak performance": keine Ladescreens, keine Wartezeiten, kein Dead Code. Audit (3 Prüfungen über ganze Codebase, Stichproben verifiziert) fand: blockierende Disk-/Prozess-Arbeit im Hauptthread (Speichern, Projekt öffnen, Ordner-Klick, PDFium-Start), Komplett-Neuaufbau des Fensters bei jedem Tastendruck, ganze Dokumente pro Taste kopiert/neu umbrochen, ungenutzter Code und Duplikate. Ziel: jede Interaktion bleibt im Frame-Budget, Disk/Git/Netz nur im Hintergrund, Code schlanker. Keine neuen Crates.

Umsetzung in 5 Phasen, je eigener Commit, ein PR. Nach jeder Phase `cargo build`, `cargo test`, `cargo clippy`.

## Phase 1: Hauptthread frei (größter Effekt auf "Wartezeit")
1. **Speichern im Hintergrund** — `atomic.rs:35` `finish` (2x `sync_all` = F_FULLFSYNC), aufgerufen über `editor_view.rs:390-421` `save`/`write_text`, `workspace.rs:1945` `save_all`, `:1977` `recompile`. Text (Rope-Clone) im Hauptthread nehmen, Konfliktprüfung + Schreiben + fsync in `background_spawn`; Build startet nach Abschluss. Dirty-Flag erst nach Erfolg zurücksetzen, Fehler wie bisher melden.
2. **Projekt öffnen asynchron** — `workspace.rs:909-971` `open_root`: Root setzen, sofort zeichnen; `file_tree::scan`, `Config::load`, `home.touch`, Review-Laden (`collab_session.rs:519`) in `background_spawn`, Ergebnis anwenden. Vorhandenes PDF sofort laden (`projects::newest_pdf`), nicht erst nach Build (`finish_build` `:2109`).
3. **Ordner auf/zu ohne Disk** — `file_tree.rs:22` `scan` läuft komplett und filtert Zugeklapptes erst danach; `workspace.rs:4981` u.a. rufen `refresh_from_disk` (`:1068`, voller Scan + Re-Read aller offenen Dateien + Git) synchron. Vollen Baum cachen (`all_entries`), sichtbare Zeilen daraus ableiten (reine Filterung, kein IO). Datei-Operationen (`:3654`, `:3678`, `:2523`, `:3414`, Add-on-Writes `:1885/1902/1941`) auf vorhandenes `refresh_in_background` (`:1015`) umstellen.
4. **PDFium-Start nicht blockierend** — `pdf/src/lib.rs` `start`: `ready_receiver.recv()` blockiert in `Workspace::new` (`pdf_view.rs:195`) vor dem ersten Frame. Thread starten ohne Warten; Ladefehler als `PdfEvent::Failed` an Clients melden.
5. **`find_main` cachen** — `build/src/config.rs:135` liest bei jedem Build/Tab-Wechsel Dateien. Cache pro aktivem Pfad im Workspace, invalidiert durch Watcher-Events.
6. **Toolchain nicht im Render** — `toolchain.rs:508` `tool_rows` → `find()` (viele `stat`s, ggf. Prozesse) pro Render von Hub-Tools, Setup, Settings. `Found` in `Toolchain` cachen, nur nach Install/Remove/Start auffrischen.
7. **Highlight-Queries vorwärmen** — `editor_core/src/syntax.rs` `LanguageConfig::get` kompiliert `Query::new` beim ersten Öffnen im Hauptthread. Beim Start in Hintergrund-Thread anstoßen (`pub fn warm_up()`).
8. **Collab aus Render/Hauptthread** — `LanBrowser::start` (`collab_session.rs:1260`) läuft in `collab_frame` aus `render` mit `block_on(bind)`: in den Dialog-Öffnen-Handler + Hintergrund verschieben. `host_session` `collect_project` (`:353`, `:972`), `apply_collab_changes` Writes (`:759`, `:793`), `save_review` (`:1844`), `Session::drop` `block_on` → `background_spawn`.

## Phase 2: Watcher und Git entschlacken
- Eigene Saves nicht als Fremdänderung behandeln: Pfad+Hash zuletzt geschriebener Dateien merken, `TEMP_PREFIX`-Dateien in `git_panel::is_watched_path` ignorieren.
- 250-ms-Dauer-Poll im Watcher (`workspace.rs:975-1010`) durch Kanal-Empfang + Debounce ersetzen (kein Aufwachen im Leerlauf).
- Ein `git status` pro Refresh teilen zwischen `git_panel.refresh` und `refresh_git`.

## Phase 3: Rendering pro Frame/Taste
1. **View-Caching** — `.cached(StyleRefinement::default().size_full())` für Editor (`workspace.rs:5872`), PDF (`:7727`, Preview-Body), Git-Panel (`:7781`), Hub (`:7713`). `cx.notify()` bei `EditorEvent::Edited` (`:1124`) nur noch, wenn Workspace-sichtbarer Zustand sich ändert (Dirty-Punkt, Outline-Version).
2. **Kein Volltext-Kopieren pro Taste** — `editor_view.rs:316` `schedule_line_diff`: Rope-Clone übergeben, `to_string()` im Hintergrund nach Timer. Gleich für `refresh_conflicts` (`:336`).
3. **Soft-Wrap nur sichtbar** — `editor_view.rs:848-896` `sync_rows` umbricht bei Breitenänderung alle langen Zeilen. Sichtbaren Bereich + Rand sofort, Rest in Häppchen nach Ruhe; während Resize/Divider-Drag alte Umbrüche behalten.
4. **Theme-Snapshot pro Frame** — `shape_styled` (`:1102`), `prepaint_rows`-Schleifen (`:1798-1889`): einmal `palette()`/`look()` pro Prepaint, Tabelle `[TextRun; Kinds]` vorbereiten; `code_font()`/`code_font_size()` nicht pro Run.
5. **Listen virtualisieren** — Outline (`workspace.rs:5055`) und Vorschau-Contents (`home_view.rs` `render_contents`) mit gemeinsamem Zeilen-Renderer in `outline.rs` (beseitigt auch Duplikat) über `uniform_list`/`list`; `heading_trail` einmal pro Frame cachen auf `(outline_version, cursor_line)`. Picker (`picker.rs:170` leere Suche ohne Limit, `workspace.rs:~7427`) mit `MAX_RESULTS`-Limit + `uniform_list`. Build-Log (`:6288`) nur sammeln, wenn sichtbar, `uniform_list`, Output-Notify drosseln (~30 Hz).
6. **PDF-View** — Fit-Width-Zoom während Drag als "zooming" behandeln (`pdf_view.rs:742`, `schedule_settle`), alte Bilder skaliert zeigen. `content_size`/`page_top` (O(Seiten) pro Seite, `:770`) als kumulative Tabelle pro Zoom cachen.
7. **Kleinkram** — Markdown-Inline-Parse pro Frame cachen (`syntax.rs:690`), Marks per `partition_point` statt O(Zeilen×Marks) (`editor_view.rs:1847,1893`), Error-Line-Set nicht pro Frame, `cursor_position()` nicht 3x pro Frame. Hub: gefilterte Projekte bei Suchänderung cachen (`home_view.rs:406`), Icon-Pfade als `&'static str`.

## Phase 4: Dead Code und Duplikate
- Löschen: `editor_core/src/state.rs:295` `is_read_only`, `syntax.rs` `has_tree`, `git/src/history.rs` `commit_files` + `CommittedFile` + Re-Export, `commit_diff` (inkl. ihrer Tests, falls nur dafür). Privat machen: `buffer.rs:84` `byte_at`, `git/remote.rs` `conflicted_files`/`remote_url`/`https_host`, `latex_usage.rs:13` `statement`.
- Icons `arrow-left.svg`, `layout.svg` löschen + aus `assets.rs` entfernen.
- Duplikate zusammenlegen: `home()` (`projects.rs:134`, `workspace.rs:824`, `collab_session.rs:1945`), Unix-`now()` (4 Kopien), XDG-Cache-Dir (`package_commands.rs:157`, `nvim_themes.rs:50`), Sprach-Icon (`home_view.rs` 2x → `TemplateLanguage::icon()` in `projects.rs`).
- Verrutschten Doc-Kommentar `projects.rs:142` zurück über `tilde()`.

## Phase 5: Abhängigkeiten und Build
- `crates/app/Cargo.toml`: `sha2` 0.10 → 0.11 (doppelte sha2/digest-Kopien weg; API-Aufrufe in `marketplace.rs`, `toolchain.rs`, `export.rs` prüfen).
- `crates/collab/Cargo.toml`: tokio-Features `macros`, `io-util` entfernen, falls Build ohne geht.
- Root `Cargo.toml` `[profile.release]`: `strip = "symbols"`, `debug = false`. `panic = "abort"` bewusst nicht (GPUI-Verhalten ungeprüft).

## Verifikation
- Pro Phase: `cargo build`, `cargo test -p app -p editor_core -p pdf -p git -p build -p collab`, `cargo clippy` ohne neue Warnungen, `cargo check -p app --no-default-features`.
- Messungen vorher/nachher (Release, Instant-Logs temporär, danach entfernt): Start bis erstes Frame; Projekt öffnen (overleaf-sample + synthetisches 300-Seiten-Projekt); Speichern; Ordner auf/zu; Tastendruck-zu-Frame bei 13k-Zeilen-Datei; Divider-Drag.
- Manuell durch User (ich steuere die App nicht fern, da User parallel arbeitet): tippen, speichern, bauen, Ordner auf/zu, Projekt öffnen, Hub-Vorschau, Collab-Dialog.
- Binärgröße `target/release/editor` vorher/nachher.
