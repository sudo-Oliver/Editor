# Editor

A native, local alternative to Overleaf for Typst, LaTeX and Markdown, written in Rust with GPUI.
The editor, the compilers and the PDF preview all run on your machine.

## Install

**Mac:** download `Editor-<version>.dmg` from the [releases](https://github.com/sudo-Oliver/Editor/releases), open it and drag Editor into Applications.
It runs on Apple silicon and Intel Macs with macOS 11 or later.
With Homebrew, `brew install --cask sudo-oliver/tap/editor` does the same and adds the `editor` command.

**Windows:** download `Editor-<version>-windows-x64-setup.exe` from the same page and open it. It installs for you alone, without an administrator, adds Editor to the Start menu and the `editor` command to the PATH, and lets `.tex`, `.typ`, `.bib` and `.md` files open in it. It runs on 64-bit Windows 10 and 11.

Neither build is signed with a paid certificate, so macOS and Windows warn the first time. [First open](#first-open) shows the two clicks past the warning.

Files open from Finder or Explorer (Open With, a double click, or drag them onto the app), and from the terminal like VS Code's `code`:

```sh
editor .            # this folder as a project
editor main.tex     # the file, in the project it belongs to
```

On the Mac without Homebrew's cask, Help, "Add editor command for Terminal…" adds the command.

On the first start, a short setup asks whether you write LaTeX, Typst or both, and installs exactly that. Nothing has to be on the computer beforehand.

On Windows the setup installs with winget, which comes with Windows, or into the app's folder (`%LOCALAPPDATA%\Editor\Tools`), without an administrator. The compact LaTeX is Tectonic, the complete one MiKTeX, which fetches missing packages on its own.

On the Mac:
For LaTeX it offers the complete MacTeX (about 7 GB, every package offline) or the compact Tectonic (about 20 MB, fetches packages as documents need them).
It shows the download and disk size of every part and checks the free space before anything starts. Then it installs in one of three ways:

- **With Homebrew**, the default when Homebrew is already there: `brew install tectonic`, `typst` or `pandoc`.
- **In the app's folder**, the default when it is not. Nobody without Homebrew has to know what it is. The official programs go into the app's own folder (`~/Library/Application Support/io.github.sudo-oliver.editor/tools`), are checked against a fingerprint, and can be removed under Writing tools. No password. The packages Tectonic and Typst download go to `~/Library/Caches/io.github.sudo-oliver.editor`, and removing either removes its packages too.
- **Install Homebrew first**, under "Other ways to install". Apple's developer tools through Apple's own dialog, then Homebrew's official installer, which asks for your password once, then the formulas.
- **By hand**, also under "Other ways to install". The setup lists each file with its download link and says where it goes: programs into `~/Applications/Editor Tools` (packed or unpacked), MacTeX through its own installer. It notices the files on its own, checks them and makes them runnable.

MacTeX always comes from its official package, checked against CTAN's checksum and its author's signature, and installed to `/Library/TeX` after macOS asks for your password.
MacTeX is the recommended LaTeX: you wait once for its download, about 15 minutes on a fast connection, and the setup shows how long it still takes. After that everything works offline.
At the end, a test document compiles in each chosen language, so that the setup only says "all set" when it works.
To add a language later, open the setup again from the project hub. Pandoc, which only the Word and HTML exports need, installs on the first such export.
If you already have MacTeX, Typst or Homebrew, the setup sees them and the app uses them.

Git and GitHub are not part of the setup. The git panel sets them up when you want them: "Set up git" opens Apple's dialog for its developer tools, and "Put it on GitHub" signs you in through the browser, creates a private repository and pushes.
Then a welcome document opens and a tour shows each part of the editor. Help, Editor tour plays it again.

## First open

### On the Mac

Editor is free and not registered with Apple, so macOS warns the first time you open it. That is expected, and it asks only once.
The dmg has the same steps in "First open.txt".

1. Drag Editor onto the Applications folder in the dmg window, then open Editor from Applications.
2. macOS says it cannot check Editor for malicious software. Click **Done**.
   <!-- TODO: screenshot of the warning -->
3. Open **System Settings**, then **Privacy & Security**, and scroll down to "Editor was blocked to protect your Mac". Click **Open Anyway** and confirm with your password or Touch ID.
   <!-- TODO: screenshot of Privacy & Security with Open Anyway -->
4. Confirm once more in the last question. From now on Editor opens like any other app.

On macOS 14 and earlier, right-click Editor in Applications, choose **Open**, and click **Open** in the question instead.
If you are at home in Terminal, this line does the same in one step:

```sh
xattr -dr com.apple.quarantine /Applications/Editor.app
```

If Editor starts from the dmg or from Downloads, a bar offers to move it to Applications. It copies itself there, starts again and ejects the dmg; from Applications it can update itself.

### On Windows

Editor is not signed with a paid certificate yet, so Windows SmartScreen warns the first time you open the installer. That is expected.

1. Open `Editor-<version>-windows-x64-setup.exe` from your Downloads.
2. Windows says "Windows protected your PC". Click **More info**.
3. Click **Run anyway**. The installer starts; it needs no administrator password.

The first time you start a live session, Windows asks whether Editor may use the network. Allow it, so that others can reach you.

## Updates

The app asks GitHub for the newest release a few seconds after it starts and then every six hours. When there is a newer one, a bar offers "Update and restart". The app downloads the dmg, checks it against the checksum GitHub lists for it, its bundle identifier, its version and its signature (intact, and from the same Apple team as the running app when that one has a team; a release signed ad hoc has none), puts it in place of the old app and starts again. Open files are saved first. If anything fails, the old app stays as it was.
On Windows the app downloads the new installer, checks it against the checksum GitHub lists for it, and runs it once the app has closed; the installer starts the new version.
An app installed with the Homebrew cask updates through `brew upgrade --cask` instead, so that Homebrew stays in step.
Only an app bundle updates itself, never a build run with cargo. The only request is one to `api.github.com`, with nothing about you in it.

### LaTeX updates

MiKTeX on Windows updates through its own console. MacTeX keeps itself current from inside the app, without Homebrew or the Terminal.

- Once a week the app asks TeX Live's package manager, `tlmgr`, which packages have newer versions. Asking needs no password, and without internet it simply tries again later.
- Settings, Writing tools shows the installed TeX Live year, when it last checked and how many updates there are. "Update LaTeX packages" first says why the password is needed, then macOS asks for it once, since MacTeX belongs to the system. `tlmgr` then updates itself and every package. Closing the password dialog changes nothing.
- When a document stops because a package is missing or too old, the error offers the same update.
- Each spring tug.org publishes the TeX Live of the new year, which the old one cannot update to. The weekly check notices, since `tlmgr` then names the new year. The app offers "Install MacTeX 2027" (or whichever year), the same download as in the setup. Your LaTeX keeps working until the new one is installed.
- The old year stays on the disk until you remove it: the settings show how much space it takes and remove it after asking for confirmation and your password. Nothing is ever deleted on its own.

## Building a release

Pushing a tag such as `v0.1.0` runs `.github/workflows/release.yml`. It builds the universal dmg on macOS and the Windows installer with Inno Setup, and attaches both, with the Windows checksum and the Homebrew cask, to a draft release.
The version comes from `Cargo.toml` and must be higher than the last release. Test the downloads of the draft, with [docs/windows-test.md](docs/windows-test.md) on a PC, then publish it: installed apps only see published releases.

Without signing details the Mac app is signed ad hoc. With an Apple Developer ID, the workflow reads the certificate and notarization details from repository secrets; the header of `scripts/bundle-macos.sh` lists them.
Locally, `scripts/bundle-macos.sh` builds `target/bundle/Editor.app` for both architectures and the dmg. For a Windows installer of a branch, run the Windows preview workflow by hand, or label a pull request `windows-preview`.

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

On Windows, install [rustup](https://rustup.rs), the Visual Studio Build Tools with "Desktop development with C++", and Git for Windows, then run the same two lines in Git Bash. `rust-toolchain.toml` pins the Rust version.
Working notes, the rules of the project and the plans behind it are in [docs/notes](docs/notes/README.md).

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
- Cmd+click (macOS) or Ctrl+click (Windows, Linux) in the PDF to jump to the source line.
- The position button in the PDF toolbar makes the PDF follow the editor cursor. It stays off until you turn it on, and Settings, Editor has the same switch.

These work for every engine:

- Drag over text in the PDF to select it, Cmd+C (macOS) or Ctrl+C (Windows, Linux) copies it.
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
- Pull merges what others pushed. A merge that stops at conflicts lists the files; solve them in the editor, then finish the merge in the panel. Push publishes a new branch to `origin`, and never forces.
- History shows all branches as a graph and loads more while you scroll.

Git runs only while the panel is open, never when a folder opens. Hooks and filters in a repository run as they would in a terminal, so open this panel only for repositories you trust. Build output in `.build` stays out of the lists. Changes appear on their own, but the panel does not fetch: pull or push to talk to a remote.

## Markdown

`.md` files get a live preview next to the text instead of a PDF: tables, images, the title from the front matter, and display formulas typeset with the installed LaTeX. The preview follows the cursor, and a double click jumps to the source. The toolbar inserts tables, code, images, symbols and `[@key]` citations; a `bibliography:` in the front matter feeds the citation picker. Word, HTML and PDF exports go through Pandoc.

## Citations and references

The editor reads every `.bib` and Hayagriva file of the project, also across files that `\input` or `#include` each other.

- `\cite{`, `@` in Typst and `[@` in Markdown open the citation picker; Insert citation (Cmd+Shift+C) opens it anywhere. It searches author, title and year, and cites several keys at once.
- Unknown keys and labels, and the same work under two keys, are underlined while you type. Problems from BibTeX or Biber's `.blg` appear at their entry.
- Hovering a key or label shows its entry or the text around it. F12 or Cmd+click goes to its definition.

## Live sessions and reviews

Start live session (the people button in the left rail) shares the open project with others, without a server or an account: they join with the invite code, and everyone edits the same files at once. Each person joins as Editor, Reviewer or Reader. A reviewer's changes stay suggestions with struck-through deletions until you accept them; readers can only comment. The review is saved in the project under `.editor`, so it outlasts the session.

## Reporting a problem

Help, Report a problem opens a prefilled GitHub issue with the app version, the system and the last build log. You only describe what happened. It needs a free GitHub account.

## Add-ons

Add-ons are in development and not part of the releases. A build with `cargo run -p app --features addons` shows the Add-ons panel; the protocol is in the crate `addon_api`, and add-ons live in their own repositories.

## Settings

The gear at the bottom left opens the settings, the question mark above it lists the keyboard shortcuts. Cmd+, (macOS) or Ctrl+, (Windows, Linux) also opens the settings. They are stored in `~/Library/Application Support/io.github.sudo-oliver.editor/settings.toml` on macOS, next to everything else the app keeps, in `%APPDATA%\Editor\settings.toml` on Windows, with caches under `%LOCALAPPDATA%\Editor\Cache`, and in `~/.config/editor/settings.toml` on Linux, or under `XDG_CONFIG_HOME` if it is set. Caches and downloads go to `~/Library/Caches/io.github.sudo-oliver.editor`. Project settings stay in `.editor.toml`.

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
| `citations` | Citation and label index for LaTeX, Typst and Markdown, built off the main thread |
| `collab` | Live sessions and reviews: CRDT, roles and peer-to-peer transport |
| `addon_api` | The messages between the editor and an add-on |
| `git` | Status, staging, commits, branches and history through the `git` program |
| `pdf` | PDFium worker thread |
| `synctex` | SyncTeX parser and lookups |
