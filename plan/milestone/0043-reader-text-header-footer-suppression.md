# 0043 - Reader text: trim repeated headers/footers

Goal: reduce “page furniture” noise (repeated headers/footers) in Reader text mode, without changing the raw extraction output.

## What shipped

- [x] Detect repeated header/footer lines across the first few pages and trim them at the page boundaries in `Wrap`/`Reflow` (`crates/engine`, `crates/ui`).
- [x] Add `Settings.reader_trim_headers_footers` and persist it in sqlite (`crates/core`, `crates/storage`).
- [x] Add Reader keybinding `h` to toggle trim on/off in text mode (`crates/ui`).
- [x] Add focused unit tests for trimming + settings roundtrip (`crates/engine`, `crates/storage`).

## Test plan

- [x] `cargo test --workspace --offline`
