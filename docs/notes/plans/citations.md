# Citations & References im Core: komplett, schnell, fehlerfrei

## Context
Citation-Picker (modal, `workspace.rs:4097` `open_picker`) gefällt. Ziel: alles, was pures LaTeX und Typst beim Zitieren und Referenzieren können, deckt der Core ab, inklusive aller Edge Cases. Maximal performant und zuverlässig, weil Fehler bei Buch/Dissertation fatal wären.

**Grenze Core/Add-on:**
- Core: `.bib`/Hayagriva-Dateien lesen, `\cite…`/`@key` tippen, Labels und Refs, Diagnostics, Hover, Jump, Build-Mehrpass.
- Add-on: Integrationen wie Zotero, DOI/arXiv-Lookup, Import, Key-Schema, Anhängen an `.bib`. Dafür kommt kein Code in den Core, nur die bestehende Add-on-API (`SetCitations`, `LookupDoi`, `BibEntries`) bleibt.

**Ist-Zustand, Probleme:**
- Kein Index: jedes `\cite{`-Trigger und jedes Öffnen des Pickers scannt den Baum und parst alle `.bib` neu (`workspace.rs:4762`, `:4132`).
- Filter pro Tastendruck allokiert pro Eintrag auf UI-Thread (`picker.rs:179`).
- `bib.rs` Parser stumm: keine Fehlerpositionen, kein crossref, keine `%`-Kommentare, kein Latin-1, `Entry` ohne Offsets.
- Kein Projekt-Graph: `\bibliography`/`\addbibresource`/`#bibliography` nie aufgelöst, `\subfile` fehlt, Labels nur aus offenen Docs, `Symbol` ohne Position.
- Kein Multi-Cite nach Komma, kein Detail-Panel, kein Hover/Jump, keine Live-Diagnostics, Biber-`.blg` ungelesen.

Constraints: keine neuen externen Crates, Arbeit off UI-Thread, gemessen, keine Emojis in UI. Lieferung in Phasen, je ein PR.

## Abdeckung (Referenzliste für Tests)
**LaTeX Zitieren:**
- Alle Cite-Befehle: `\cite \cite* \nocite \nocite{*}`, natbib (`\citep \citet \citealp \citeauthor \citeyear \Citet …`), biblatex (`\parencite \textcite \Textcite \footcite \autocite \supercite \smartcite \fullcite \citetitle \citedate …`).
- Optionale Argumente `[pre][post]`, `*`-Varianten.
- Multicite: `\cites{a}{b}`, `\autocites(pre)(post)[p1]{a}[p2]{b}`.
- Komma-Listen mit Leerzeichen und Zeilenumbrüchen innerhalb `{}`.
- Bib-Quellen:
  - `\bibliography{a, b}` (ohne/mit `.bib`)
  - `\addbibresource{x.bib}` (Pflicht-Endung, `[location=remote]` nicht flaggen)
  - `\addglobalbib`, `\addsectionbib`
  - `\begin{refsection}[x.bib]`
  - Dateien relativ zum Main-Ordner, dann kpsewhich-Pfad (`BIBINPUTS`) lazy
- Backend erkennen: bibtex/natbib vs biblatex + `backend=`.

**LaTeX Referenzen:**
- `\label` (auch `\label{ x }`, nicht in `\newcommand`-Bodies mit `#1`).
- Ref-Befehle: `\ref \eqref \pageref \autoref \nameref \vref \cref \Cref \cpageref \labelcref \crefrange{a}{b} \subref \hyperref[x]{..}`.
- Komma-Listen bei cleveref.
- `xr`/`xr-hyper` `\externaldocument` gelesen: Labels aus fremder `.aux`, sonst keine False-Positives.

**Ignorieren:**
- `%`-Kommentare (`\%` nicht)
- `verbatim`, `lstlisting`, `minted`, `comment`-Env
- `\iffalse…\fi`
- `\verb|..|`

**Projekt:**
- `\input`, `\include` (+ `\includeonly`), `\import`/`\subimport`, `\subfile`, Zyklen.
- Main via `build::find_main` und `% !TEX root =`-Magic-Comment.

**Typst:**
- Zitieren: `@key`, `@key[p. 5]`, `#cite(<key>)`, `#cite(label("k"), form: ..)`.
- Labels und Refs: `<label>`, `@label`, `#ref(<x>)`.
- Bibliographie: `#bibliography("a.bib")`, `#bibliography(("a.bib", "b.yml"))`, Hayagriva-YAML.
- Kein `@` nach Alnum (E-Mail).
- Ignorieren: Raw-Blöcke `` ` ``/```` ``` ````, Kommentare `//` `/* */`.
- `#include`.

**.bib:**
- Syntax: `@string` + `#`-Concat, `@preamble`, `@comment`, `%`-Zeilen außerhalb Entries, `{}`/`()`-Entries, verschachtelte Klammern, `"..."`-Werte mit `{"}`.
- Kodierung: CRLF, BOM, Latin-1/CP1252 (über `encoding_rs`, ist schon Dependency).
- Felder: crossref und xdata mit Vererbung, Key-Groß/Kleinschreibung.

## Phase 1: Kern (neues Workspace-Crate `crates/citations`, nur std + `encoding_rs`)
**`citations::bib`**: fehlertoleranter State-Machine-Parser über Bytes, ein Pass.
- `Entry { kind, key, key_span, fields: Vec<Field{name, value, span}>, span }`, Werte nach `@string`-Auflösung.
- Recovery: ein unclosed Entry endet beim nächsten `@` am Zeilenanfang, nur dieser Eintrag geht verloren.
- `Diagnostic { range, line, col, severity, kind }`:
  - fehlendes Komma
  - ungeschlossene Klammer oder Anführung
  - unbekannter `@string`
  - doppelter Key, Key nur in Groß/Kleinschreibung verschieden
  - leerer Key
  - rohes `&`/`%`/`#` im Wert
  - crossref-Ziel fehlt, crossref-Zyklus
- Bestehende Helfer aus `crates/app/src/bib.rs` übernehmen: `clean`, `fold`, `split_names`, `last_name`, `authors`, `computed_letters`, `bbl_letters`. `bib.rs` wird dünne Fassade, Tests `bib.rs:461–610` bleiben grün.

**`citations::hayagriva`**: minimaler YAML-Subset-Leser.
- Keys auf Spalte 0 zuverlässig.
- Dazu `title`, `author`, `date`, `parent`/venue best effort.
- Syntaxfehler (Einrückung, doppelter Key) mit Zeile.

**`citations::scan`**: ein Tokenizer pro Sprache für `.tex` und `.typ`.
- Findet Cites, Refs, Labels, Includes, Bib-Quellen und Pakete mit Ranges.
- Beachtet Kommentare, Verbatim und Raw.
- Ersetzt die Zeilen-Heuristik in `addon_reports::latex_names` (`addon_reports.rs:99`). Diese wird auf `scan` umgestellt, Fix: Duplikate nicht mehr per `or_insert` (`:53`) verschlucken.

**`citations::project`**: Graph ab Main (`build::find_main` `config.rs:135`).
- Folgt Includes rekursiv mit `seen`-Set; die Include-Logik von `word_count.rs:273` wandert hierher und `word_count` nutzt sie mit.
- Offene Docs liefern ihren Text statt Disk.
- Labels mit `{file, range, line, env, caption, before_caption}`.
- Nummern aus der `.aux` der letzten Build (`\newlabel{x}{{3.2}{12}…}`).
- Fallback: keine Bib-Quelle deklariert → alle `*.bib`/`.yml` im Baum (heutiges Verhalten). Deklariert, aber Datei fehlt → Diagnose am Argument.

**`citations::index`**: `CitationIndex`, immutable hinter `Arc`.
- Pro Entry vorgefaltete Felder (key, authors, title, year, venue) in einer Arena-`String` mit Offsets.
- `search(query, limit)`:
  - jedes Query-Token muss in irgendeinem Feld treffen (UND)
  - Feldgewichte: key > author > year > title > venue
  - nutzt `picker::fuzzy_score`
  - null Allokation pro Eintrag, Top-K via Partial-Select
- Labels als `HashMap<name, Vec<LabelDef>>`, Duplikate bleiben erhalten.
- Per-File-Cache `(path, mtime, len | doc version) → parse`. Ändert sich eine Datei, wird nur sie neu geparst, und ein neuer `Arc` entsteht aus dem Cache.

**Integration `Workspace`:**
- Feld `citations: Arc<CitationIndex>`, gebaut per `background_spawn`.
- Neuaufbau bei:
  - Start
  - `refresh_in_background` (`workspace.rs:1116`), nur geänderte Dateien
  - Edits offener `.tex`/`.typ`/`.bib`/`.yml`, debounced, mit Versions-Check wie `check_packages` (`workspace.rs:2433`)
- Der Swap auf dem UI-Thread ist nur ein Pointer-Tausch.

**Tests:**
- Jede Zeile der Abdeckungsliste als Test.
- Mutations-Test: zufällige Byte-Änderungen auf einem Korpus, der Parser darf nie paniken und Spans bleiben gültig.
- Timing-Tests `#[ignore]` (Muster `suggestion_timing`), release:
  - 5000 Einträge: Suche < 1 ms
  - 40 `.tex`-Dateien: Graph-Rebuild gemessen
  - eine geänderte `.bib`: Reparse gemessen

## Phase 2: Picker & Completion (LaTeX + Typst)
- `check_citation_trigger` und `open_picker` lesen nur noch den Index, ohne Scan; der Picker ist sofort offen.
- `snippets::opens_citation` (`snippets.rs:249`) öffnet auch:
  - nach `,` innerhalb der Cite-Klammer
  - in jedem `{` eines Multicite (`\cites{a}{`)
  - nach `[..]`-Argumenten
  - Typst: nach `@` und in `#cite(<`
- Accept:
  - fügt nur den Key ein und schließt `}`/`>` nur, wenn sie fehlen
  - Keys, die im selben Cite schon stehen, sind ausgeblendet
  - Typst: `@key` bzw. `<key>`
- `refresh_picker` (`workspace.rs:4429`) nutzt `index.search`.
- `render_picker` (`workspace.rs:8342`) bekommt ein Detail-Panel rechts:
  - Inhalt: Autoren voll, Titel, Venue, Jahr, DOI, Abstract-Auszug, Datei:Zeile
  - Farben aus `theme::palette()`, dadurch passt Paper Mode automatisch
- Ref-Completion (`completion.rs:61`, `editor_view.rs:848`):
  - Komma-Listen
  - Labels projektweit aus dem Index; ersetzt die Label-Kopie in `sync_project_symbols`
  - Detail zeigt env, Nummer und Caption-Anfang
- cleveref geladen:
  - `\cref`/`\Cref` stehen oben unter den Befehlen
  - Ranking nach Label-Prefix passend zum Befehl: `\eqref` zuerst `eq:`; `\cref` alle; ein Typ-Prefix im Query (`fig:`) filtert

## Phase 3: Live-Diagnostics & Lints
- Läuft über bestehende Marks (`Mark`/`MarkSeverity`, `EditorView::set_hint_marks` `editor_view.rs:658`); Severity Error/Warning wird ergänzt, falls nötig.
- Berechnet im Hintergrund aus Index + offenem Text, mit Versions-Check.

**`.tex`/`.typ`:**
- unbekannter Cite-Key: Error. Bei einem Key, der sich nur in Groß/Kleinschreibung unterscheidet, mit Hinweis.
- unbekanntes Label: Warning.
- doppeltes Label: Error an jeder Definition.
- `\label` vor `\caption` im Float: Warning.
- deklarierte Bib-Datei fehlt: Error.
- Keine Marks, solange der Index nicht bereit ist oder die Bib-Auflösung fehlgeschlagen ist.
- `\nocite{*}` beachtet.
- Bei `xr` mit fehlender `.aux` keine Label-Marks.

**`.bib`/`.yml`:**
- Parser-Diagnosen mit Zeile/Spalte.
- Doppelte Einträge (gleiche DOI oder gleicher gefalteter Titel + Jahr) als Hint an beiden Stellen.
- Pflichtfelder je Typ und Backend (biblatex: `date` oder `year`, `journaltitle` oder `journal`).
- Ungeschützte Akronyme im `title` (≥2 Großbuchstaben oder Binnen-Groß, nicht in `{}`; nur bei bibtex-Stilen relevant): Hint.
- bibtex-Backend + Nicht-ASCII: Hint.
- biblatex/biber + TeX-Escapes (`{\"u}`, `{\ss}`): dezenter Hint.
- biber + Nicht-UTF-8-Datei: Warning.

## Phase 4: Hover & Jump
- Hover über Cite-Key oder Label (Maus-Move in `editor_view.rs`): Tooltip mit vollem Eintrag bzw. env, Nummer, Caption und Kontextzeile.
- Cmd+Klick auf Key oder Label: öffnet `.bib`/`.yml`/`.tex`/`.typ` und selektiert die Definition.
- Cmd+Klick sonst bleibt Multi-Cursor (`on_mouse_down` `editor_view.rs:1196`).
- Zusätzlich ein Befehl "Go to Definition".

## Phase 5: Build-Runner
- `crates/build/src/runner.rs`: `.blg` beider Tools parsen statt der Sammel-Warnung (`bibliography_problems` `:349`):
  - Biber: `WARN`/`ERROR` mit `line N`
  - BibTeX: `---line N of file x.bib`, `Repeated entry`, `I was expecting`
  - Ergebnis: Diagnosen mit Datei/Zeile
- `needs_latexmk` (`:371`):
  - Bib-Liste kommt aus dem Projekt-Graph statt nur aus dem Main-Ordner
  - "Citation `x' undefined" wird zeilengenau statt per Substring-Test geprüft
- Mehrpass bleibt bei latexmk; Ziel: nach einer neuen Quelle nie `[?]` im PDF.
- Typst kompiliert in einem Lauf, nichts nötig.

## Nicht im Core (Add-on)
- Zotero, DOI/arXiv-Lookup, Import aus Zwischenablage, Key-Schema, Anhängen an `.bib`.
- UI und Netz dafür leben im Add-on.
- Der Core bietet nur, was Zuverlässigkeit braucht, als Host-API für Add-ons. Neue Host-Nachricht im `addon_api` mit Protokoll-Bump; Add-on-Repo separat anpassen.
- `AddBibEntry { bibtex }`:
  - Der Host parst mit `citations::bib` und prüft auf Duplikate (Key, DOI, Titel + Jahr).
  - Er vergibt den Key nach Schema aus `.editor.toml` (`citation_key`, Default `{author}{year}{word}`), bei Kollision mit Suffix a/b.
  - Er hängt den Eintrag atomar (`crate::atomic::write`) an die erste deklarierte Bib-Datei an.
  - Die Antwort enthält den Key oder den Duplikat-Treffer.
- Kein Netz im Core.

## Kritische Dateien
- `crates/citations/*` (neu)
- `crates/app/src/bib.rs`
- `crates/app/src/workspace.rs`: Picker, Trigger, Refresh, Marks
- `crates/app/src/picker.rs`, `snippets.rs`, `completion.rs`, `editor_view.rs`, `addon_reports.rs`
- `crates/editor_core/src/word_count.rs`
- `crates/build/src/runner.rs`
- `docs/ui-spec.md`

## Verifikation (pro Phase)
- `cargo test --workspace` und `cargo clippy`; die Abdeckungsliste ist als Tests umgesetzt.
- Timing-Tests in release, Zahlen im PR.
- Korpus `crates/build/tests/corpus.rs` (5000 Referenzen, kaputte Einträge): die Diagnosen des neuen Parsers gegen echte bibtex/biber-Fehler vergleichen; die Zeilen müssen übereinstimmen.
- Manuell über den `run`-Skill, mit Beispielprojekten unter `examples/`:
  - Dissertation-Layout: `chapters/`, `\addbibresource` in der Preamble, `\subfile`
  - Typst-Projekt mit `.bib` + `.yml`
- Prüfen:
  - Picker öffnet sofort
  - Komma- und Multicite
  - Detail-Panel in Paper Mode
  - rote Marke bei `\cite{typo2024}`
  - doppeltes Label markiert
  - Hover und Cmd+Klick
  - Build mit neuer Quelle ohne `[?]`
