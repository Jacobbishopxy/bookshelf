# 0031 - Reader (Kitty) image mode: smoothness/performance

Goal: make Reader image mode feel responsive in Kitty (faster page turns and less lag on pan/zoom), without regressing reliability.

## Symptoms

- Page turns and/or first paint can feel slow in Kitty image mode.
- Pan/zoom currently rebuilds the image protocol payload, which can be expensive.

## Plan

- [x] Add lightweight timing instrumentation around the render pipeline (PDF rasterize → viewport crop → protocol encode) and surface it via Reader `d dump` (and optionally a debug-only footer line).
  - Target files: `crates/ui/src/lib.rs`
- [x] Reduce unnecessary work in the main loop:
  - [x] Add a `dirty`/`needs_redraw` flag so we don’t redraw at the tick rate when nothing changed (especially important for Kitty image payloads).
  - [x] Ensure `ReaderPanel::ensure_rendered` is only called when needed (mode/page/size/pan/zoom changed).
  - Target files: `crates/ui/src/lib.rs`
- [x] Improve perceived latency on page turns:
  - [x] Add a small in-memory cache (LRU) of rendered page bitmaps (e.g. prev/current/next) keyed by `(page, zoom, render_width_px, font_size)` to avoid re-rasterizing when paging back/forth.
  - [ ] Optionally pre-render next/prev pages after the current page finishes rendering.
  - Target files: `crates/ui/src/lib.rs` (or extract to a small module under `crates/ui/src/`)
- [ ] Put guardrails on worst-case render cost:
  - [ ] Cap render resolution (max width px / max megapixels) so a very wide terminal doesn’t trigger huge PDF rasterization work; add a setting later if needed.
  - Target files: `crates/ui/src/lib.rs`
- [ ] (Optional) Investigate whether Kitty “image id + placement” can reduce repeated full-image transfers for pan/refresh; only pursue if instrumentation shows protocol transfer dominates.

## Test plan

- [x] `cargo test -p ui --offline`
- [ ] Manual in Kitty: measure time-to-first-image and page turn latency before/after (use timings from `d dump`).
