# 0001 - Workspace scaffold

- [x] Convert root into a Cargo workspace (`Cargo.toml`)
- [x] Add member crates: `crates/app`, `crates/core`, `crates/application`, `crates/storage`, `crates/engine`, `crates/ui`, `crates/test`
- [x] Rename packages to avoid std/reserved collisions (`bookshelf-core`, `bookshelf-test`)
- [x] Wire path dependencies between crates (`crates/*/Cargo.toml`)
- [x] Replace `cargo new` template code with minimal stubs in each crate
- [x] Remove old single-crate `src/` layout (now replaced by `crates/app`)
- [x] Update structure notes in `plan/structure.md`
- [x] Validate workspace build (at least `cargo check` and `cargo test --workspace`)
