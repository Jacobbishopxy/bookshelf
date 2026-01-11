# 0030 - Reader image mode: Kitty-only + “spawn kitty reader”

Goal: make Reader image mode reliably readable by using Kitty’s graphics protocol, and make it usable from terminals that *can’t* render graphics (e.g. MobaXterm) by spawning a dedicated Kitty reader window.

## Background

- `ratatui-image`’s Kitty protocol works great when the app runs *inside* Kitty.
- In “nested” environments (e.g. launched from tmux or from terminals that don’t support Kitty graphics), the Kitty escape sequences and unicode placeholders can leak into the UI as raw text (`_Gi=...`, lots of `/`), resulting in an unreadable/black screen.

## What shipped

- [x] Gate Reader image mode on “actually running inside Kitty” (`KITTY_WINDOW_ID`), not on ambiguous TERM/protocol hints (`crates/ui/src/image_protocol.rs`).
- [x] Only query terminal capabilities (`Picker::from_query_stdio`) when inside Kitty; otherwise use `Picker::halfblocks()` to avoid writing probe escape sequences to non-Kitty terminals (`crates/ui/src/lib.rs`).
- [x] Set an opaque `ratatui-image` background color to improve contrast and avoid “black on black” renders in image mode (`crates/ui/src/lib.rs`).
- [x] Provide a single escape hatch from non-Kitty terminals: `k` spawns a new Kitty window that opens directly into Reader on the same book/page (`crates/ui/src/kitty_spawn.rs`, `crates/ui/src/lib.rs`).
- [x] Bootstrap Reader in the spawned process via env vars (best-effort cleared after use):
  - `BOOKSHELF_BOOT_READER=1`
  - `BOOKSHELF_BOOT_READER_PATH=<encoded path>`
  - `BOOKSHELF_BOOT_READER_PAGE_INDEX=<0-based>`
  - `BOOKSHELF_BOOT_READER_MODE=image|text`
- [x] Fix “raw `_G...` sequences / black screen” when spawning Kitty from a tmux-like environment by sanitizing env for the child:
  - remove `TMUX`, `TERM_PROGRAM`, `TERM` before spawning Kitty (`crates/ui/src/kitty_spawn.rs`)
- [x] Make boot-reader sessions nicer: after leaving Reader with `Esc`, ignore the next `Esc` quit so the Kitty window doesn’t instantly close (`crates/ui/src/lib.rs`).
- [x] Improve debugging: Reader `d dump` includes key env vars and `ratatui-image` font size at the top of the dump (`crates/ui/src/lib.rs`).

## Current UX

- Reader mode toggle (`m`) only offers image mode when running inside Kitty.
- From any non-Kitty terminal, press `k` in Reader to open a dedicated “kitty-reader” window and continue reading there.

## Test plan

- [x] `cargo test -p ui --offline`
- [x] Manual: run in Kitty directly → image mode renders correctly.
- [x] Manual: run in non-Kitty terminal → press `k` → spawned Kitty opens directly in Reader image mode.
