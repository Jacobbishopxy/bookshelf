# 0003 - Enforce rlib-only output

- [x] Remove `crate-type = [\"dylib\"]` overrides so crates build as default `rlib` (`crates/*/Cargo.toml`)
- [x] Confirm `cargo test --workspace` succeeds when run outside the sandbox filesystem constraints (approval/escalated run)
- [x] Keep note: sandboxed `cargo test` may fail with `EXDEV` (cross-device link) during rustc archive/metadata writes
