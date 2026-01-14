# 0048 Tabbed label operations overlay

Goal: consolidate label filtering, assignment, and catalog management into a single `/` overlay with ratatui `Tabs`, replacing the old `a`/`m` label screens.

- [x] UI: replace label browse/assign/manage screens with `/` overlay tabs (Search/Assign/Manage) (`crates/ui`).
- [x] UI: keep `l` as a shortcut that opens `/` focused on the Collections filter (`crates/ui`).
- [x] UI: remove library keybinds `a`/`m` for labels (`crates/ui`).
- [x] UI: integrate favorite toggle into Assign tab and apply it on Enter (`crates/ui`).
- [x] UI: clear terminal after stdio image protocol probe to avoid stray startup output (`crates/ui`).
- [x] UI: add spacing line between “Last opened” and “Favorite” in Details panel (`crates/ui`).
- [x] Verify with `cargo fmt` and `cargo test --workspace`.
