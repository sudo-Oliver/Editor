# Release checks

What to run before a release, and the numbers of the last run, so that a
slower build shows. Measured on an M-series MacBook, release builds.

## Run

```sh
cargo clippy --workspace --all-targets
cargo test --workspace
cargo test --workspace --release -- --ignored --nocapture   # needs LaTeX, Typst and the internet
scripts/bundle-macos.sh                                      # then the end-to-end steps below
```

## Numbers (2026-11-24, v0.1.0)

| What | Result |
|---|---|
| Idle CPU, a LaTeX project open | 0.0 % |
| Memory, a LaTeX project open | 157 MB |
| App binary (universal) / dmg | 37.7 MB / 38.6 MB |
| 10 MB LaTeX file, per key (background parse) | 5.5 ms, UI thread free |
| 1 MB Markdown preview, per key near the start / end | 1.0 ms / 0.45 ms |
| Collab: 4 editors, 1200 edits at once, all equal after | 86 ms |
| Collab: one edit to every guest | about 100 ms |
| Fresh install and first compile: Typst / Tectonic | 17 s / 2 min 15 s |
| Display math, 3 formulas with pdflatex | 0.4 s |
| Welcome project, first compile | 1.2 s |

## End to end, with the built dmg

1. Open the dmg: the window shows the app, the arrow, Applications and First open.txt, with no white band.
2. Copy the app and start it with an empty `EDITOR_HOME`: the setup assistant shows.
3. Finish the setup: the welcome project opens and compiles.
4. Put an unknown command in it and save: the error card explains it and offers the report link.
5. Report a problem: the details show version, macOS, tools and the build, with no user name.
6. Start a live session and join with `target/release/examples/guest <code>`: the guest's text, cursor, suggestion and comment arrive.
7. A Markdown file shows its preview, with typeset display math.
