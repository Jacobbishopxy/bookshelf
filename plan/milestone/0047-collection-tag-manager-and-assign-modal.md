# 0047 - Collection/tag manager + assign modal

Goal: add a dedicated manager screen to create/rename/delete collections + tags, and an “Assign labels” modal in the library to apply existing collections/tags to books.

Constraints:

- Collections are exclusive (0–1 per book).
- Tags are a flat list (0–N per book).
- Assign modal “apply and close” on Enter.
- In assign modal, `n` creates a new tag/collection depending on focus.

## Work

- [x] Storage: add tag/collection catalog APIs (list/create/rename/delete) (`crates/storage`).
- [x] Application state: track known collections/tags and pending catalog ops (`crates/application`).
- [x] App wiring: load catalog from sqlite at boot + rescan; persist catalog ops on exit (`crates/app`).
- [x] UI: add “Manage labels” screen (`m`) with tabs for Collections/Tags and actions `n`/`r`/`d` (`crates/ui`).
- [x] UI: add “Assign labels” modal (`a`) with collection radio list + tag checkboxes, `Tab` focus, `/` filter, `n` create (`crates/ui`).
- [x] UI: update footer/help text for new keybindings (`crates/ui`).
- [x] Tests: add focused sqlite unit tests for catalog ops (`crates/storage`).

## Test plan

- [x] `cargo test --workspace --offline`
- [x] `cargo fmt --check`
