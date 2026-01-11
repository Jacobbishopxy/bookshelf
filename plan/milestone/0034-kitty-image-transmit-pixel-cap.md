# 0034 - Kitty Reader: cap transmitted image pixels

Goal: reduce lag in Kitty image mode during pan/refresh by sending fewer pixels per frame.

## Why

In Kitty image mode the slow path is often building/sending the Kitty graphics payload (base64), which scales with the number of pixels transmitted.

## What changed

- [x] Downscale the viewport image before creating the Kitty protocol when it exceeds a pixel cap (`MAX_KITTY_TRANSMIT_PIXELS`) (`crates/ui/src/lib.rs`).
- [x] Construct the Kitty protocol directly with the full cell-area so the image still fills the Reader viewport even when the transmitted bitmap is smaller (`crates/ui/src/lib.rs`).
- [x] Extend `d dump` timing output with `downscale_ms` and `transmit_px` so we can tune the cap (`crates/ui/src/lib.rs`).

## Follow-ups

- [ ] Add a user-facing quality setting (e.g. `Fast / Balanced / Sharp`) that maps to a max transmitted megapixels (or scale), so users can trade sharpness for speed.

## Test plan

- [x] `cargo test -p ui --offline`
- [ ] Manual in Kitty: pan a page and confirm `protocol_ms` drops when `transmit_px` < `viewport_px`.
