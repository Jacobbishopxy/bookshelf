# 0029 - Reader image zoom + pan

- [x] Cache full-page RGBA render for `ReaderMode::Image` (`crates/ui/src/lib.rs`)
- [x] Render viewport crop (no fit-to-height downscaling) to reduce blur (`crates/ui/src/lib.rs`)
- [x] Add keybindings: `+/-` zoom, `0` reset, `↑/↓` pan, `h/j/k/l` pan, `PgUp/PgDn` page-pan (`crates/ui/src/lib.rs`)
- [x] Update Reader header/footer hints to show zoom + pan controls (`crates/ui/src/lib.rs`)
- [x] Run `cargo test --workspace --offline`
