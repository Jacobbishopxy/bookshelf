# 0042 - Reader: move page/mode status into Page title

Goal: keep the Reader header focused on the document title, and move page/mode status (`p13/542`, text mode, image details) into the Page block title.

## What shipped

- [x] Reader header shows only `Reader — <title>` and wraps when needed (`crates/ui/src/lib.rs`).
- [x] Page block title shows `p<cur>/<total> · <mode>` for both text and image modes (`crates/ui/src/lib.rs`).

## Test plan

- [x] `cargo test --workspace --offline`
