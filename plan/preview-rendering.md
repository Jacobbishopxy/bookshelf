# Preview Rendering Investigation (text, braille/blocks, ratatui-image)

This note responds to the issues shown in:

- `tmp/bookshelf-debug-text.png`
- `tmp/bookshelf-debug-braille.png`
- `tmp/bookshelf-debug-blocks.png`

and answers:

1. How to make **text** mode more user friendly (paragraphs, readability).
2. How to render **braille/blocks** “as original as possible”.
3. Whether **ratatui-image + PDF → image** can improve things.

## Current implementation (what Bookshelf does today)

### Text mode

- Code path: `crates/engine/src/lib.rs`
  - Extraction: `ops_to_text()` and `render_page_text()` / `render_text_preview()`.
  - Data source: `pdf` crate content ops (`Op::TextDraw`, `Op::TextDrawAdjusted`, etc).
  - Line breaks:
    - `Op::TextNewline` → `\n`
    - `Op::MoveTextPosition { translation }` with `translation.y < 0.0` → `\n`
  - Everything else becomes a stream of “tokens” separated by a simple space heuristic.

This is intentionally simple, but it does **not** reconstruct layout (columns, indentation, paragraph spacing, hyphenation, headers/footers).

### Braille / Blocks mode (unicode art)

- Code path: `crates/engine/src/lib.rs`
  - Rasterize via Pdfium: `render_page_bitmap_gray()` renders to `PdfBitmapFormat::Gray`.
  - Convert pixels → chars in `render_page_raster_in_process()`:
    - `PreviewMode::Braille`: `braille_cell()` (2×4 pixels per char, 1-bit threshold)
    - `PreviewMode::Blocks`: `blocks_cell()` (2×4 pixels per char, 5 shades: `░▒▓█`)

This is “portable” (works everywhere), but it’s fundamentally low-fidelity:

- very low “dpi” in terminal character cells
- grayscale only (no color)
- no dithering / contrast tuning
- fixed thresholding for braille (`THRESHOLD: 200`)

## What the screenshots tell us (root causes)

### 1) Text mode is hard to read

Common PDF text extraction problems that match the screenshot:

- **No paragraph reconstruction**: PDF “text ops” are often positioned fragments, not semantic paragraphs.
- **Multi-column interleaving**: extraction that ignores x positions will mix left/right columns.
- **Hyphenation artifacts**: line-end hyphens should sometimes be removed and words re-joined.
- **Headers/footers**: repeated page furniture pollutes the reading flow.

### 2) Braille/blocks are not “human readable”

Two main reasons:

1. **Resolution is too low** for text shapes.
   - Current render width is basically “1 pixel per output dot”, so the page is downscaled extremely aggressively.
2. **The conversion is naive** (hard threshold / coarse shading), so anti-aliased text becomes noise.

Even with improvements, braille/blocks will never look like the original PDF. If the goal is *“as original as possible”*, you want **true terminal image protocols** (Kitty/iTerm2/Sixel) rather than character art.

## Q1 — Making text mode more user friendly

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

## Q2 — Rendering braille/blocks as original as possible

### Reality check: unicode art cannot be “original”

Braille/blocks are inherently approximations. The best “portable” you can do is:

- higher sampling resolution
- better tone mapping and dithering
- optional color (requires non-string output / styled cells)

If the goal is truly “original as possible”, use **terminal image protocols** via `ratatui-image`.

### A. Improve current braille/blocks (still character art)

1) **Render at higher pixel resolution (oversample)**

Instead of:

- `pixel_width = width_chars * cell_w`

render at:

- `pixel_width = width_chars * cell_w * SCALE` (e.g. SCALE=2..4)

Then downsample per dot/block using averaging. This alone typically makes text shapes recognizable.

1) **Adaptive thresholding for braille**

Replace fixed `THRESHOLD=200` with:

- global auto-threshold (e.g., Otsu)
- or per-cell adaptive threshold (local mean/variance)

This reduces “speckle noise” and recovers faint strokes.

1) **Dithering for blocks**

Instead of mapping avg darkness to 5 characters, use ordered dithering (Bayer matrix) to choose among a richer set of block characters.

1) **Optional color**

Blocks/halfblocks become dramatically more readable with truecolor:

- `▀` with per-cell fg/bg (two vertical pixels per cell)
- “pixel art in cells” style

This requires changing the engine output type away from `String` (e.g., return a `ratatui::Text` or a 2D buffer of styled cells), or doing the mapping in the UI layer.

At this point, `ratatui-image` already solves most of this better.

### B. Use ratatui-image (closest to “original”)

`ratatui-image` supports:

- Kitty graphics protocol
- iTerm2 protocol
- Sixel
- fallback “halfblocks” (unicode + fg/bg colors)
- optional chafa integration for very high quality text-mode image rendering

Key API points from the crate (source: `ratatui-image` v10.0.2):

- `picker::Picker::from_query_stdio()`:
  - queries terminal capabilities + cell size
  - **must be called after entering alt screen but before reading terminal events**
- `picker.new_resize_protocol(image::DynamicImage)` → `protocol::StatefulProtocol`
- render with `StatefulImage` (`Frame::render_stateful_widget`)
- for responsiveness, use `thread::ThreadProtocol` to offload resize+encode (and only render on UI thread)

#### PDF → image: how to feed ratatui-image

Bookshelf already rasterizes via Pdfium. For ratatui-image, prefer producing an RGBA `DynamicImage`:

1) Compute the target pixel size from render area and font size:

- `pixel_w = area.width * font_size.0`
- `pixel_h = area.height * font_size.1`

1) Render with Pdfium to an RGBA bitmap (or BGRA and swizzle).

2) Wrap into `image::DynamicImage` (from the `image` crate), then:

- `let proto = picker.new_resize_protocol(dyn_img);`
- `f.render_stateful_widget(StatefulImage::default(), rect, &mut proto);`

This gives near-original output on terminals with real image protocols.

#### Fallback behavior and feature flags

By default, `ratatui-image` enables `chafa-dyn` (see README / `protocol/halfblocks.rs`), which requires `libchafa` at runtime.

If you want to avoid that system dependency, use:

- `ratatui-image = { version = "...", default-features = false, features = ["crossterm", "image-defaults"] }`

and rely on its built-in primitive halfblocks fallback.

## Proposed approach for Bookshelf (pragmatic roadmap)

### Phase 1 — Text readability (no new UI widgets)

- Add a text post-processing pass:
  - de-hyphenate
  - paragraph joining (simple heuristics)
  - optional header/footer suppression
  - wrap to viewport width (reflow mode)
- Add a Reader toggle: `Raw` vs `Reflow`.

### Phase 2 — Reader image mode (ratatui-image)

- Keep **Preview** as `Text` / `Braille` / `Blocks` (portable and fast).
- Add a **Reader** render mode:
  - `ReaderMode::Image` using `ratatui-image` (protocol auto-detect; halfblocks fallback).
- This keeps image rendering in the Reader only, and avoids expensive inline-image work in the library preview.

### Phase 3 — True image protocols when available (best fidelity)

- At startup, call `Picker::from_query_stdio()` (after alt screen, before event loop).
- If protocol is Kitty/iTerm2/Sixel, allow:
  - `Image (Auto)` that uses the best available protocol
- Use `ThreadProtocol` or a dedicated render worker to keep the UI responsive.

### Phase 4 — Optional: keep braille/blocks, but fix them

If you still want braille/blocks as a “no color / purely textual” mode:

- oversample + downsample
- adaptive thresholds + dithering

## Open questions / decisions to make

1) Do we want separate `PreviewMode` (Preview panel) and `ReaderMode` (Reader panel), or a single shared mode?
2) Is `libchafa` an acceptable runtime dependency? If not, disable `ratatui-image` default features.
3) How should scrolling/zoom work for image pages?
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

### 1) Make text preview genuinely readable (universal)

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

### 2) Portable “pixel-ish” fallback using halfblocks (works in most terminals)

Even without Kitty/Sixel/iTerm2, we can render a page as:

- a grid of `▀`/`▄`/`█` (or halfblocks) with **truecolor fg/bg**

This is what `ratatui-image` uses as its **fallback protocol**.

It’s still limited by terminal cell resolution, but it’s typically **far more readable** than pure braille/blocks because:

- it uses color
- it uses sub-cell vertical resolution (fg+bg)
- it can do better scaling than our current 2×4 grayscale mapping

### 3) “Open in browser” preview (highest fidelity, terminal-independent)

If the real goal is *“as original as possible”* and it must work from any SSH terminal,
the most robust approach is: **render pages to images and view them outside the terminal**.

Two practical variants:

#### A) Export images to `tmp/` (simple)

- Keybinding: “Export current page preview”
- Writes `tmp/bookshelf-preview-<id>-p<page>.png`
- UI shows:
  - local path
  - suggested commands (`scp`, `rsync`, `sshfs`) for remote → local viewing

#### B) Embedded HTTP preview server + SSH port forwarding (best UX)

- Keybinding: “Preview in browser”
- App starts an HTTP server on `127.0.0.1:<port>` on the machine running Bookshelf.
- It serves:
  - a minimal HTML viewer with next/prev page controls and zoom
  - PNG/JPEG endpoints rendered on-demand (with caching)

Remote workflow:

- user runs `ssh -L 8181:127.0.0.1:8181 <remote>`
- open `http://127.0.0.1:8181` locally in a browser

This works even if the user’s terminal is extremely limited, and it gives real zoom/pan.
