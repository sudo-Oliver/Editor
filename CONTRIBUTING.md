# Contributing to Editor

Thank you for helping. Bug reports, ideas and pull requests are all welcome.

## Before you start

- **Bugs:** Help, Report a problem in the app opens a prefilled GitHub issue with the app version, the system and the last build log. You only describe what happened.
- **Larger changes:** open an issue first, so that we can agree on the approach before you spend time on it.
- **One change per pull request.** New features must also work in Vim mode and must not take over existing key bindings.

## Terms for contributions

Editor is source-available under the [PolyForm Strict License 1.0.0](LICENSE), not open source. That license does not allow changes or new works on its own. For contributions, the author grants this extra permission:

- You may clone this repository and change it **only to prepare a contribution** to this repository, and to build and test that change on your own computer.
- You may not publish, distribute or sell a changed version, or use the code for your own product.

By opening a pull request, you agree that:

1. the contribution is your own work, or you have the right to submit it,
2. you grant the author of Editor (sudo-Oliver) a perpetual, worldwide, irrevocable, royalty-free license to use, change, relicense and distribute your contribution as part of Editor or otherwise,
3. you will be named as a contributor in the git history.

## Rules for the repository

- **No secrets, ever.** No API keys, tokens, passwords, certificates, private keys or `.env` files, not even in a test, a comment or a commit that a later commit removes. Signing and release details live only in the GitHub repository secrets. Test data uses obvious fakes such as `sk-ant-abc`.
- **No personal data.** Use your GitHub noreply address for commits (`git config user.email <id>+<name>@users.noreply.github.com`). No real names, email addresses or paths from your own computer in code, tests or screenshots.
- **Tests stay green.** Run the checks below before you open a pull request.
- **AI tools are fine.** This project is built with heavy use of AI. You are responsible for every line you submit, so read and test what an assistant wrote.

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

Building on Linux also needs the system libraries that GPUI requires (X11, Wayland, Vulkan, fontconfig). The Linux build has not been run yet.

Add-ons are in development and not part of the releases. `cargo run -p app --features addons` shows the Add-ons panel. The protocol is in the crate `addon_api`, and add-ons live in their own repositories.

Working notes, the rules of the project and the plans behind it are in [docs/notes](docs/notes/README.md).

## Checks

```sh
cargo clippy --workspace --all-targets
cargo test --workspace
cargo test --workspace --release -- --ignored --nocapture   # needs LaTeX, Typst and the internet
```

[docs/release-checks.md](docs/release-checks.md) lists the end-to-end steps with a built app, and [docs/windows-test.md](docs/windows-test.md) the steps on a Windows PC.

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

## Building a release

Pushing a tag such as `v0.1.0` runs `.github/workflows/release.yml`. It builds the universal dmg on macOS and the Windows installer with Inno Setup, and attaches both, with the Windows checksum and the Homebrew cask, to a draft release.

The version comes from `Cargo.toml` and must be higher than the last release. Test the downloads of the draft, with [docs/windows-test.md](docs/windows-test.md) on a PC, then publish it: installed apps only see published releases. After that, copy the attached `editor.rb` into the tap repository (`sudo-Oliver/homebrew-tap`, `Casks/editor.rb`).

Without signing details the Mac app is signed ad hoc. With an Apple Developer ID, the workflow reads the certificate and notarization details from repository secrets. The header of `scripts/bundle-macos.sh` lists them.

Locally, `scripts/bundle-macos.sh` builds `target/bundle/Editor.app` for both architectures and the dmg. For a Windows installer of a branch, run the Windows preview workflow by hand, or label a pull request `windows-preview`.
