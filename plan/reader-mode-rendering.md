# Reader Mode Rendering (text extraction + image rendering)

Note: this file predates removing the Preview panel; Preview was removed in `plan/milestone/0038-remove-preview-feature.md`.

This note responds to the issues shown in:

- `tmp/bookshelf-debug-text.png`

and answers:

1. How to make **Reader text** mode more user friendly (paragraphs, readability).
2. How to get **high-fidelity Reader image** rendering when available.

## Current implementation (what Bookshelf does today)

### Reader text mode

- Code path: `crates/engine/src/lib.rs`
  - Extraction: `ops_to_text()` and `render_page_text()`.
  - Data source: `pdf` crate content ops (`Op::TextDraw`, `Op::TextDrawAdjusted`, etc).
  - Line breaks:
    - `Op::TextNewline` → `\n`
    - `Op::MoveTextPosition { translation }` with `translation.y < 0.0` → `\n`
  - Everything else becomes a stream of “tokens” separated by a simple space heuristic.

This is intentionally simple, but it does **not** reconstruct layout (columns, indentation, paragraph spacing, hyphenation, headers/footers).

### Reader image mode

- Code path: `crates/ui/src/lib.rs`
  - Rasterize via Pdfium to RGBA (engine): `Engine::render_page_bitmap_rgba()` (`crates/engine/src/lib.rs`)
  - Display as a real image in Kitty: `ratatui-image` using the Kitty graphics protocol

This gives near-original output (and avoids “character art” compromises), but it’s only available when the terminal supports it (Bookshelf currently gates image mode on Kitty).

## What the screenshots tell us (root causes)

### 1) Reader text mode is hard to read

Common PDF text extraction problems that match the screenshot:

- **No paragraph reconstruction**: PDF “text ops” are often positioned fragments, not semantic paragraphs.
- **Multi-column interleaving**: extraction that ignores x positions will mix left/right columns.
- **Hyphenation artifacts**: line-end hyphens should sometimes be removed and words re-joined.
- **Headers/footers**: repeated page furniture pollutes the reading flow.

## Q1 — Making Reader text mode more user friendly

There are two viable directions: **heuristic reflow** (fastest) or **layout-aware extraction** (best results).

### A. Quick wins (heuristics on extracted text)

Goal: improve readability without fully reconstructing coordinates.

1) **Paragraph reflow pass**

- Treat the extracted output as a sequence of lines.
- Join lines into paragraphs using simple heuristics:
  - keep blank lines as “hard” breaks
  - if a line ends without sentence punctuation and the next line starts with lowercase, join
  - if a line ends with `-` and next line begins with a letter, de-hyphenate and join

Then wrap paragraphs to viewport width.

1) **Noise cleanup**

- normalize whitespace
- remove repeated nulls (`\0`) and control characters (already partially done)
- optionally collapse duplicate spaces introduced by PDFs with odd spacing

1) **Header/footer suppression (best-effort)**

For the first N pages, detect lines that repeat verbatim at:

- the top K lines, and/or
- the bottom K lines

and drop them.

This helps a lot on academic/technical PDFs.

What this *won’t* solve: accurate multi-column reading order.

### B. Layout-aware text extraction (recommended for “reader-quality”)

Goal: preserve human reading order (especially for 2-column PDFs).

Options:

1) **Track text positioning from PDF ops**

- Maintain an approximate “cursor” `(x, y)` while iterating PDF text operators.
- When `Op::MoveTextPosition` changes x/y, use it to:
  - start new lines (y decreased)
  - detect indentation (x increased)
  - detect column breaks (large x jumps)
- Bucket fragments into line records keyed by y (with tolerance), then sort by x.
- Detect columns by clustering x positions.

This stays within the existing `pdf` crate approach, but it’s a real algorithm (more code, more edge cases).

1) **Prefer Pdfium text extraction when available**

Since Bookshelf already depends on Pdfium for raster rendering, consider using Pdfium’s text APIs (if exposed via `pdfium-render`, or via a small additional binding) to extract text *with bounding boxes*.

This is often more consistent and already solves some mapping problems.

### UI-level improvements (regardless of extraction backend)

- Add a “Text display” toggle in the Reader:
  - `Reflow`: wraps to viewport width with paragraph reconstruction
  - `Raw`: shows extracted lines exactly as extracted (debug/faithful mode)
- Add search highlight and “jump to next match” to help navigation even when extraction is imperfect.
- When extraction fails (`no text found`), offer a one-key switch to image preview (see below).

## Q2 — High-fidelity image rendering

`ratatui-image` supports:

- Kitty graphics protocol
- iTerm2 protocol
- Sixel
- fallback “halfblocks” (unicode + fg/bg colors) when no image protocol is available

Key API points from the crate (source: `ratatui-image` v10.0.2):

- `picker::Picker::from_query_stdio()`:
  - queries terminal capabilities + cell size
  - **must be called after entering alt screen but before reading terminal events**
- `picker.new_resize_protocol(image::DynamicImage)` → `protocol::StatefulProtocol`
- render with `StatefulImage` (`Frame::render_stateful_widget`)
- for responsiveness, use `thread::ThreadProtocol` to offload resize+encode (and only render on UI thread)

### PDF → image: how to feed ratatui-image

Bookshelf rasterizes via Pdfium. For ratatui-image, produce an RGBA `DynamicImage`:

1) Compute the target pixel size from render area and font size:

- `pixel_w = area.width * font_size.0`
- `pixel_h = area.height * font_size.1`

1) Render with Pdfium to an RGBA bitmap (or BGRA and swizzle).

2) Wrap into `image::DynamicImage` (from the `image` crate), then:

- `let proto = picker.new_resize_protocol(dyn_img);`
- `f.render_stateful_widget(StatefulImage::default(), rect, &mut proto);`

This gives near-original output on terminals with real image protocols.

### Fallback behavior and feature flags

By default, `ratatui-image` enables `chafa-dyn` (see README / `protocol/halfblocks.rs`), which requires `libchafa` at runtime.

If you want to avoid that system dependency, use:

- `ratatui-image = { version = "...", default-features = false, features = ["crossterm", "image-defaults"] }`

and rely on its built-in primitive halfblocks fallback.

## Proposed approach for Bookshelf (pragmatic roadmap)

### Phase 1 — Text readability (no new UI widgets)

- Tracked as milestone: `plan/milestone/0039-reader-text-readability.md`
- Add a text post-processing pass:
  - de-hyphenate
  - paragraph joining (simple heuristics)
  - optional header/footer suppression
  - wrap to viewport width (reflow mode)
- Add a Reader toggle: `Raw` vs `Reflow`.

### Phase 2 — Reader image mode

- Already implemented via `ratatui-image` in the Reader; focus future work on ergonomics/perf (zoom, pan, caching).

## Open questions / decisions to make

1) Is `libchafa` an acceptable runtime dependency? If not, disable `ratatui-image` default features.
2) How should scrolling/zoom work for image pages?
   - fit-to-width vs fit-to-height
   - zoom in/out with pan (arrow keys)

## SSH/Remote reality check (Kitty/Sixel/iTerm2)

Terminal “inline images” are **not** something you “install on the remote”.

- Kitty/Sixel/iTerm2 are **terminal emulator features** on your *local machine*.
- Over SSH, your app just writes escape sequences; your *local terminal* interprets them.

So:

- If you SSH **from Kitty** (or WezTerm, foot+sixel, iTerm2, etc), inline images can still work.
- If you SSH from a terminal that **does not** implement any image protocol (GNOME Terminal, Windows Terminal, VS Code integrated terminal, many others), then there is **no universal way** to show real raster images inside the terminal.

This means we need a layered strategy:

1) **Best-effort inline images** when available (nice, but optional).
2) A **portable fallback** that still looks “close enough”.
3) A way to get **true fidelity** without relying on the terminal at all.

## Alternative approaches that work over SSH (even without inline image support)

### 1) Make Reader text genuinely readable (universal)

If the user is in a “plain” terminal, **text is the only guaranteed medium**.

Recommended UI:

- `Text (Raw)` — current extracted output (debug/faithful)
- `Text (Reflow)` — paragraph reconstruction + wrapping

Engine work (see Q1 above):

- join wrapped lines into paragraphs
- de-hyphenate
- (optional) drop repeated headers/footers
- (optional) basic 2-column detection if we track x/y positions

This makes the default experience much better for everyone, regardless of terminal.

### 2) Portable “pixel-ish” fallback using halfblocks (optional)

Even without Kitty/Sixel/iTerm2, we can render a page as:

- a grid of `▀`/`▄`/`█` (or halfblocks) with **truecolor fg/bg**

This is what `ratatui-image` uses as its **fallback protocol**.

It’s still limited by terminal cell resolution, but it’s typically **far more readable** than older unicode-art approaches because:

- it uses color
- it uses sub-cell vertical resolution (fg+bg)
- it can do better scaling than naive per-cell thresholding

### 3) Spawn a local Kitty Reader window (highest fidelity)

When running in a non-Kitty terminal, spawning a dedicated Kitty reader window provides a “works everywhere” escape hatch without reintroducing Preview.
