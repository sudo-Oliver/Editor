# Working notes

What used to live only in Claude's memory on the Mac: the rules for this
project, where each piece of work stands, and the plans behind it. Read this
first in a new session, on any machine.

## Current priority (2026-11-24)

No new features. The one goal before the first release (v0.1.0) is a
Windows version that works perfectly and matches the Mac app. Work happens
on the Windows PC from now on; test with [../windows-test.md](../windows-test.md).

The first release ships without add-ons: they are not ready for users yet.
The `addons` cargo feature of `crates/app` is off by default, which hides the
Add-ons panel, the "Cite from a DOI" command and starts no installed add-on.
`cargo run -p app --features addons` brings it all back for development.

Known Windows problems, to fix first:

- "Set up git" in the git panel calls `xcode-select --install`
  (`toolchain::install_developer_tools`), which does not exist on Windows. It
  should install Git for Windows (winget `Git.Git`) or say where to get it.

## Rules for the project

- **Lightweight, no bloat.** Keep the features that were asked for, drop the
  extras nobody asked for. Lazy work, no new crates, no repaint without a
  change, zero CPU when idle. Measure before and after.
  - Exception: live collaboration is in the core (`crates/collab`, yrs and
    iroh), behind a cargo feature, and starts nothing until a session opens.
- **No emojis** in any text inside the app, also when a pasted spec has them.
  Use the drawn icons in `crates/app/assets/icons` or plain text.
- **Add-ons in their own repos.** The AI add-on lives in
  [sudo-Oliver/editor-ai](https://github.com/sudo-Oliver/editor-ai). The editor
  repo keeps only the add-on host, the protocol (`crates/addon_api`) and the
  marketplace. No AI code, backends or AI shortcuts in this repo.
- **Citations: pure language support in the core, services in add-ons.** The
  core covers every LaTeX and Typst citation and reference feature (all cite
  commands, .bib and Hayagriva, labels, multi-file projects), fast and
  correct, since citation errors are fatal in books and theses. Zotero,
  DOI/arXiv lookup and clipboard import go to add-ons.
- **Problem reports go to GitHub issues** of sudo-Oliver/Editor, never email.
  A GitHub account is the one requirement; everything technical is prefilled,
  the user only describes the problem in their own words.
- **Setup for a fresh machine.** Someone who downloads from GitHub Releases,
  with no terminal knowledge, must end with a working LaTeX and/or Typst
  after the setup, first time, and fast (an hour is unacceptable). Users wait
  in the setup until a test compile passed, with clear progress. Always a
  manual path too, which says exactly where each file goes. Show size and
  purpose of big installs (MacTeX, MiKTeX) first. Git is not a setup step,
  but connecting it from the git panel must be one click.
- **Release:** macOS and Windows x86_64 ship together. Windows must look, work
  and perform like the Mac app. Both unsigned (no money for certificates);
  free options such as the SignPath Foundation are fine. Clear first-open
  guides for Gatekeeper and SmartScreen.

## AI add-on

- For people who use the Claude app or claude.ai, not Claude Code. Its UI
  never says "Claude Code", "CLI" or "Terminal"; onboarding is one "Sign in
  with Claude" button, and the add-on installs and logs in the engine itself.
- Works without an Anthropic API key (Claude subscription, also Gemini).
- Feels like claude.ai in the editor: drop or paste images, PDFs and CSVs
  into the chat; plots, figures, tables and BibTeX come back as cards with a
  preview and Insert at cursor / Save to project / drag into the document.
- Strictly opt-in: without the enabled add-on the editor shows nothing AI.
  Settings > Add-ons turns it on and off, all at once.
- State: editor-ai v0.4.0 on `main` speaks add-on API 1 (Claude through the
  installed `claude` with stream-json, OpenAI-compatible APIs, local models,
  MCP tools, one approval gate, usage limits, model and effort dropdowns).
  Its branch `feat/protocol-v2` moves the panel to add-on API 2 and is not
  merged yet. The editor side of API 2 is merged.
- Plan: [plans/claude-addon-protocol-v2.md](plans/claude-addon-protocol-v2.md).

## Live collaboration and review

Built and in the repo:

- `crates/collab`:
  - `Project` is the yrs CRDT, owned by the UI thread in step with the editor.
  - `Session` is transport only, with a replica for syncing newcomers and
    checking roles (Editor, Reviewer, Reader). Reader updates are dropped,
    their comments go through `Intent`. Reviewer updates must pass
    `apply_suggestions_only`, or the reviewer is banned.
  - `track()` turns edits into suggestion ops plus restores, so deletions
    stay struck through. `ReviewFile` is the `.review` format.
- App: `crates/app/src/collab_session.rs`. Review state is saved in the
  project at `.editor/review.ydoc` and `.editor/review.json`, 2 s after a
  change.
- Headless guest for tests: `cargo run -p collab --example guest -- <code>`.

Open on purpose:

- Reviewer enforcement detects violations but cannot undo them; a CRDT
  cannot roll back.
- Guests cannot rejoin after the host quits.
- Files changed outside the editor while it was closed are not synced live.
- The relay URL is not configurable.

Goal behind it: the local-first alternative to Overleaf, also for thesis
reviews with supervisors.

## Windows pitfalls

- `cargo-xwin` only for quick compile loops; final linking and tests on the
  `windows-latest` runner.
- Strip `\\?\` from `canonicalize` results, above all before paths go to
  pdflatex, latexmk, biber, typst, tectonic or pandoc; TeX aborts on them.
- Caption buttons must be exact `WindowControlArea::{Min, Max, Close}` plus a
  `Drag` area, or Snap Layouts and double-click maximize break.
- Single instance through a named pipe: the second instance calls
  `AllowSetForegroundWindow(pid of the first)` before it exits, or the window
  stays behind Explorer.
- The manual checklist is [../windows-test.md](../windows-test.md).

## Branches not merged

- `perf/no-waiting`: five perf commits (disk, PDFium and network off the main
  thread, drawing only what shows, one sha2, stripped release). Branched off
  an old `main`; merging conflicts in 9 files. Worth porting piece by piece.
- `chore/release-without-apple`: publishing from the Mac without a Developer
  ID. Superseded by the release workflow, which builds unsigned on CI.

## Developing on Windows

1. Install [rustup](https://rustup.rs) (the repo pins Rust 1.98.1 in
   `rust-toolchain.toml`), the Visual Studio Build Tools with "Desktop
   development with C++", and Git for Windows.
2. In Git Bash: `scripts/fetch-pdfium.sh` (puts `pdfium.dll` into
   `vendor/pdfium/bin`).
3. `cargo run --release -p app -- examples/overleaf-sample`

## Releasing

Push a tag `vX.Y.Z` (the version in `Cargo.toml` must match and be higher than
the last release). `.github/workflows/release.yml` builds the universal dmg
and the Windows installer and attaches both to a draft release. Test the
draft's downloads, then publish it; installed apps see only published
releases.

## Plans

| Plan | Topic |
|------|-------|
| [release.md](plans/release.md) | Audit, Windows port, first release |
| [claude-addon-protocol-v2.md](plans/claude-addon-protocol-v2.md) | Add-on protocol v2, editor-ai, MCP |
| [addon-system.md](plans/addon-system.md) | Add-on system and AI agent |
| [citations.md](plans/citations.md) | Citations and references in the core |
| [git-integration.md](plans/git-integration.md) | Native git integration |
| [pdf-panel.md](plans/pdf-panel.md) | PDF panel: selection, links, auto-follow |
| [performance-audit.md](plans/performance-audit.md) | Performance and bloat audit |
| [latex-package-suggestions.md](plans/latex-package-suggestions.md) | Suggestions for any LaTeX package |
| [fix-keystroke-panic.md](plans/fix-keystroke-panic.md) | Fix for the `secondary-a` keystroke panic |
