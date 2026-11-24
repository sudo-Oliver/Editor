# Add-on-System und KI-Agent

## Context

Studierende kopieren LaTeX zwischen Browser-Chat und Editor hin und her, weil die KI das Projekt nicht kennt. Ziel: Agent mit Zugriff auf das ganze Projekt, der ändert, kompiliert und Fehler selbst korrigiert. Ohne Add-on bleibt die App exakt der schnelle Offline-Editor. Wordcount ist verworfen.

Keine Emojis in der App-Oberfläche. Kein Bloat im Kern: keine neue Crate im `app`-Binary, kein Prozess und keine Arbeit, solange kein Add-on läuft.

Korrekturen am Vorschlag:
- `⌘I` ist Italic. Shortcut wird `⌘⇧I`, in Vim `Space a` (frei).
- Ollama nutzt auf Apple Silicon die GPU, nicht die Neural Engine.
- Modellnamen sind frei wählbar statt fest eingebaut.
- Kein Index mit Embeddings: Agent sucht und liest bei Bedarf, das spart Kontext und Upload.

## Entscheidungen

- Reihenfolge: erst Plugin-System, dann KI als erstes echtes Add-on.
- Rechte: Nutzer wählt im Panel `Auto` oder `Manual`, jederzeit umschaltbar.
- Modelle: Claude-Abo über `claude`-CLI, Gemini, OpenAI-kompatible Cloud, lokal (Ollama, LM Studio, llama.cpp). Keine Anthropic-API.
- Panel in linker Leiste, damit PDF sichtbar bleibt.

## Offene Risiken

1. **Claude-Abo:** Anthropics Regeln verbieten Dritt-Entwicklern, Claude.ai-Login anzubieten oder Anfragen im Namen ihrer Nutzer über Free/Pro/Max zu leiten. `claude -p` selbst ist dokumentiert und zieht seit 15.06.2026 aus eigenem Agent-SDK-Guthaben im Abo. Plan: App berührt nie Tokens und bietet kein Login, sie startet nur das installierte `claude` des Nutzers. Für eigene Nutzung passt das aus meiner Sicht. **Vor öffentlicher Veröffentlichung im Marktplatz bei Anthropic klären**, das ist keine Rechtsauskunft.
2. **Gemini:** Headless-Modus mit `stream-json` ist dokumentiert. Nicht bestätigt: ob Login über Google-Abo headless geht, und ob eingebaute Werkzeuge abschaltbar sind. Vor Umsetzung mit installierter CLI prüfen. Fällt das aus: Gemini über OpenAI-kompatiblen Endpunkt mit Key aus AI Studio, dann voll gesteuert. `gemini` ist hier nicht installiert.

## Phase A: Add-on-Host

Native GPUI-Oberfläche lässt sich nicht dynamisch laden, Wasmtime wäre großer Ballast. Darum Add-on = eigener Prozess, JSON-RPC über stdio (wie LSP). Host zeichnet.

**`crates/addon_api` (neu):** Protokolltypen, Version `api = 1`. Nur `serde`, `serde_json` (liegt schon im Lockfile).

**Format:** `<config>/editor/addons/<id>/addon.toml`, Pfad wie `settings::default_path` (`crates/app/src/settings.rs`): id, Name, Version, Autor, Programm, Icon, Rechte.

**`crates/app/src/addons.rs`, `addon_panel.rs` (neu):**
- Prozess startet erst beim ersten Öffnen des Panels, endet beim Abschalten und App-Ende. Absturz zeigt Meldung und Neustart-Knopf.
- Anfragen an Host: `ui.set`, `ui.append` (Streaming ohne alles neu zu senden), `project.list`, `project.read` (offener Puffer vor Platte), `project.search` (`picker::search_text`), `project.diagnostics`, `editor.active`, `editor.edit`, `build.compile`, `secret.get/set`.
- Ereignisse an Add-on: Klick, Eingabe, aktive Datei, Build fertig.
- Host lehnt nicht deklarierte Rechte ab und hält alle Pfade im Projekt (kanonisiert, gegen Symlink-Ausbruch). `file_tree::scan` blendet Gitignore aus.
- View-Baum: Spalte, Zeile, Text, Code, Knopf, Eingabe, Auswahl, Trenner. Eingabe nutzt `EditorView` mit `set_plain` wie Commit-Box.
- `LeftPanel::Addon(id)`, Rail-Knopf (`render_rail`) nur für aktive Add-ons. Tastatur-Hook wie bei `GitPanel::contains_focus`.
- Geheimnisse: macOS `security -i`, Linux `secret-tool`, jeweils über stdin, nie als Argument, nie in `.editor.toml`. Sonst Umgebungsvariable.

**Einstellungen:** neue Seite `Add-ons` im Dialog (`SettingsPage`): Liste, an/aus, entfernen, `Install from folder`.

Wichtig und in der UI genannt: Add-ons sind native Programme mit Nutzerrechten, **ohne Sandbox**. Rechte-Prüfung regelt nur Host-API. Aktivieren zeigt Autor und Rechte.

## Phase B: KI-Add-on

**`crates/addon_ai` (neu, eigenes Binary):** Netz über `curl`, Key über `--config -` per stdin, damit nicht in `ps` sichtbar.

Werkzeuge: `list_files`, `read_file`, `search`, `diagnostics`, `edit_file` (alt/neu), `create_file`, `compile`. **Keine Shell, kein Netz.**

**Einheitliches Rechte-Tor:** Alle Modelle nutzen nur diese Werkzeuge.
- OpenAI-kompatibel (auch Cloud und lokal): Schleife im Add-on. Modell ohne Tool-Support: nur Chat mit aktueller Datei.
- `claude`-CLI: `-p --output-format stream-json --include-partial-messages`, `--tools` leer, `--strict-mcp-config`, `--mcp-config` zeigt auf `editor-ai --mcp`, das über Unix-Socket zum Add-on spricht. Abschalten der eingebauten Werkzeuge und Tool-Timeout beim Warten auf Klick zuerst am echten CLI prüfen.
- `gemini`-CLI: gleicher Aufbau, falls Prüfung oben klappt.

**Modi:**
- `Manual`: Lesen läuft, jedes `edit_file`, `create_file`, `compile` zeigt Karte mit Inhalt oder Diff und `Allow`/`Deny`.
- `Auto`: läuft durch bis fertig, kompiliert und korrigiert Fehler, begrenzt durch maximale Schrittzahl. Stop-Knopf immer da.

**Änderungen:** gehen in Puffer als Undo-Gruppe (`merge_undo_since`, `crates/editor_core/src/state.rs`), nie direkt auf Platte. Nur `compile` speichert, wie `recompile` heute. Panel listet geänderte Dateien pro Lauf mit `Discard`.

**Datenschutz:** Hinweis beim ersten Lauf je Cloud-Modell: Gelesene Dateien gehen an diesen Anbieter. Lokale Modelle bleiben offline.

**Prompt-Injection:** Text in `.bib` oder fremdem `.tex` kann Modell steuern. Darum im Auto-Modus Reichweite nur: Projektdateien plus Compile mit `-no-shell-escape`. Hinweis im Modus-Schalter.

Oberfläche: Modellwahl, Auto/Manual, Verlauf, Eingabe. `⌘⇧I`, `Space a`.

## Phase C: Marktplatz

Knopf `Install`: Registry-JSON (id, Version, Plattform, URL, SHA-256) per `curl`, Prüfsumme, Entpacken, Zustimmung mit Rechten. Updates per Vergleich.

Offen, vor Phase C zu klären: wo Registry und Binaries liegen, und Signaturen. SHA-256 aus derselben Quelle schützt nur vor kaputten Downloads, nicht vor kompromittierter Quelle. Dazu Risiko 1.

Pro Phase eigene Commits.

## Verifikation

1. `cargo test --workspace`, Clippy. Zusätzlich `cargo tree -p app` ohne neue Crates.
2. Host mit Test-Add-on: nicht deklariertes Recht abgelehnt, Pfad außerhalb abgelehnt, Symlink-Ausbruch abgelehnt, Absturz, langsame Antwort blockiert UI nicht.
3. Parser gegen aufgezeichnete Streams von `claude` und OpenAI-Format; Schleife gegen lokalen Fake-Server mit `curl`.
4. Rechte: Manual führt ohne `Allow` nichts aus; Key erscheint nie in `ps` oder `.editor.toml`; `claude`-Lauf nutzt nachweislich keine eingebauten Werkzeuge.
5. Echter Lauf auf Scratch-Kopie mit `claude`: Tabelle einfügen, absichtlich kaputtes LaTeX, Agent repariert bis Build grün. Verbraucht etwas Abo-Guthaben. Ollama und Gemini brauchen Installation durch User.
6. Performance: Ohne Add-on Idle-CPU und Binary wie vorher, kein Kindprozess. Fenster per Screenshot; Klicks muss User testen, solange Hilfszugriff fehlt.
