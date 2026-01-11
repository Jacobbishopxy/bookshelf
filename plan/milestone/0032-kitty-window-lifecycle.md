# 0032 - Spawned Kitty reader lifecycle

Goal: if Bookshelf spawns a Kitty reader window from a non-Kitty terminal, closing the main app should also close that Kitty window.

## What changed

- [x] Track spawned `kitty` processes in the UI (`crates/ui/src/lib.rs`).
- [x] When the main UI exits with `UiExit::Quit`, kill/wait all tracked Kitty children so their windows close (`crates/ui/src/lib.rs`).
- [x] Adjust kitty spawn helpers to return `std::process::Child` so the UI can manage lifecycle (`crates/ui/src/kitty_spawn.rs`).

## Notes

- This is best-effort cleanup; it won’t run on hard kills like `kill -9` / terminal disconnect.

## Test plan

- [x] `cargo test -p ui --offline`
- [ ] Manual: run in a non-Kitty terminal → in Reader press `k` → spawned Kitty opens → quit main app → Kitty window closes.
