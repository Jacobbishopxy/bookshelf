# 0004 - Settings persistence + preview UI (sqlite)

- [x] Add dev note for bypassing machine-global Cargo config (later removed in `plan/milestone/0005-cleanup-cargo-home.md`)
- [x] Centralize external dependency versions in workspace `Cargo.toml`
- [x] Add dependencies to member crates via `workspace.dependencies`
- [x] Add `PreviewMode` parse/format helpers for storage (`crates/core/src/lib.rs`)
- [x] Implement sqlite settings store in `crates/storage/src/lib.rs`
- [x] Implement interactive TUI panel for preview mode/depth in `crates/ui/src/lib.rs`
- [x] Wire app to load settings from sqlite and save on exit (`crates/app/src/main.rs`)
- [x] Run `cargo test --workspace` (requires network + non-sandbox run in this environment)
