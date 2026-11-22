# Editor

A native, local alternative to Overleaf for Typst, LaTeX and Markdown, written in Rust with GPUI.
The editor, the compilers and the PDF preview all run on your machine.

## Install

Download `Editor-<version>.dmg` from the [releases](https://github.com/sudo-Oliver/Editor/releases), open it and drag Editor into Applications.
It runs on Apple silicon and Intel Macs with macOS 11 or later.
With Homebrew, `brew install --cask sudo-oliver/tap/editor` does the same and adds the `editor` command.

Files open from Finder (Open With, or drag them onto the Dock icon), and from the terminal like VS Code's `code`:

```sh
editor .            # this folder as a project
editor main.tex     # the file, in the project it belongs to
```

Without Homebrew's cask, Help, "Add editor command for Terminal…" adds the command.

On the first start, a short setup asks whether you write LaTeX, Typst or both, and installs exactly that. Nothing has to be on the Mac beforehand.
For LaTeX it offers the complete MacTeX (about 7 GB, every package offline) or the compact Tectonic (about 20 MB, fetches packages as documents need them).
It shows the download and disk size of every part and checks the free space before anything starts. Then it installs in one of three ways:

- **With Homebrew**, the default when Homebrew is already there: `brew install tectonic`, `typst` or `pandoc`.
- **In the app's folder**, the default when it is not. Nobody without Homebrew has to know what it is. The official programs go into the app's own folder (`~/.config/editor/tools`), are checked against a fingerprint, and can be removed under Writing tools. No password.
- **Install Homebrew first**, under "Other ways to install". Apple's developer tools through Apple's own dialog, then Homebrew's official installer, which asks for your password once, then the formulas.
- **By hand**, also under "Other ways to install". The setup lists each file with its download link and says where it goes: programs into `~/Applications/Editor Tools` (packed or unpacked), MacTeX through its own installer. It notices the files on its own, checks them and makes them runnable.

MacTeX always comes from its official package, checked against CTAN's checksum and its author's signature, and installed to `/Library/TeX` after macOS asks for your password.
With the complete LaTeX, the setup also installs the compact one first, so that you can start writing after about a minute; MacTeX takes over once its download is done.
Big downloads run in the background: you can start writing at once, the status bar shows how far it is, and the first PDF follows once the compiler is there.
At the end, a test document compiles in each chosen language, so that the setup only says "all set" when it works.
To add a language later, open the setup again from the project hub. Pandoc, which only the Word and HTML exports need, installs on the first such export.
If you already have MacTeX, Typst or Homebrew, the setup sees them and the app uses them.

Git and GitHub are not part of the setup. The git panel sets them up when you want them: "Set up git" opens Apple's dialog for its developer tools, and "Put it on GitHub" signs you in through the browser, creates a private repository and pushes.
Then a welcome document opens and a tour shows each part of the editor. Help, Editor tour plays it again.

## Updates

The app asks GitHub for the newest release a few seconds after it starts and then every six hours. When there is a newer one, a bar offers "Update and restart". The app downloads the dmg, checks it against the checksum GitHub lists for it, its bundle identifier, its version and its signature (the same Apple team as the running app), puts it in place of the old app and starts again. Open files are saved first. If anything fails, the old app stays as it was.
An app installed with the Homebrew cask updates through `brew upgrade --cask` instead, so that Homebrew stays in step.
Only an app bundle updates itself, never a build run with cargo. The only request is one to `api.github.com`, with nothing about you in it.

## Building a release

`scripts/bundle-macos.sh` builds `target/bundle/Editor.app` for both architectures and packs it into a dmg.
Without signing details it signs ad hoc, which is enough for your own Mac.
A release needs an Apple Developer ID; the script header lists the environment variables, and `.github/workflows/release.yml` reads them from repository secrets when a `v*` tag is pushed.
The workflow creates a draft release. Publishing the draft is what the update bar of installed apps sees; the version comes from `Cargo.toml` and must be higher than the last release.

## Setup for development

The app calls external compilers. Install the ones you need, or let the app install Typst and Tectonic for you:

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

Clicking a PNG, JPEG, WebP or SVG in the file tree opens it in a tab, with its resolution and file size. Copy include puts the matching code for the engine on the clipboard, for example `\includegraphics{images/photo.png}` or `#image("images/photo.png")`. Export saves a copy elsewhere. Cmd+W closes the tab.

## Toolbar

The buttons insert code in the language of the open file, LaTeX or Typst.

- The symbol palette holds templates, Greek letters, operators, relations and arrows. Outside a formula, a symbol gets `$` around it. The palette stays open until you press `Esc` or click elsewhere.
- The table button inserts a table of the size you pick in the grid.
- The figure button lists the project's images, and the citation button lists the entries of its `.bib` files.

After an insert, `Tab` moves to the next empty field. `Esc` or moving the cursor away ends this. In Vim mode, a button click switches to Insert mode.

### Files

The buttons in the file tree's header make a new file or folder, add files from elsewhere and close the panel. New entries go to the last clicked folder, or the folder of the last clicked file. Nothing gets replaced: a name that exists shows an error, and added files with such a name are left out. Clicking the header folds the tree, so that the outline gets its space.

### File menu

The File menu creates files and folders (New file with Cmd+N), adds files from elsewhere, and copies the open file as `a-copy.tex`, with its unsaved changes. Version history opens the Git panel with the history showing.

Word count works from the main file through `\input`, `\include`, `\subfile` and `#include`, and open documents count with their unsaved text. Comments, math, code, labels, citations and the preamble do not count, and neither does text that only macros or Typst functions generate. Files it cannot find are named.

Downloads never change the project, and a failed one leaves an existing file as it was:

- **PDF** copies the preview byte for byte, so its layout is the same. If the PDF has errors, is out of date or has undefined references, a dialog says so first and offers to recompile.
- **Source (.zip)** holds everything except `.git`, `.build` and `.DS_Store`, also what `.gitignore` lists, because a build can need it. Files that may hold secrets, such as `.env`, are asked about. Every entry is compared with its original before the archive is saved. A file that cannot be read stops the export. Typst `@preview` and Tectonic packages download at compile time and are not in the archive.
- **Word, HTML and Markdown drafts** come from Pandoc, which has to be installed. They do not keep the layout: page margins, the document class and fonts are lost. A dialog lists what Pandoc reports as left out, before the draft is saved. Pandoc does not report everything for Typst, such as shapes and show rules, so compare with the PDF. A Typst project with `@preview` packages only converts once they are in the local cache.

### Packages

LaTeX errors about a missing package get a button that adds `\\usepackage` to the preamble of the main file. After each build, the editor also looks through the whole project:

- A command or environment from a known package, with that package not loaded, gets a quiet underline.
- A package that nothing in the project uses gets one too. This covers only packages whose commands are all known and that change nothing by being loaded, such as `booktabs`.

The editor stays silent when it cannot be sure: with an unknown document class or package, with `.sty` or `.cls` files in the project, or when the project defines the name itself. The compiler's error still has the button then.

`Cmd+.` applies the fix on the line of the cursor, in Vim's Normal mode also `Space .`. A strip below the editor explains the underline on the cursor's line. All details lists every hint with its button. Undo reverts a fix. Turn the underlines off with Settings, Editor, Package hints, or `package_hints = false` in `settings.toml`.

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
- `Tab` moves between editor and file tree. In the tree, `j` and `k` move, `l` or `Enter` opens and `h` closes a folder. `a` makes a new file, `A` a new folder and `u` adds files from elsewhere.
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
