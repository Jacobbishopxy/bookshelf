# 0040 - Reader text: wrap mode + non-text placeholders

Goal: keep extracted text structure closer to the source when desired, and handle image/chart-only pages more gracefully in Reader text mode.

## What shipped

- [x] Add `ReaderTextMode::Wrap` alongside `Raw`/`Reflow` (`crates/core/src/lib.rs`).
- [x] Persist `reader_text_mode` in settings (SQLite) (`crates/storage/src/lib.rs`).
- [x] Reader keybinding: `r` cycles `raw → wrap → reflow` (`crates/ui/src/lib.rs`).
- [x] `Wrap` pre-wraps each original line to viewport width (preserves blank lines), while avoiding wrapping “preformatted-looking” lines.
- [x] Show a placeholder block when a page has no extractable text (likely image/chart), with a hint to use image mode / kitty-reader (`crates/ui/src/lib.rs`).

## Test plan

- [x] `cargo test --workspace --offline`
