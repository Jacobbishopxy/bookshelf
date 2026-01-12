# 0038 - Remove Preview feature

Goal: remove all “Preview” functionality (settings + UI + engine rendering) and its persistence wiring.

## What changed

- [x] Removed preview fields/types from `Settings` (`crates/core/src/lib.rs`).
- [x] Removed preview rendering pipeline from the engine (`crates/engine/src/lib.rs`).
- [x] Removed preview persistence columns/queries; settings row insert is now `id`-only (`crates/storage/src/lib.rs`).
- [x] Removed preview UI (preview panel + details preview section) (`crates/ui/src/lib.rs`).
- [x] Removed `--pdfium-worker` path (preview worker) from the app (`crates/app/src/main.rs`).

## Test plan

- [x] `cargo test --workspace --offline`
