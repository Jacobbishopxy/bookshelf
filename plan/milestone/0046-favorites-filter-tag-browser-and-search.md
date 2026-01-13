# 0046 - Favorites filter + tag/collection browser + search filters

Goal: make favorites/tags/collections usable as first-class filters and browsing tools in the library UI.

## Work

- [x] Add “favorites only” filter toggle in the library view (`crates/ui`, `crates/application`).
  - [x] Choose keybinding + status indicator (e.g., footer chip).
  - [x] Filter impacts library list and selection behavior (keep selection stable where possible).
- [x] Add a new “Browse labels” panel for tags + collections (`crates/ui`).
  - [x] Show a list of collections and tags with counts (books in each).
  - [x] Selecting a label filters the library list (quick browse flow).
  - [x] Provide “clear filter” and “back” behavior (Esc).
- [x] Enhance search panel with structured label filters (`crates/ui`).
  - [x] Show all tags and collections with counts.
  - [x] Support multi-select tags with `AND` / `OR` match mode.
  - [x] Support selecting exactly one collection (or “any” / “none”).
  - [x] Combine with existing title/path query input.
  - [x] Provide clear UX affordances: selected chips, help text, keyboard-only navigation.

## Test plan

- [x] `cargo test --workspace --offline`
- [x] `cargo fmt --check`
