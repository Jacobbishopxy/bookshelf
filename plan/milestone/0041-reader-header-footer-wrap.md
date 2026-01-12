# 0041 - Reader header/footer wrap for long strings

Goal: when Reader header/footer strings are too long for the terminal width, wrap to the next line instead of truncating.

## What shipped

- [x] Reader footer reserves 2 lines (+ border) and wraps key-hint text when needed (`crates/ui/src/lib.rs`).
- [x] Reader header wraps long title text within its fixed height (`crates/ui/src/lib.rs`).

## Test plan

- [x] `cargo test --workspace --offline`
