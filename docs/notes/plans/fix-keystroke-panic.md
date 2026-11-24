# Fix: panic `InvalidKeystrokeError { keystroke: "secondary-a" }`

## Context
gpui bump c427327 -> 5709f32 (commit 04fd953) changed `KeyBinding::new`: now parses with
`Keystroke::parse` (no OS), so `secondary-` modifier errors and `.unwrap()` panics at startup.
Only `KeyBinding::load(.., cx)` resolves `secondary-` via `cx.platform_info().os`
(gpui `keymap/binding.rs:30-60`, `platform/keystroke.rs:158`).
Old gpui fix (block crate) was fine; this is a new API break from the same bump.

## Change (one file: `crates/app/src/actions.rs`)
1. Add helper next to `bind_keys`:
   ```rust
   /// `KeyBinding::new` no longer resolves `secondary-`, only `load` with the host OS does.
   fn binding<A: Action>(keys: &str, action: A, context: Option<&str>, cx: &App) -> KeyBinding {
       let context = context.map(|c| Rc::new(KeyBindingContextPredicate::parse(c).unwrap()));
       KeyBinding::load(keys, Box::new(action), context, false, None, cx).unwrap()
   }
   ```
   Imports: `gpui::{Action, KeyBindingContextPredicate}`, `std::rc::Rc`.
2. Replace every `KeyBinding::new("secondary-...", X, ctx)` (lines ~127-176) with
   `binding("secondary-...", X, ctx, cx)`. Build that group into a `Vec` first, then
   `cx.bind_keys(vec)` to avoid borrow clash. Bindings without `secondary-` stay `KeyBinding::new`.

No new crates.

## Verify
- `cargo build --release -p app`
- `cargo run --release -p app -- examples/overleaf-sample`: starts, no panic.
- Test Cmd-A, Cmd-S, Cmd-Z, Cmd-1 in app.
