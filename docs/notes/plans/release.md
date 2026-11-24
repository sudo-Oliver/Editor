# Release-Plan: gründliches Audit, dann macOS + Windows x86 gemeinsam

## Context
Der erste öffentliche Release soll fertige Downloads für Leute bieten, die nichts selbst bauen. Entscheidungen des Users:
- **Mac und Windows erscheinen gemeinsam und später**, erst nach Tests.
- Windows muss **identisch aussehen, funktionieren und gleich schnell sein**.
- **Kein Geld fürs Signieren**: beide Builds unsigniert, kostenlose Wege sind ok (SignPath Foundation für Open Source).
- Vor jedem Push von Download-Builds kommt ein **akribisches Audit** auf Fehler und Performance.
- Ein Windows-PC steht später zum manuellen Testen bereit.

Stand: Lokal liegt eine universale Mac-DMG (38,6 MB, ad hoc signiert). Branch `feat/release-hardening` ist nicht gepusht: Problem-Melder, Collab-Grenzen, Setup-Timeout, Release-Workflow ohne Secrets. Das neue App-Icon (`packaging/macos/AppIcon.png`, `scripts/make-icon.swift`) ist noch nicht committet. PR #28 (Markdown) ist offen, CI grün.

Die Windows-Bestandsaufnahme zeigt: **Die App kompiliert heute nicht für Windows.** Gründe sind unix-only Prozessgruppen, `libc::kill`/`killpg`/`statvfs`, `symlink`, `MetadataExt`. Dazu kommt, dass Setup, Updater, Schlüsselbund, Paketierung und CI nur auf macOS existieren.

## Phase A — Mac-Audit (ohne Push)
1. Icon-Änderung committen. `make-icon.swift` erzeugt gleich auch eine `.ico` für Windows mit (siehe B8).
2. Statische Prüfung: `cargo clippy --workspace --all-targets -- -D warnings`, alle Funde beheben.
3. Alle Tests inklusive ignorierter auf dem Mac:
   - `cargo test --workspace`
   - Live-Tests: `math_render`, `toolchain::live` (`fresh_mac`, Probes), `collab many_editors`/`roles`
   - Benchmarks: `large_document`, `markdown::speed`
   - Zahlen in `docs/release-checks.md` festhalten.
4. Code-Review der Diffs von heute (`main...feat/release-hardening`) mit `/code-review high`. Alle bestätigten Funde fixen.
5. End-to-End mit der gebauten DMG, Screenshots per CGEvent-Klickhelfer:
   - DMG öffnen, Layout und Icon prüfen
   - App kopieren und mit frischem `EDITOR_HOME` starten (wie ein neuer User)
   - Setup-Assistent durchlaufen
   - LaTeX-, Typst- und Markdown-Projekt: kompilieren bzw. Vorschau
   - Fehlerkarte und Problem-Melder
   - Collab mit Headless-Gast (`cargo run -p collab --example guest`)
   - Icon im Dock in Hell- und Dunkelmodus
6. Gatekeeper-Anleitung: `First open.txt` auf macOS 26 prüfen und die README-Download-Sektion schreiben.

## Phase B — Windows-Port (Parität)
Bestehendes nach Plattform aufteilen statt doppeln. Neues Modul `crates/app/src/platform.rs` (plus `build`-Crate-Helfer) für alles Plattformabhängige. `windows-sys` steckt über gpui/tokio schon im Abhängigkeitsbaum, ist also kein neuer Ballast.

**B1 Kompilieren**
- Prozesse beenden: Prozessgruppe auf unix, Job Object auf Windows. Ein Helfer für `updater.rs`, `addons.rs`, `toolchain.rs`, `build/src/runner.rs`.
- `recovery.rs:78` Lebenscheck per `OpenProcess`.
- `setup.rs:958` freier Speicher per `GetDiskFreeSpaceExW`.
- `build/src/preamble.rs` auf `metadata.modified()` umstellen.
- `main.rs:56` Pfad aus URL ohne `OsStringExt`.
- Nur-mac-Module hinter cfg: `app_location`, `shell_command`, Teile von `updater` und `texlive`.
- `DirBuilderExt` und Test-Symlinks hinter `cfg(unix)`.
- Ziel: `cargo check --target x86_64-pc-windows-msvc` grün. `cargo-xwin` dient nur für schnelle Compile-Schleifen auf dem Mac. Bindings zu SDK-Headern (pdfium, windows-sys-Features, GPUI-DirectX mit `fxc.exe`) können beim Cross-Compile an Kleinigkeiten hängen. Für das finale Linken und Testen gilt allein der `windows-latest`-Runner von GitHub Actions.

**B2 Verhalten**
- Home-Ordner per `USERPROFILE`, Speicherorte nach `%APPDATA%` / `%LOCALAPPDATA%` (`storage.rs`).
- `find_tool` probiert die Endungen aus `PATHEXT` (`build/src/tools.rs`). Windows-Suchordner: TeX Live, MiKTeX, Git for Windows, Programme-Ordner der App.
- Ein Spawn-Helfer setzt `CREATE_NO_WINDOW` für alle Kindprozesse (git, Compiler, curl/tar).
- `#![windows_subsystem = "windows"]`, git-askpass bleibt per Pipe funktionsfähig.
- `file:///C:/` korrekt umwandeln.
- Pfadvergleiche ohne Groß/klein-Unterschied: synctex, Workspace-Root, Recovery.
- Eigener Helfer `strip_verbatim_prefix` (etwa 10 Zeilen, keine neue Crate) entfernt `\\?\` und `\\?\UNC\` aus `std::fs::canonicalize`. Er greift an allen rund 41 `canonicalize`-Stellen und vor allem vor jeder Pfadübergabe an pdflatex, latexmk, biber, bibtex, typst, tectonic und pandoc, denn TeX-Engines brechen bei `\\?\` sofort ab. Ein Test prüft, dass der Build-Runner keinen Pfad mit `\\?\` weitergibt.
- Log-Parser erkennt Laufwerkspfade (`build/src/parse.rs:243,269`).
- kpathsea-Trenner `;`, latexmk-Quoting unter `cmd.exe`, `GIT_CONFIG_GLOBAL=NUL`, `GIT_EDITOR` ohne `true`.
- Neue Zeilen übernehmen das Zeilenende der Datei (CRLF).
- Windows Credential Manager für Secrets (`secrets.rs`).

**B3 Gleiches Aussehen**
- Eigene Fensterknöpfe (Minimieren, Maximieren, Schließen) in der transparenten Titelleiste, jeder exakt als `WindowControlArea::{Min, Max, Close}` markiert. Die Zieh-Zone ist `WindowControlArea::Drag`. Nur mit diesem exakten Hit-Test funktionieren:
  - Snap Layouts von Windows 11 beim Hovern über Maximieren
  - Doppelklick auf die Zieh-Zone maximiert bzw. stellt wieder her
  - Ziehen an den Rand und Aero Shake
  - Abstand links für die Ampelknöpfe nur auf dem Mac
- Code-Schrift für beide Plattformen beilegen (JetBrains Mono, OFL), damit sie überall gleich ist. Menlo gibt es nur auf dem Mac.
- Markdown-Vorschau unter Windows mit Segoe UI.
- Texte pro Plattform: "Finder" / "Explorer", "Mac" / "PC", ⌘ / Ctrl über `keymap::label`. Betroffen sind rund 46 Stellen, darunter `tour.rs`, `git_panel.rs`, der Problem-Melder und `setup.rs`.
- Problem-Melder: Windows-Version und Architektur auslesen.

**B4 Setup unter Windows** (`toolchain.rs`, `setup.rs`)
- `release()`-Einträge für `x86_64-pc-windows-msvc`: Typst, Tectonic, Pandoc, je mit Prüfsumme und `.exe`-Pfad.
- Installation ohne Adminrechte in `%LOCALAPPDATA%\Editor\Tools`.
- Kleines LaTeX = Tectonic, wie auf dem Mac.
- Volles LaTeX = MiKTeX, per Benutzer und still installiert (`--unattended --private`). MiKTeX lädt fehlende Pakete selbst nach und ersetzt damit `texlive.rs`.
- Homebrew-Weg auf Windows ausblenden.
- Downloads nativ statt über `curl`/`tar`: über `zip` (schon Abhängigkeit) bzw. Windows-`tar`, in CI verifiziert.

**B5 Installer und Updater**
- Inno Setup (kostenlos), Installation pro Benutzer ohne Admin:
  - nach `%LOCALAPPDATA%\Programs\Editor`
  - Startmenü-Eintrag, Uninstaller
  - Dateizuordnungen tex/typ/md/bib
  - `editor.cmd` im PATH
- Updater-Zweig für Windows:
  - Asset `Editor-x.y.z-windows-x64-setup.exe` laden
  - SHA-256 aus den Release-Notes prüfen
  - Installer still ausführen, App neu starten

**B6 Eine Instanz**
Ein zweiter Start (Doppelklick auf eine `.tex`) reicht die Datei per Named Pipe an das laufende Fenster weiter, statt eine zweite App zu öffnen. Die erste Instanz schickt dabei ihre PID mit zurück. Instanz 2 ruft vor dem Beenden `AllowSetForegroundWindow(pid)` auf, damit das Fenster nach vorne kommt und nicht hinter dem Explorer liegen bleibt. Instanz 1 holt sich dann den Fokus. Test: Doppelklick im Explorer bei verdecktem Fenster, das Fenster muss vorne sein.

**B7 PDFium**
`fetch-pdfium.sh` bekommt `pdfium-win-x64.tgz` mit Prüfsumme, DLL aus `bin/`. `pdf_view.rs` sucht `pdfium.dll` neben der `.exe`.

**B8 CI und Release**
- `ci.yml`: Job `windows-latest` mit `cargo test --workspace`. Mac-only Tests sind gegated.
- `release.yml`: Job Windows mit
  - Release-Build
  - Icon und Versionsinfo per `winresource` (nur Build-Abhängigkeit)
  - Inno-Installer
  - Upload neben der DMG in den Draft-Release
- Collab: Hinweis auf die Windows-Firewall-Abfrage beim ersten Teilen.

**B9 Kostenlose Signatur**
- Antrag bei SignPath Foundation (Open-Source-Programm) vorbereiten.
- Bis zur Freigabe unsigniert, mit klarer SmartScreen-Anleitung ("Weitere Informationen" > "Trotzdem ausführen") im Installer-Download und in der README.

## Phase C — Windows-Test
- CI-Tests auf Windows grün.
- Dazu eine Schritt-für-Schritt-Checkliste `docs/windows-test.md` für deinen PC: dieselben E2E-Schritte wie A5, plus Installer, Update, Dateizuordnung und Firewall-Abfrage.
- Gefundene Fehler kommen über den Problem-Melder als Issues zurück und werden gefixt, bis die Checkliste komplett durchläuft.

## Phase D — Release
- PRs mergen: #28 und `feat/release-hardening`, getrennter PR `feat/windows`.
- Tag `v0.1.0` setzen. CI baut den Draft-Release mit DMG und Windows-Installer.
- Du prüfst den Draft und veröffentlichst selbst.

## Kritische Dateien
- Plattform-Helfer: `crates/app/src/platform.rs` (neu), `crates/build/src/tools.rs`, `crates/build/src/runner.rs`
- `crates/app/src/{main,storage,secrets,toolchain,setup,updater,texlive,app_location,shell_command,recovery,addons,pdf_view,theme,workspace,home_view,problem_report}.rs`
- `.github/workflows/{ci,release}.yml`, `scripts/fetch-pdfium.sh`
- Neu: `packaging/windows/{editor.iss,editor.cmd}`, `crates/app/build.rs`

## Verifikation
- **Mac:** A2 bis A5 grün, E2E-Screenshots ohne Auffälligkeiten, Benchmarks in `docs/release-checks.md` nicht schlechter als heute:
  - 1-MB-Markdown ≤ 1 ms pro Taste
  - Leerlauf 0 % CPU
  - Collab: 1200 Edits in < 0,2 s konvergiert
- **Windows:**
  - `cargo test --workspace` grün auf `windows-latest`
  - Release-Job erzeugt den Installer
  - `docs/windows-test.md` auf deinem PC vollständig abgehakt, inklusive:
    - Setup ohne vorinstallierte Tools, Kompilieren, Collab Mac↔Windows und Update
    - Snap Layouts am Maximieren-Knopf, Doppelklick auf die Titelleiste
    - Projekt in einem Ordnerpfad mit Leerzeichen und Umlauten (wegen `\\?\` und Quoting)
    - Doppelklick auf eine `.tex` bei verdecktem Fenster bringt die App nach vorne
- **Release-Draft:** beide Assets, Prüfsummen, Anleitungen für Gatekeeper und SmartScreen.

## Umfang
- Phase A: wenige Stunden.
- Phase B: mehrere Arbeitstage.
- Phase C: hängt vom Zugriff auf deinen PC ab.
- **Ein Release heute ist mit Windows-Parität nicht realistisch.**
