# Claude nativ im Editor: Add-on-Protokoll v2, editor-ai, MCP, CLI

## Context
Kollegen mit Claude-Abo arbeiten heute so: Claude im Browser fragen, Antwort kopieren, im Editor (früher Overleaf) einfügen. Ziel: Claude hilft direkt im Editor bei allem (Setup, Text, Zitationen, Bilder, Compile-Fehler), sieht aus wie ein natives Feature, mit den Infos der Claude-Code-Statusline (Modell, Modus, Effort, Kontext %, 5h/Wochen-Limit, Kosten) und Umschaltern für Modell/Modus/Effort.

Ist-Stand:
- `editor-ai` (eigenes Repo) = leeres Chat-Panel, gepinnt auf alte `addon_api`-Rev.
- Add-on-Protokoll v1 (`crates/addon_api/src/lib.rs`) zu schmal: kein Markdown, keine Statusitems, kein Diff-Review, keine Events, keine Commands/Kontextmenü, kein Bild-Drop, kein PDF, keine Fernsteuerung.
- `claude` 2.1.311 mit Pro-Abo eingeloggt; `claude -p --input-format stream-json --output-format stream-json` liefert alles Nötige (init, Deltas, tool_use, `rate_limit_event`, `result` mit Kosten) und Live-Steuerung (`set_model`, `set_permission_mode`, `interrupt`, `can_use_tool`, `get_context_usage`).

Entscheidungen: Inline-Review mit Annehmen/Ablehnen pro Block (Modus Auto wendet direkt an). Reihenfolge Panel, dann MCP, dann CLI. Erst nur Claude, Backend-Trait offen. Generische UI-Bausteine ins Editor-Repo (Protokoll v2), KI-Logik bleibt in `editor-ai`.

Regeln: keine neuen Crates, kein Idle-CPU, kein Repaint ohne Änderung, keine Emojis, kein KI-Code im Editor-Repo.

## Leitlinien (Feedback)
1. **Zielgruppe: Leute, die Claude aus der App oder von claude.ai kennen, nicht Claude Code.**
   - Im Add-on fallen weder "Claude Code" noch "CLI" noch "Terminal". Der Einstieg ist ein einziger Knopf "Sign in with Claude".
   - Das Add-on richtet die offizielle Claude-Code-Engine selbst ein: Installation in den eigenen Add-on-Ordner, dann `claude auth login`, das den Browser öffnet und mit dem normalen Claude-Konto (Pro/Max) anmeldet.
   - Grund: Die Claude-App und claude.ai haben keine Schnittstelle für andere Programme. Der einzige offizielle Weg über das eigene Abo ist diese Engine; derselbe Account und dieselben Limits.
2. **Es muss sich wie claude.ai anfühlen, nur im Editor.**
   - Bilder, PDFs und Daten (CSV) per Drag & Drop oder Einfügen in den Chat.
   - Erzeugte Inhalte (Plots, Abbildungen, Tabellen, Textabschnitte, BibTeX) erscheinen als Karten mit Vorschau und "Insert at cursor", "Save to project", "Copy" und "Open".
   - Karten lassen sich direkt an eine Stelle im Dokument ziehen.
3. **KI ist ausschließlich opt-in über das Add-on.**
   - Ohne aktiviertes Add-on gibt es im Editor nichts mit KI: keine Menüeinträge, keine Shortcuts, keine Statusitems, kein Panel, keine Prozesse.
   - Das Add-on wird in Settings > Add-ons installiert und dort ein- und ausgeschaltet.
   - Ausschalten beendet den Prozess sofort und entfernt alle Beiträge des Add-ons aus der Oberfläche.
   - Die neuen Protokoll-Bausteine im Editor sind generisch und enthalten keinen KI-Text.

## Phase A: Add-on-Protokoll v2 (Editor-Repo, generisch)

### A0 Versionierung (`crates/addon_api/src/lib.rs`)
- Neue Konstante `API_VERSION = 2`.
- Der Host akzeptiert Manifeste mit `api` 1 und 2; v1-Add-ons bekommen keine neuen Nachrichten.
- `Init` bekommt `host_version` und `features: Vec<String>`, beide mit `#[serde(default)]`. Add-ons prüfen Features, nicht Versionen.
- `Node` bekommt `#[serde(other)] Unknown`; der Host zeichnet dafür nichts, statt die View zu verwerfen.
- Keine neuen Permissions: Lesezugriffe laufen unter `ReadProject`, Schreibzugriffe unter `EditDocuments`.
- Fixture-Test: Jede Nachricht wird gegen eingecheckte JSON-Dateien serialisiert, damit Protokolländerungen im Review auffallen.

### A1 Neue Nodes und inkrementelle Updates (`addon_panel.rs`)
Neue Nodes:
- `Markdown{id,text}`: wird per `Append` gestreamt.
- `Message{id,role,children}`: Chat-Blase für user, assistant, tool oder system.
- `Collapsible{id,title,open,children}`: für Tool-Aufrufe und Thinking.
- `Card{id,tone,children}`: für Erlaubnis-Anfragen.
- `Chips{id,items}`: Kontext-Chips.
- `Image{path}`.
- `Input` bekommt `multiline`, `submit` und `drop_target`.

Neue Nachrichten vom Add-on: `Insert{parent,index,node}`, `Replace{id,node}`, `Remove{id}`, `Notify{text,tone}`, `Subscribe{events}`.

Performance:
- Ein Id-Index (Id auf Kindpfad) macht `Append`, `Replace` und `Remove` zu O(Tiefe) statt einem Lauf über den ganzen Baum.
- Nachrichten werden gebündelt: `try_recv` leert die Queue, danach kommt ein einziges `cx.notify()` pro Batch.
- Markdown-Parser im eigenen `crates/app/src/markdown.rs`, ca. 300 Zeilen, ohne Crate. Er kann Überschriften, Absätze, fett/kursiv, Inline-Code, Codeblöcke mit den Knöpfen "Copy" und "Insert at cursor", Listen, Zitate und Links.
- Der Parser arbeitet inkrementell: Fertige Blöcke werden gecacht, beim Streamen wird nur der letzte offene Block neu geparst.
- Danach ein eigener PR: virtuelles Scrollen (`Scroll{virtual}`) über gpui `list()`, für lange Chats.
- Keine Timer und keine Animationen.

### A2 Events (nur für abonnierte v2-Add-ons)
- `ActiveFileChanged`
- `SelectionChanged`: entprellt mit 250 ms; ein Timer existiert nur, wenn ein Add-on das Event abonniert hat.
- `Saved`
- `BuildFinished{success,errors,warnings}`
- `ProjectChanged`
- `FilesDropped`: über `on_drop::<ExternalPaths>`.
- `ImagePasted`: als PNG in einem privaten Temp-Ordner mit Rechten 0700.
- `Command{id,context}`
- `StatusClick` und `StatusSelect`
- `ReviewDecided`

### A3 Requests (Wiederverwendung in Klammern)
- `Reveal{path,line,column,select_to}` (`open_file`, go to line)
- `ProjectInfo{root,main,kind,engine,toolchain_ok}` (Setup, Toolchain)
- `BuildLog{max_bytes}` (`build_log`)
- `PdfText{page}` (`pdf_text.rs`)
- `PdfPage{page,width_px}`: liefert ein PNG im privaten Temp-Ordner (Crate `pdf` zum Rendern, `image` als PNG-Encoder; ist schon Abhängigkeit).
- `PdfInfo`
- `BibEntries` (`bib.rs` parse)
- `InsertText{text,replace_selection}` (`edit_from_outside`)
- `CopyIntoProject{source,dest}` (`resolve_path`, `atomic`)
- `ProposeEdits{path,edits|text,title}`: Antwort `Reviewed{accepted,rejected}` kommt, sobald alle Blöcke entschieden sind.
- `CancelReview`

### A4 Review-Ansicht (neu: `crates/app/src/review.rs`)
- Read-only `EditorView` mit Merge-Text: Alte Zeilen bekommen `DecorationKind::Deleted`, neue Zeilen `Inserted`. Wiederverwendet werden `editor_core/src/line_diff.rs` und die Decorations aus `editor_view.rs`.
- Das echte Dokument bleibt unangetastet, bis der User entscheidet.
- Annehmen eines Blocks = `edit_from_outside`, also ein Undo-Schritt. "Alle annehmen" ist zusammen ein einziger Undo-Schritt.
- Leiste nach Vorbild der Konflikt-Leiste in `git_sync.rs:580-631`: "Change 2 of 5", Accept (y), Reject (n), Next (`]c`), Accept all, Reject all. Die Befehle kommen in `COMMANDS`.
- Ändert sich das Dokument von außen (Kollaboration o. ä.), wird das Review verworfen und das Add-on bekommt einen Fehler.

### A4b Ziehen ins Dokument und Einfügen an einer Stelle (generisch)
- Neuer Node `Draggable{id,payload,children}`. Die Payload ist `Text{text}` (z. B. LaTeX-Snippet, BibTeX) oder `File{path}` (z. B. ein Plot-PNG).
- Ziehen aus dem Panel in den `EditorView` setzt den Cursor an der Drop-Position.
  - Eine Datei-Payload kopiert der Host über `CopyIntoProject` in `figures/` und fügt `\includegraphics` bzw. `#image(..)` ein. Das ist dieselbe Logik wie beim generischen Bild-Einfügen.
  - Eine Text-Payload wird direkt eingefügt.
  - Danach geht das Event `Dropped{id,path,line}` an das Add-on.
- Neuer Request `InsertAt{path,line,column,text}`.
- Ohne Add-on nutzt der Editor dieselbe Mechanik für Bilder, die aus dem Finder ins Dokument gezogen werden. Das ist ein generisches Feature ohne KI-Bezug.

### A4c An- und Abschalten sauber (Add-on-Host)
- Deaktivieren beendet den Prozess und entfernt das Panel, die Statusitems, die Commands und Shortcuts, die Kontextmenü-Einträge und offene Reviews (die Reviews werden verworfen).
- Aktivieren stellt alles ohne Neustart wieder her.
- Mit einem Test absichern: Kein Beitrag eines deaktivierten Add-ons ist in Keymap, Menü oder UI sichtbar.

### A5 Status-Items
- Neue Nachricht `SetStatusItems{items:[{id,text,tooltip,tone,options,selected}]}`, höchstens 8 Items.
- Sie erscheinen als Kopfzeile im Add-on-Panel. Die Statusleiste existiert heute nur im Vim-Modus; dort erscheinen sie zusätzlich.
- Items mit `options` öffnen das vorhandene Dropdown.

### A6 Commands, Shortcuts, Kontextmenü, Palette
- `[[commands]]` im Manifest, statisch: `id`, `label`, `keys`, `context`, `menu`. Damit kann `keymap.rs` die Shortcuts vor dem Prozessstart binden. Ein Command startet das Add-on bei Bedarf.
- `RunAddonCommand`: Die Befehle erscheinen in `keymap.json` als `addon.<id>.<cmd>` und im Shortcut-Dialog.
- Neues Rechtsklick-Menü im `EditorView` (generisch): Cut, Copy, Paste, darunter die Einträge der Add-ons. Vorbild ist das Menü-Muster in `workspace.rs:8281`.
- Command-Palette `PickerKind::Commands` in `picker.rs`.

### Reihenfolge der PRs
1. A0 und A1 (ohne virtuelles Scrollen)
2. A3-Lesezugriffe und A2
3. A4
4. A5
5. A6
6. Virtuelles Scrollen

Danach die `rev` von `addon_api` in `editor-ai` hochziehen.

## Phase B: editor-ai (eigenes Repo)

### Module
- `host.rs`: stdin/stdout und Request-Ids.
- `ui.rs`: baut die View einmal und arbeitet danach nur mit Insert, Append und Replace.
- `backend/mod.rs`: `trait Backend` mit start, send, interrupt, set_model, set_mode, set_effort und einem Event-Kanal.
- `backend/claude.rs`
- `channel.rs`
- `mcp.rs`: minimaler JSON-RPC-Server (initialize, tools/list, tools/call; Text und Bild).
- `sessions.rs`
- `tools.rs`

Abhängigkeiten nur `serde` und `serde_json`.

### Claude-Prozess
Aufruf:

```
claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages
  --model M --effort E --permission-mode P --session-id <uuid> | --resume <uuid>
  --mcp-config '{"mcpServers":{"editor":{"command":"<editor-ai>","args":["--mcp","--channel","<sock>"]}}}'
  --strict-mcp-config --tools Read,Glob,Grep,WebFetch,WebSearch,Bash
  --allowedTools 'mcp__editor__*' --permission-prompts host --append-system-prompt <LaTeX/Typst-Leitfaden>
```

- Arbeitsverzeichnis ist die Projektwurzel.
- `MCP_TOOL_TIMEOUT` wird hoch gesetzt, weil ein Review dauern kann.
- Niemals `--bare` oder `ANTHROPIC_API_KEY`, damit das eigene Abo genutzt wird.
- Der Prozess startet erst beim ersten Prompt. Nach 15 Minuten Leerlauf wird er beendet und beim nächsten Prompt mit `--resume` fortgesetzt.
- Modell und Modus werden live über `set_model` und `set_permission_mode` gewechselt. Für einen Effort-Wechsel startet der Prozess neu mit `--resume`.

### Privater Kanal
- Unix-Socket in `$TMPDIR/editor-ai-<pid>-<rand>/` mit Rechten 0700, abgesichert durch ein Token im env des Kindprozesses.
- Ein blockierender Accept-Thread, damit kein Idle-CPU entsteht.
- Das Kind `editor-ai --mcp` leitet Tool-Aufrufe an den Elternprozess weiter, der sie als Host-Requests an den Editor stellt.

### Tools `mcp__editor__*`
- Kontext und Lesen: `active_context`, `list_files`, `read_file` (inklusive ungespeicherter Änderungen), `search`, `diagnostics`, `build_log`, `compile`, `project_info`.
- Schreiben: `propose_edit` im Modus Ask, `edit_file` im Modus Auto, `create_file`, `insert_text`, `reveal`.
- PDF: `pdf_text`, `pdf_page` (als Bild).
- Zitationen: `bib_entries`, `add_bib_entry` (als Vorschlag), `insert_citation`.
- Bilder: `add_figure` legt das Bild in `figures/` und schlägt `\includegraphics` bzw. Typst `#figure(image(..))` vor.
- Die eingebauten Tools Edit und Write sind ausgeschlossen. Alle Änderungen laufen damit über den Editor, auch für ungespeicherte Dokumente.

### Rechte
- `can_use_tool` für `mcp__editor__*` wird automatisch erlaubt; die Sicherheit liegt im Review.
- Für Bash und WebFetch erscheint eine native Karte mit Allow, "Allow for this session" und Deny.
- Offene Anfragen werden beim Stoppen oder Schließen abgelehnt, damit Claude nie hängt.

### Modi (Dropdown)
- Ask: `default` und `propose_edit`.
- Auto: `acceptEdits` und `edit_file`.
- Plan: `plan`.

### Statuszeile
Anzeige: `Sonnet 4.x`, `Ask`, `effort high`, `ctx 34%`, `5h 12% · resets 14:30`, `week 40% · Mon`, `$0.42 (API-equivalent)`.

Quellen:
- Modell und Modus aus `system/init`.
- 5h- und Wochen-Limit aus `rate_limit_event`.
- Kontext über `get_context_usage`, einmal nach jedem `result`, nie gepollt.
- Kosten aus `result`.

Resets stehen als Uhrzeit da, nicht als Countdown, damit kein Timer laufen muss.

### Chat
- Nachrichten sind `Message`-Nodes; Antworten sind `Markdown`, gebündelt in etwa 30-ms-Schritten.
- Tool-Aufrufe und Thinking sind eingeklappte `Collapsible`s.
- Kontext-Chips zeigen Datei, Auswahl, Fehler, PDF-Seite und abgelegte oder eingefügte Bilder.
- Knöpfe: Stop, New, Copy, Retry.
- Sessions-Dropdown, gespeichert in einem eigenen Index (nicht in `~/.claude`).

### Onboarding ohne Vorwissen
Ablauf im Panel, ohne Terminal:
1. "Sign in with Claude"
2. Die Engine installiert sich, wenn sie fehlt: offizieller Installer von Anthropic in den Add-on-Ordner, mit Fortschritt im Panel.
3. `claude auth login` öffnet den Browser; der User meldet sich mit seinem Claude-Konto an.
4. Fertig.

Zustand prüfen mit `claude auth status`. Hinweis im UI: "Uses your Claude plan (Pro/Max). Usage counts toward its limits."

### Anhänge wie auf claude.ai
- Ins Panel gezogene oder eingefügte Bilder, PDFs, CSV- und Textdateien (Events `FilesDropped`/`ImagePasted`) werden zu Chips mit Vorschau.
- Beim Senden gehen sie als Inhaltsblöcke an `stream-json`: Bilder base64, PDFs als document-Block, Text inline.
- Die PDF-Seite des Editors lässt sich mit einem Klick anhängen ("Attach current PDF page" über `PdfPage`).

### Artefakte (wie Plots auf claude.ai, aber direkt nutzbar)
- **Plots und Abbildungen:** Claude schreibt sie bevorzugt nativ als pgfplots/TikZ oder Typst (cetz), damit kein Python nötig ist.
  - Neues Tool `render_figure{code,language}`: kompiliert den Code in einem Temp-Ordner mit dem installierten Compiler (Standalone-Klasse bzw. Typst) zu einem PNG.
  - Die Karte zeigt die Vorschau und die Knöpfe "Insert at cursor", "Save to figures/" und "Edit code"; die Karte lässt sich ins Dokument ziehen (`Draggable`).
  - Optional ein Python-Plot: Nur wenn `python3` mit matplotlib vorhanden ist und die Bash-Erlaubnis erteilt wurde; das Ergebnis-PNG wird zur selben Karte.
- **Aus Daten:** Eine abgelegte CSV wird zu pgfplots mit `\addplot table`. Die CSV landet in `data/` im Projekt, damit der Plot reproduzierbar bleibt.
- **Text, Tabellen, BibTeX:** Jeder Codeblock in einer Antwort bekommt "Insert at cursor" und "Copy" und ist ziehbar. Tabellen werden direkt als `tabular`/`booktabs` bzw. Typst `table` geliefert.
- **Bilder aus Dateien:** "Use as figure" kopiert das Bild nach `figures/` und schlägt eine `figure`-Umgebung mit Caption und Label vor (über das Review).

### Workflows
- Web-Recherche über WebSearch und WebFetch, mit Quellen als Links im Markdown.
- Zitation: DOI über Crossref als BibTeX holen, dann `add_bib_entry` (Review), dann `insert_citation`.
- Commands "Fix build errors with Claude" (Problemliste und Editor) und "Ask Claude about selection" (Rechtsklick und Shortcut).
- Compile-Fix-Schleife mit höchstens 3 Runden.
- Setup über `project_info`; Bash nur mit Erlaubnis.

Das Manifest bekommt `icon`, `description`, `api = 2` und `commands`.

## Phase C: MCP für externe Agents
- Editor-Repo, generisch: neues `crates/app/src/control.rs`.
  - Standardmäßig aus: keine Socket-Datei, solange die Einstellung "Allow other programs to control Editor" nicht aktiv ist.
  - Socket `~/Library/Application Support/Editor/control.sock`, Ordner 0700, Datei 0600, Prüfung mit `getpeereid` (gleicher Benutzer).
  - JSON-Zeilen mit denselben `addon_api`-Typen: `Hello{client,cwd}`, dann `Request`/`Reply`.
  - Beim ersten Mal fragt der Editor "<client> wants to control Editor".
  - Änderungen laufen immer als Review.
  - Die Dispatch-Logik aus `handle_addon_request` (`workspace.rs:1828`) wird für Panel und Socket gemeinsam genutzt.
- editor-ai: `editor-ai --mcp --socket` ist derselbe `mcp.rs`, nur über den Control-Socket.
  - Einrichtung: `claude mcp add editor -- <path>/editor-ai --mcp --socket`, für Claude Desktop analog.
  - Die MCP-Übersetzung bleibt KI-Code und damit in editor-ai.

## Phase D: CLI
- `main.rs`: Ist das erste Argument `--ctl`, läuft vor gpui ein Socket-Client.
- `packaging/macos/editor`: `goto|compile|diagnostics|selection|active|log|pdf-text|status` werden an `--ctl` weitergereicht, alles andere wie bisher an `open -a`.
- Pfade sind relativ zu `$PWD`; `--json` liefert Rohdaten. `editor compile` gibt `file:line: error: msg` aus und endet bei Fehlern mit einem Exit-Code ungleich 0.
- Ein kurzes CLAUDE.md-Snippet erklärt das; damit kann Claude Code im Terminal den Editor auch ohne MCP steuern.

## Risiken
- Bedingungen von Anthropic: Nur das eigene installierte `claude` mit eigenem Login starten, keine Tokens lesen oder weiterleiten. Hinweis im UI, dass die Nutzung zum Pro-Limit zählt.
- Die stream-json-Control-Requests sind halb-intern: getestete CLI-Version festhalten, Fähigkeiten aus `init` prüfen, notfalls mit Flags neu starten.
- Eine unbeantwortete `can_use_tool`-Anfrage lässt Claude hängen, deshalb immer beantworten.
- Lang blockierende Review-Tools brauchen ein hohes MCP-Timeout.
- Prompt-Injection über PDF, Bib oder Web: Im Modus Ask landet alles im Review, Bash fragt immer.
- gpui-Bild-Paste und `ExternalPaths`-Drop sind gegen die gepinnte Version zu prüfen.

## Kritische Dateien
- Editor:
  - `crates/addon_api/src/lib.rs`
  - `crates/app/src/addon_panel.rs`
  - `crates/app/src/workspace.rs` (`handle_addon_request` :1828, `addon_edit` :1930)
  - `crates/app/src/editor_view.rs` (`edit_from_outside` :363, Decorations :1913)
  - neu: `review.rs`, `markdown.rs`, `control.rs`
  - `status_bar.rs`, `actions.rs`, `keymap.rs`, `picker.rs`, `main.rs`, `packaging/macos/editor`
- editor-ai: `src/*`, `Cargo.toml` (rev), `addon.toml`.

## Verifikation
- Tests:
  - `cargo test -p addon_api`: Fixtures stimmen, v1-Manifeste werden weiter gelesen.
  - Markdown: Inkrementelles Parsen ergibt an jeder Trennstelle dasselbe wie ein voller Parse.
  - Review: Annehmen und Ablehnen entsprechen genau dem angewendeten `line_diff`.
- Performance:
  - Panel offen und Claude untätig: 0 % CPU, kein Render (gemessen mit `sample` und einem Render-Zähler).
  - 20.000 gestreamte Tokens: Frames unter 8 ms.
  - Chat mit 200 Nachrichten scrollt flüssig.
- Manuell mit dem echten Abo:
  - Prompt, gestreamte Antwort, Statuszeile füllt sich.
  - Modell, Modus und Effort umschalten.
  - Ein Review mit 5 Blöcken: 2 annehmen, 3 ablehnen, Undo.
  - Compile-Fix-Schleife.
  - DOI als Zitation einfügen.
  - Bild per Drop als Abbildung einbauen.
- Onboarding auf einem Konto ohne `claude`: "Sign in with Claude" installiert die Engine, öffnet den Browser-Login und landet im Chat. Kein Terminal.
- Screenshot ins Panel ziehen und fragen; PDF-Seite anhängen.
- "Plot sin(x) von 0 bis 2pi": Die Karte zeigt eine PNG-Vorschau, lässt sich an eine bestimmte Stelle ins Dokument ziehen, das Bild landet in `figures/` und das Dokument kompiliert.
- CSV ablegen und plotten lassen.
- Add-on in den Settings ausschalten: Panel, Menüeinträge, Shortcuts und Statusitems verschwinden, der Prozess ist beendet, und im Editor ist nichts mehr von KI zu sehen.
- Socket: aus, solange die Einstellung aus ist; ein anderer Benutzer wird abgewiesen.
- `claude mcp add` mit `editor-ai --mcp --socket`: Ein externer Claude Code behebt einen Compile-Fehler, das Review erscheint im laufenden Editor.
- `editor compile` liefert den richtigen Exit-Code.
