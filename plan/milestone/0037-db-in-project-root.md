# 0037 - Store SQLite DB in project root

Goal: use a per-project SQLite database under the repo/project directory instead of the OS config directory.

## What changed

- [x] Default DB path is now `./.bookshelf/bookshelf.db` (relative to the app’s current working directory) (`crates/app/src/main.rs`).
- [x] Ignore local DB artifacts in git (`.gitignore`).

## Test plan

- [x] `cargo run` from the project root and confirm `./.bookshelf/bookshelf.db` is created.
