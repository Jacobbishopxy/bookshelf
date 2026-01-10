# 0028 - Reader image mode (ratatui-image) + separate Preview/Reader modes

- [x] Add `ratatui-image` dependency (no `libchafa` requirement) (`Cargo.toml`, `crates/ui/Cargo.toml`)
- [x] Align `ratatui`/`crossterm` versions for `ratatui-image` (`Cargo.toml`, `Cargo.lock`)
- [x] Add Pdfium RGBA bitmap render helper for page images (`crates/engine/src/lib.rs`)
- [x] Add ratatui-image `Picker` init + image widget rendering in Reader (`crates/ui/src/lib.rs`)
- [x] Split settings: `PreviewMode` stays `text/braille/blocks`, new `ReaderMode` is `text/image` (`crates/core/src/lib.rs`)
- [x] Persist `reader_mode` in sqlite and migrate legacy `preview_mode='image'` (`crates/storage/src/lib.rs`)
- [x] Keep Preview limited to text/character-art; use `ReaderMode` for Reader rendering (`crates/engine/src/lib.rs`, `crates/ui/src/lib.rs`)
- [x] Update preview rendering investigation note (`plan/preview-rendering.md`)
- [x] Run `cargo test --workspace --offline`
