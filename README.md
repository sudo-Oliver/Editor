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
| `pdf` | PDFium worker thread |
| `synctex` | SyncTeX parser and lookups |
