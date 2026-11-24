# Suggestions for any LaTeX package, not only the ones in our list

## Context
Today the suggestions only know the hard-coded `LATEX_COMMANDS` (~150) and the palette. Niche packages get nothing, even when they are loaded. Hand-made lists do not scale, and bundling foreign completion lists (TeXstudio `.cwl`) is ruled out: GPL in an Apache project. Goal: suggestions for every loaded package, without new crates and without cost per keystroke.

Checked in this session:
- `kpsewhich natbib.sty siunitx.sty tikz.sty glossaries.sty` takes ~0.1 s for all 4 together.
- The `.sty` files contain the public commands as `\newcommand{\x}[n]`, `\DeclareRobustCommand`, `\NewDocumentCommand \x {o m m}`. natbib gives `citep citet citealp Citeauthor ...`, siunitx 26 xparse commands, glossaries hundreds.
- `build::find_tool` (`build/src/tools.rs:21`) also finds `/Library/TeX/texbin`, for apps started from Finder.

## Approach (two sources that complement each other)

1. **What the project already uses** (works at once, also with Tectonic or without a TeX installation):
   - `ProjectUsage.commands` (`build/src/packages.rs`) already holds every command used in the project, from the package check after each build.
   - `package_check::hints` also returns them.
   - They go to the views like `Loaded` (`EditorView::set_loaded_packages`, `workspace.rs show_package_hints`), and become candidates with detail `"used"` and template `\name`, no slots, since the arguments are unknown.

2. **Read the `.sty` of the loaded packages** — new `crates/app/src/package_commands.rs`:
   - **When:** in the background, in the existing `check_packages` task (`workspace.rs ~1473`), only for loaded packages that are not in the cache.
   - **Find:** one `kpsewhich a.sty b.sty ...` call through `find_tool("kpsewhich")` for all misses at once. No kpsewhich (Tectonic only): source 2 is skipped silently.
   - **Parse** (pure, unit tested):
     - `\newcommand`, `\renewcommand` not, `\providecommand`, `\DeclareRobustCommand`, each with `*`, `{\x}` or `\x`, optional `[n]` and `[default]`.
     - `\NewDocumentCommand`/`\DeclareDocumentCommand \x {spec}`.
     - `\newenvironment`/`\NewDocumentEnvironment` for `\begin{` suggestions.
     - Names with `@`, `_` or `:` (internal, expl3) go. `\def` is ignored: mostly internal.
     - Template from the arguments: `[n]` with a default gives n−1 slots, otherwise n slots `{${}}`. xparse: one slot per `m`/`+m`, `o`/`O{}`/`s` none.
   - **Following:** `\RequirePackage{x}` one level deep (natbib alone is enough, siunitx/glossaries load their parts), max 20 files per check.
   - **Cache:** `~/.cache/editor/packages.json` (XDG like `nvim_themes::cache_path`), per package: `.sty` path, mtime, commands, environments. A hit costs one `stat` per package. A TeX Live update (new mtime) rereads it.
   - **To the views:** `Arc<Vec<PackageCommand { name, template, package }>>`, candidates with the package name as detail, after `LATEX_COMMANDS` (so built-in templates win on doubles, the dedup in `completion::suggestions` keeps the first).

3. **Candidates** (`crates/app/src/completion.rs`): `candidates()` gets a `&Extra { package_commands, package_environments, used }` (borrowed, `&str` like `Candidate`), no copies per keystroke. Order: project symbols, `LATEX_COMMANDS`, palette, package commands, used commands. `Loaded::provides` stays for the built-in list.

## Performance
- **Per keystroke:** more candidates (glossaries alone has hundreds), still borrowed only. Scoring is linear. The ignored `suggestion_timing` gets a case with 1,500 extra commands. Target < 50 µs per key (today 19 µs).
- **Background:** a cold cache costs one kpsewhich call (~0.1 s) + reading the files. A warm one costs only `stat`. Never on the UI thread.

## Files
- new `crates/app/src/package_commands.rs` (+ `main.rs` mod)
- `crates/app/src/package_check.rs` (return the used commands)
- `crates/app/src/workspace.rs` (task, distribution)
- `crates/app/src/editor_view.rs` (field + setter)
- `crates/app/src/completion.rs` (`Extra`, timing)

## Verification
- Unit tests for the parser on extracts: natbib (`\DeclareRobustCommand\citep`, `\newcommand*{\citealp}`), siunitx (`\NewDocumentCommand \qty { O{} m m }` gives 2 slots), glossaries (`\newcommand{\gls}[2][]` gives 1 slot), `@` names are left out, environments.
- Cache test: second run without reading (mtime unchanged), new mtime rereads.
- Real check against TeX Live: ignored test that scans `natbib`, `siunitx`, `glossaries`, `tikz` through kpsewhich and prints the number of commands + time.
- `completion` test: used `\myniche` appears with detail "used", a package command with its package name.
- `cargo test -p app -p build`, `cargo clippy --all-targets`, `suggestion_timing` before/after.
- App manual: `\usepackage{glossaries}`, build, type `\gls`: suggestions from glossaries with detail `glossaries`.
