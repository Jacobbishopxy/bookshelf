# 0033 - Kitty reader: `Esc` quits the session

Goal: in the dedicated Kitty reader window (boot-reader session), pressing `Esc` should close the window by exiting the app instead of returning to the main library UI.

## What changed

- [x] In boot-reader sessions, Reader `Esc` returns `UiExit::Quit` immediately after saving progress (`crates/ui/src/lib.rs`).

## Test plan

- [ ] Manual: from a non-Kitty terminal → Reader `k` → in spawned Kitty reader press `Esc` → Kitty window closes.
- [ ] `cargo test -p ui --offline`
