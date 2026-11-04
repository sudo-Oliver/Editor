# Editor

A native, local alternative to Overleaf for Typst, LaTeX and Markdown, written in Rust with GPUI.
The editor, the compilers and the PDF preview all run on your machine.

## Setup

The app calls external compilers. Install the ones you need:

- Typst: `typst`
- LaTeX: a TeX distribution with `latexmk` and `pdflatex` or `lualatex`, or `tectonic`
- Markdown: `pandoc` and `typst`

Then download the pinned PDFium build once and start the app with a project folder:

```sh
./scripts/fetch-pdfium.sh
cargo run --release -p app -- examples/overleaf-sample
```

Building on Linux also needs the system libraries that GPUI requires (X11, Wayland, Vulkan, fontconfig).
The Linux build has not been run yet.

## Compiling

Opening a project compiles it. Output goes to `.build/` inside the project.
Compile on save is the default. The Recompile menu also offers compile on type, which saves your files automatically after the delay.

Optional settings live in `.editor.toml`:

```toml
main = "paper.tex"      # file to compile, otherwise found automatically
engine = "lualatex"     # typst, tectonic, pdflatex, lualatex or pandoc
trigger = "type"        # save or type
debounce_ms = 300       # delay for compile on type
```

When a build fails, a card above the PDF names the file and line, shows the source and explains the error in plain language.
The last good PDF stays visible below it. If the compiler still produced a PDF, the card says that it may be incomplete.

LaTeX keeps state in files such as `.aux`, and a failed run can leave them broken.
The build after a failed or stopped run therefore starts without them, and the PDF is kept.
A build that fails where LaTeX reads those files runs once more without them.
The Recompile menu also offers "Recompile from scratch".

`-shell-escape` stays off and `latexmk` ignores `latexmkrc` files, because both can run commands from a document.

## PDF navigation

Navigation uses SyncTeX, so it works for LaTeX only. Typst and Pandoc do not produce SyncTeX data.

- Click a line in the editor to jump to it in the PDF. The Recompile menu can turn this off.
- Cmd+click (macOS) or Ctrl+click (Linux) in the PDF to jump to the source line.
- The position button in the PDF toolbar makes the PDF follow the editor cursor. It stays off until you turn it on, and Settings, Editor has the same switch.

These work for every engine:

- Drag over text in the PDF to select it, Cmd+C (macOS) or Ctrl+C (Linux) copies it.
- Click a link, such as a citation or `\ref`, to jump to its target. Cmd+[ or Alt+Left goes back, Cmd+] or Alt+Right forward again. Links to websites open in the browser, other kinds of external links stay closed.

## Images

Clicking a PNG, JPEG, WebP or SVG in the file tree shows it in place of the editor, with its resolution and file size. Copy include puts the matching code for the engine on the clipboard, for example `\includegraphics{images/photo.png}` or `#image("images/photo.png")`. Export saves a copy elsewhere. Cmd+W or selecting a tab closes the image.

## Toolbar

The buttons insert code in the language of the open file, LaTeX or Typst.

- The symbol palette holds templates, Greek letters, operators, relations and arrows. Outside a formula, a symbol gets `$` around it. The palette stays open until you press `Esc` or click elsewhere.
- The table button inserts a table of the size you pick in the grid.
- The figure button lists the project's images, and the citation button lists the entries of its `.bib` files.

After an insert, `Tab` moves to the next empty field. `Esc` or moving the cursor away ends this. In Vim mode, a button click switches to Insert mode.

### Suggestions

A popup at the cursor offers commands and names while you type, without AI or network access:

- LaTeX: after `\\` commands and your own `\\newcommand` macros, in `\\begin{` environments, and in `\\ref{` and its relatives the labels of the open documents.
- Typst: after `#` keywords, functions and your own `#let` names, after `@` the labels of the open documents.

While the popup shows, `Tab` accepts, `Up` and `Down` or `Ctrl+N` and `Ctrl+P` choose, and `Esc` closes it. `Enter` always starts a new line. Prose gets no popup, and `\\cite{` keeps its citation picker. In Vim mode, the popup works in Insert mode and `.` repeats an accepted suggestion. Turn it off with Settings, Editor, Auto suggestions, or `auto_suggestions = false` in `settings.toml`.

Some LaTeX output needs a package in the preamble, which the editor does not add: `graphicx` for figures, `svg` for SVG figures and `amsmath` for matrices and cases.

## Git

The branch button in the left rail shows the Git panel in place of the file tree. It uses the `git` program, so Git LFS, hooks, SSH keys, credential helpers and commit signing work as they do in a terminal.

- A folder without a repository offers to create one, with `.build/` in `.gitignore`.
- Click `+` and `-` to stage and unstage. Click a file to open it. `⌘↩` in the message field commits, and open documents are saved first.
- The branch name opens a list of branches. Type a new name to create one. Git refuses a switch that would overwrite changes.
- Pull only fast-forwards. Push publishes a new branch to `origin`, and never forces. Merges and conflicts belong in a terminal for now: the panel lists conflicts but does not resolve them.
- History shows all branches as a graph and loads more while you scroll.

Git runs only while the panel is open, never when a folder opens. Hooks and filters in a repository run as they would in a terminal, so open this panel only for repositories you trust. Build output in `.build` stays out of the lists. Changes appear on their own, but the panel does not fetch: pull or push to talk to a remote.

## Add-ons

An add-on is a program in its own folder, with an `addon.toml` that names it and the permissions it wants. The Add-ons button in the left rail lists what is installed. There you install an add-on from a folder, turn it on or off and remove it. An add-on that is on gets its own button in the rail, and its program starts when you first open that panel. Without an add-on turned on, nothing runs.

Add-ons have no sandbox. They run with your user rights, so install only those you trust. The permissions limit what they can ask the editor for, not what the program can do by itself. Through the editor, they reach only files inside the project and never `.git`.

The Add-ons panel also lists what the catalog offers, with each add-on's description and permissions before you install it. The catalog is the public repository [editor-addons](https://github.com/sudo-Oliver/editor-addons). The editor contacts it only when you first open the panel in a session or press Refresh, through `curl` over https. A failed request leaves a line in the panel and installed add-ons keep working. Downloads must match their sha256 and start turned off. An update stays on only if it asks for no new permissions.

There are no signatures. The checksum protects the download but does not say who wrote it, so installing means trusting that repository.

An add-on can supply entries for the citation picker. In LaTeX, typing `\cite{` or a relative such as `\autocite[p. 3]{` opens the picker when entries exist. Insert citation and Cmd+Shift+C open it in LaTeX and Typst.

An add-on with the `lookup_doi` permission can cite from a DOI: Cmd+Shift+D, Insert > Citation from a DOI, or `Space r` in Vim mode. The editor passes on a DOI from the selection or clipboard and draws the dialog of the add-on. With the cursor in a file that takes no citation, the key goes to the clipboard.

The crate `addon_api` describes the protocol: one JSON message per line over stdin and stdout, with the panel sent as a tree of nodes that the editor draws.

## Settings

The gear at the bottom left opens the settings, the question mark above it lists the keyboard shortcuts. Cmd+, (macOS) or Ctrl+, (Linux) also opens the settings. They are stored in `~/.config/editor/settings.toml`, or under `XDG_CONFIG_HOME` if it is set. Project settings stay in `.editor.toml`.

## Vim mode

Vim mode is off by default. Turn it on with Settings, Editor, Keybindings. It applies at once.

- Modes: Normal, Insert, Visual, Visual Line and Command, shown in a status bar.
- `Space w` saves, `Space x` closes the tab, `Space f` finds a file and `Space g` finds text in the project.
- `Tab` moves between editor and file tree. In the tree, `j` and `k` move, `l` or `Enter` opens and `h` closes a folder.
- `]d` and `[d` jump between errors. `H` and `L` switch tabs.

The question mark lists all keys while the mode is on. `/` searches literal text and ignores case unless the query has an uppercase letter. `Space g` reads files from disk, so unsaved edits do not count.

Not included: LSP keys, git hunks, macros, named registers, marks and custom key bindings.

## Crates

| Crate | Purpose |
| --- | --- |
| `app` | Window, panels, editor view, PDF view |
| `editor_core` | Rope buffer, multicursor, undo, Tree-sitter highlighting |
| `build` | Compiler runner, log parsers, project settings |
| `addon_api` | The messages between the editor and an add-on |
| `git` | Status, staging, commits, branches and history through the `git` program |
| `pdf` | PDFium worker thread |
| `synctex` | SyncTeX parser and lookups |
