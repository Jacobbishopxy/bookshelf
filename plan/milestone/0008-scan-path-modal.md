# 0008 - Scan path input modal (`o`)

- [x] Add scan-path modal state + renderer (`crates/ui`)
- [x] Add text input handling (type/backspace, Esc cancel)
- [x] Use `o` to open and `o` to apply+rescan (Enter also applies)
- [x] Apply to `Settings.library_roots` and persist on exit
- [x] Trigger rescan via existing `UiExit::Rescan` flow
- [x] Run `cargo test --workspace`
