# 0045 - Tags + favorites + collections

Goal: persist and edit per-book favorites, tags, and a single “collection” label from the library UI.

## Work

- [x] Add `favorite` to `Book` and persist it in sqlite (`crates/core`, `crates/storage`).
- [x] Add sqlite schema for `tags` + `book_tags` with a `kind` column (`crates/storage`).
- [x] Add `BookLabels` (tags + optional collection) and load/save it (`crates/core`, `crates/storage`, `crates/app`).
- [x] Add library UI actions:
  - [x] `f` toggle favorite
  - [x] `t` add/remove tag (by name)
  - [x] `c` set/clear collection (by name)
  (`crates/ui`)
- [x] Add focused storage unit tests (favorites + labels roundtrip) (`crates/storage`).

## Test plan

- [x] `cargo test --workspace --offline`
- [x] `cargo fmt --check`
