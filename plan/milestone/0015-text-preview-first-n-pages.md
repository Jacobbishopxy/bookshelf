# 0015 - Text preview from first N pages

- [x] Add `Settings.preview_pages` (editable in Preview Settings) (`crates/core`, `crates/ui`)
- [x] Persist `preview_pages` in sqlite settings (`crates/storage`)
- [x] Implement PDF text extraction for first N pages in `engine` (`crates/engine`)
- [x] Show `no text found` when extraction returns empty (`crates/engine`)
- [x] Run `cargo test --workspace`
