# 0035 - Reader image mode: fit-to-frame + center

Goal: in Kitty image mode, render the page centered and sized based on the `Page` frame (prefer using available height), avoiding “top-left only” presentation on wide terminals.

## What changed

- [x] Add `Engine::page_size_points()` to get a page aspect ratio without rendering (`crates/engine/src/lib.rs`).
- [x] In Reader image mode (zoom=100, no pan), compute a base render width that fits the page into the frame (height-first), then render the page and let `ratatui-image` fit it into the frame (`crates/ui/src/lib.rs`).
- [x] When a downscaled Kitty transmit is used (zoom/pan path), shrink the Kitty placeholder area to the transmitted image cell size so it centers inside the frame instead of sticking to the top-left (`crates/ui/src/lib.rs`).

## Test plan

- [ ] Manual in Kitty: open a portrait page in a wide terminal → image should be centered and use the full frame height (with side margins).
- [ ] Manual: zoom/pan path should still center the transmitted image area.
- [ ] `cargo test -p ui --offline`
- [ ] `cargo test -p engine --offline`
