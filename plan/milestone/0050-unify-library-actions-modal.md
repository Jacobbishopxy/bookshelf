# 0050 - Unify library actions modal

Goal: collapse multiple per-action library keybindings into a single tabbed actions modal, and shorten the main footer text.

Constraints:

- Replace multiple top-level action keys with a single entry point.
- Modal is tabbed so related actions stay grouped.
- Main footer stays short and readable.

## Work

- [x] Remove per-action main keys (fav/favorite/tags/collection)
- [x] Open tabbed modal from single key
- [x] Shorten main footer text

## Test plan

- [x] Run `cargo fmt` + UI tests
