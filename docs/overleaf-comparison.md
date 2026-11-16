# Compared with Overleaf

This lists what Overleaf does that the editor does not, or does better. Each gap is its own piece of work, so the list is meant to be taken one item at a time. Paths are relative to `crates/`.

## Editing

- **Find and replace.** Overleaf has Cmd+F with replace, regular expressions and search across the project. Here, search exists only in Vim mode (`/` in a file, `Space g` across the project). The project search is literal, reads files from disk, so unsaved edits do not count, and stops at 200 results (`app/src/picker.rs`). There is no replace and no `:s`.
- **Spell checking.** Overleaf checks spelling in many languages. The editor has none.
- **Files.** Overleaf renames, deletes and moves files and folders, and takes uploads by drag and drop. The file tree here only creates files and folders, adds files from elsewhere and makes copies (`app/src/file_tree.rs`). Images cannot be pasted.
- **Visual editor.** Overleaf can show the document as formatted text. The editor shows source only.
- **Code folding and bracket matching.** Overleaf folds environments and highlights the matching bracket. Here, brackets are only paired while typing, and `%` jumps in Vim mode.
- **Undo across restarts.** Undo history lives in memory and ends when the app closes. Unsaved text does survive a crash through recovery snapshots.

## Compiling

- **A full TeX installation.** Overleaf runs a complete TeX Live, so no package is ever missing. Here, a package must be in the installed distribution. A missing one gets a button that adds `\usepackage`, but nothing installs it (`build/src/explain.rs`). Only Tectonic and Typst download packages by themselves.
- **XeLaTeX.** Overleaf offers it, here the engines are Typst, Tectonic, pdfLaTeX, LuaLaTeX and Pandoc (`build/src/config.rs`).
- **Bibliographies, indexes and glossaries.** Overleaf runs BibTeX, Biber, makeindex and makeglossaries as needed. Here, they run only through latexmk. Without latexmk they do not run, and glossaries never build, because latexmk runs with `-norc`.

## Collaboration and history

- **Comments and track changes.** Overleaf keeps them in every project. Here, they exist only during a live session, and comments and suggestions appear to be gone when it ends.
- **Chat.** Overleaf has a project chat. The editor has none.
- **Sharing.** An Overleaf link works at any time. A live session here needs the host to stay online.
- **History.** Overleaf saves versions on its own, with labels and a diff view. Here, history is Git: versions exist only when someone commits, and merge conflicts belong in a terminal.

## Projects

- **Templates.** Overleaf has thousands, from journals, universities and conferences. The editor ships Empty LaTeX, Empty Typst and Empty project (`app/src/projects.rs`).
- **Import.** Overleaf takes a zip upload. Here, only folders open, so an Overleaf download has to be unzipped first.

## PDF

- **Search in the PDF.** Overleaf can, the editor cannot.
- **Page overview.** Overleaf shows thumbnails and the PDF outline. The editor has neither.
- **SyncTeX for Typst.** Neither has it, but in Overleaf it is not needed because Typst is not offered.

## The app itself

- **Platforms.** Overleaf runs in any browser. The editor is tested on macOS with Apple silicon. Linux has not been run, and the code does not compile on Windows (process groups and signals in `build/src/runner.rs`, `HOME` paths, no `pdfium.dll`).
- **Languages.** Overleaf's interface is translated. Here, all text is English and written into the code.
- **Accessibility.** Overleaf works with screen readers. The editor has no accessibility support.
- **Key bindings.** Overleaf lets you choose Vim or Emacs keys. The editor has Vim mode but no custom bindings.
- **Testing.** There is no CI. `app/src/workspace.rs` holds most of the app's behavior with few tests.

## Where the editor is ahead

- Everything runs on your machine, also offline, without compile time limits or a cap on collaborators.
- Typst is supported.
- Build errors are explained in plain language, with the file and line, and the last good PDF stays visible.
- Missing and unused packages are pointed out before the compiler complains.
- Vim mode, crash recovery, live sessions without a server, real Git and add-ons.
