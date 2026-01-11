# 0036 - Reader keys: simplify image pan

Goal: reduce key clutter in Reader mode by removing `h/j/k/l` panning and using arrows consistently.

## What changed

- [x] Remove `h/j/k/l` pan bindings in Reader image mode (`crates/ui/src/lib.rs`).
- [x] Add `Shift+←/→` to pan horizontally in image mode (regular `←/→` remains page prev/next) (`crates/ui/src/lib.rs`).
- [x] Update footer hints to reflect the new bindings (`crates/ui/src/lib.rs`).

## Current keymap (Reader)

- `←/→`: page prev/next
- `↑/↓`: scroll (text) / pan vertical (image)
- `Shift+←/→`: pan horizontal (image)

## Test plan

- [ ] Manual: in image mode, `Shift+←/→` pans left/right; `←/→` still turns pages.
- [ ] `cargo test -p ui --offline`
