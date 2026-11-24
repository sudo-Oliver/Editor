# PDF-Panel: Textauswahl, Links + Verlauf, Auto-Follow, Bild-Vorschau (lean)

## Context
Editor (GPUI + PDFium) zeigt PDF nur als Bitmap: kein Markieren, keine klickbaren Links. Binärdateien im Tree enden in `Cannot open …`. Ziel: Overleaf-/Sioyek-Komfort für lange Arbeiten. **Harte Vorgabe: lightweight, maximale Performance, kein Bloat.**

Schon vorhanden, wird nicht neu gebaut: Forward Sync per Klick (`forward_search`, `workspace.rs:731`), Backward Sync per Cmd+Klick (`backward_search`, `workspace.rs:756`), SyncTeX-Parser, `-synctex=1`.

## Performance-Regeln
1. **Keine neuen Crates.** `cargo tree` vor/nach gleich lang. `image`-Features nur ergänzen, falls GPUI sie nicht schon aktiviert (`cargo tree -e features`).
2. **Nichts beim Scrollen oder Rendern extrahieren.** Seiteninhalt erst, wenn Maus über Seite kommt. Durchscrollen kostet null.
3. **Kein Repaint ohne Änderung.** `on_mouse_move` ruft `cx.notify` nur, wenn Hover-Link oder Auswahl wechselt.
4. **Idle = 0 CPU.** `request_animation_frame` nur während laufender Scroll-Animation. Auto-Follow aus: nur Bool-Prüfung.
5. **Speicher begrenzt.** Inhalt wird wie Bilder verworfen (`KEEP_PAGES`), außer bei aktiver Auswahl. Verlauf max. 100 Einträge.
6. **PDFium bleibt auf Worker-Thread.** UI-Thread nur Hit-Tests auf fertigen Daten.

## Bewusst gestrichen
Wort→Spalte bei Backward Sync (bleibt zeilengenau), Toolbar-Buttons für Zurück/Vor, Zeichen-Hit-Test für I-Beam (I-Beam über ganzer Seite), Rand-Auto-Scroll beim Ziehen (Mausrad geht), Cmd+A, Bild-Tabs, `.pdf`-Assets, Trackpad-Swipe (GPUI bietet keinen; Maus-Seitentasten gehen).

## Abweichungen von Vorlage
- **Router:** keine Text-Whitelist. Nur Bildendungen abzweigen, `.toml`/`.txt` öffnen weiter.
- **Bild ohne Tab:** Vorschau ersetzt Editor-Bereich, wie Overleaf. Tab-Wechsel schließt sie. Spart Umbau an ~36 Stellen.
- **Auto-Follow:** scrollt nur, wenn Ziel außerhalb Viewport-Mitte (25–75 %), dann zentrieren. Kein Flash. Default aus.
- **Typst/Pandoc:** ohne SyncTeX kein Auto-Follow. Auswahl, Links, Verlauf gehen überall.

## Schritt 1: SyncTeX-Index (`crates/synctex/src/lib.rs`)
`forward` macht aktuell vier lineare Scans über alle Nodes auf UI-Thread: für Auto-Follow bei langer Arbeit zu teuer.
- Beim Parsen sortierten Index `leaves: Vec<u32>` nach `(tag, line)` bauen (läuft schon auf Hintergrund-Task, `workspace.rs:649`).
- `forward` per Binärsuche: erste Zeile `>= line`, sonst letzte Zeile der Datei. Verhalten gleich, bestehende Tests + `tests/roundtrip.rs` sichern das ab.

## Schritt 2: Inhalt auf Anfrage (`crates/pdf`)
```rust
pub struct PageChar { pub ch: char, pub left: f32, pub top: f32, pub right: f32, pub bottom: f32 }
pub enum LinkTarget { Page { page: usize, x: Option<f32>, y: Option<f32> }, Uri(String) }
pub struct PageLink { pub rect: Rect, pub target: LinkTarget }
pub struct PageContent { pub chars: Box<[PageChar]>, pub links: Box<[PageLink]> }
// Command::Content { generation, page }  /  PdfEvent::PageContent { generation, page, content }
```
- Worker bedient `Content` vor Render-Queue, einzeln, sodass Scrollen nicht blockiert.
- Text: `page.text()?.chars()`, `unicode_char()`, `loose_bounds()`. Links: `page.links().iter()`, `rect()`, `destination()`, sonst `action()` (`LocalDestination`, `Uri`); `view_settings()` liefert x/y.
- Koordinaten nach oben-links (`y = page_height - y`), gleiche Basis wie `PdfLocation`.

## Schritt 3: Auswahl + Copy
Reines Modul `crates/app/src/pdf_text.rs`, getestet: `char_at`, `selection_rects` (Boxen je Zeile), `selected_text` (`\r\n` zu `\n`, U+0002 weg, Seiten mit `\n`).

`crates/app/src/pdf_view.rs`:
- `content: HashMap<usize, PageContent>`, `requested: HashSet<usize>`, `selection: Option<Selection>`, `focus: FocusHandle`. Bei `Loaded`/`clear` leeren.
- Hit-Test aus `on_mouse_down` (`:319-332`) in `hit_page(position)` auslagern.
- `.on_mouse_move`, `.on_mouse_up`, `.on_mouse_up_out`, `.track_focus`, `.key_context("PdfView")`.
- `char_at` nur bei Mouse-Down und Drag, nie beim bloßen Hover.
- Rechtecke nur für sichtbare Seiten berechnen, bei Auswahländerung cachen; `PaintList` erweitern.
- `crates/app/src/actions.rs`: `secondary-c` auf `Copy` im Kontext `PdfView`.

## Schritt 4: Links, weiches Scrollen, Verlauf
`pdf_view.rs`:
- Hover: Linear über wenige Links der Seite; dezentes Quad, Hand-Cursor.
- Klick: Aktion bei Mouse-Up ohne Drag. Cmd+Klick bleibt Backward Sync.
- `scroll_anim: Option<ScrollAnim>` (~180 ms ease-out), in `prepare_paint`. Mausrad oder Zoom bricht ab. `reveal` nutzt das.
- Interner Link: Position (Seite + y in Punkten, zoomfest) auf `back`, `forward` leeren, scrollen, Ziel flashen. Bleibt über Recompile, Seite geklemmt.
- URI-Link: nur `http`, `https`, `mailto` per `cx.open_url`; PDF-Inhalt ist fremd, Rest ignorieren + Status.

`actions.rs`: `PdfBack`/`PdfForward` auf `secondary-[`/`secondary-]`, `alt-left`/`alt-right` in `PdfView`; `MouseButton::Navigate(..)`; Einträge in `shortcuts.rs`.

## Schritt 5: Auto-Follow
- `crates/app/src/settings.rs`: `auto_scroll_pdf: bool`, Default `false`, Test-Literale `:70`, `:80`.
- `workspace.rs`:
  - Zeile in `render_editor_settings` (`:2395`-Muster), Toggle in Vorschau-Toolbar, beide über `update_settings`.
  - `cx.observe(&view, ..)` in `open_file`, Subscription in `OpenDocument`. Deckt Vim und Tastatur ab, ohne Bewegungspfade anzufassen. Reihenfolge: Setting aus oder Dokument inaktiv, sofort raus; Zeile gleich, raus.
  - Zeile geändert: Task ersetzen, 150 ms warten, `PdfView::follow(location)`. Kein Verlaufseintrag.
  - **Schleifenschutz:** `backward_search` setzt `follow_line` vor `go_to_line`.
  - Nach `finish_build` mit neuer SyncTeX einmal nachziehen.

## Schritt 6: Bild-Vorschau
`crates/app/src/image_view.rs`:
- `gpui::img(path)` mit `object_fit` Contain: GPUI dekodiert async und unterstützt PNG, JPEG, WebP, SVG.
- Beim Schließen aus GPUI-Cache entfernen, sonst bleibt Bild im Speicher.
- Meta im Hintergrund: `image::image_dimensions` (liest nur Header) + Dateigröße. SVG ohne Auflösung.
- **Export:** `cx.prompt_for_new_path`, `fs::copy` im Hintergrund.
- **Copy Include:** reine, getestete `include_snippet(engine, relative, extension)`: LaTeX `\includegraphics{..}`, SVG `\includesvg{..}`, Typst `#image("..")`, Pandoc `![](..)`. Pfad relativ zum Ordner der Hauptdatei, sonst Projektwurzel.

`workspace.rs`: `image_preview: Option<Entity<ImageView>>`. `open_file` (`:379`): Endung (case-insensitive) `png|jpg|jpeg|webp|svg`, dann Vorschau statt `read_to_string`. `activate` leert sie. Editor-Bereich rendert sie, falls gesetzt.

## Schritt 7: Doku
`README.md` Abschnitte PDF navigation, Settings.

Je Schritt ein Commit, Stil `feat(pdf): …`, `feat(app): …`.

## Verifikation
1. `scripts/fetch-pdfium.sh` (fehlt lokal), `cargo test --workspace`.
2. Neue Tests: `pdf_text`, `include_snippet`, Settings, `forward`-Index. `crates/pdf/tests/content.rs`: hyperref-PDF per `pdflatex`, Link-Ziel + Text prüfen; still überspringen wenn Tools fehlen, wie `synctex/tests/roundtrip.rs`.
3. **Performance vorher/nachher messen** (Baseline auf `main`, Release-Build):
   - Crate-Anzahl (`cargo tree`) und Binary-Größe: 0 neue Crates, Größe im Bericht nennen
   - Großes PDF (500+ Seiten): Scroll-Frames unverändert, Idle-CPU 0 %
   - Speicher nach Durchscrollen ohne Maus über Seiten: gleich wie Baseline
   - `forward` auf großer `.synctex.gz`: per Test mit Zeitausgabe, deutlich unter 1 ms Ziel
4. Manuell, `cargo run --release -p app -- examples/overleaf-sample`, Dokument mit `hyperref`, `\cite`, `\ref`:
   - Text ziehen, Cmd+C, einfügen; über Seitengrenze, Zoom ≠ 100 %, invertiert
   - `[42]` klicken; `Cmd+[` zurück; `Cmd+]` vor. Externer Link öffnet Browser
   - Auto-Follow an: Vim-`j`, PDF folgt; aus: bleibt stehen. Cmd+Klick mit Auto-Follow: kein Rücksprung. Neustart: Einstellung erhalten
   - PNG, JPG, WebP, SVG: Vorschau + Meta, Export, Include-Code; Tab-Wechsel zurück
   - Risiko prüfen: Seite mit CropBox-Offset oder Rotation
