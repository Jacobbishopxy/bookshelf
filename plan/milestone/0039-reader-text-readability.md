# 0039 - Reader text mode readability

Goal: make Reader text mode pleasant to read by default (paragraphs + wrapping), without requiring image protocols.

Related note: `plan/reader-mode-rendering.md` (Phase 1).

## Scope

- Reader **text** mode only (no changes to image mode).
- Heuristic post-processing on extracted text (no layout-aware PDF reconstruction yet).

## Work items

- [x] Add an engine text post-processing pass (pure function) that:
  - [x] normalizes whitespace + control chars
  - [x] joins lines into paragraphs with simple heuristics
  - [x] de-hyphenates common line-break hyphenation (`foo-\nbar` → `foobar`)
  - [x] preserves explicit blank lines as hard paragraph breaks
- [x] Add a Reader toggle for text display:
  - [x] `Raw` (current extracted output)
  - [x] `Reflow` (post-processed + wrapped)
- [x] Render `Reflow` to the viewport width (word wrapping) and keep scroll behavior sensible.
- [x] Persist the chosen text display mode in `Settings` (`crates/core`) so it survives restarts.
- [x] Add focused unit tests for the reflow pass (hyphenation + paragraph joining + whitespace).
- [x] Run `cargo test --workspace --offline`

## Acceptance criteria

- `Reflow` produces fewer mid-sentence line breaks and fewer hyphenation artifacts on typical PDFs.
- Switching `Raw`/`Reflow` is one keypress and does not affect Reader image mode.
