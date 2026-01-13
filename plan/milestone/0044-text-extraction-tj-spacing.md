# 0044 - Text extraction: TJ spacing + no forced inter-token spaces

Goal: make Reader text extraction usable for PDFs that emit per-character glyph draws by removing the “space between every letter” artifact, while still restoring word spaces from `TJ` spacing adjustments when present.

## What shipped

- [x] Stop inserting implicit spaces between consecutive `Tj`/`TJ` text chunks (`crates/engine/src/lib.rs`).
- [x] Use `TJ` spacing adjustments to insert a word space for sufficiently large negative spacing values (`crates/engine/src/lib.rs`).
- [x] Add focused unit tests for both behaviors (`crates/engine/src/lib.rs`).

## Test plan

- [x] `cargo test --workspace --offline`
