# Analysis: `external/tdf` (itsjunetime/tdf)

This doc summarizes what the `tdf` project does, how it’s structured, and what it would mean to integrate it (or parts of it) into Bookshelf.

## What `tdf` is

- `tdf` is a **terminal PDF viewer** focused on responsiveness and rendering performance.
- It renders **PDF pages as images** (via MuPDF) and then displays those images in the terminal using terminal graphics protocols (Kitty, etc) via `ratatui-image` + `kittage`.
- It also supports **search**, **hot reload**, and **async rendering / conversion**.

Key point: it’s not primarily a “PDF text extraction” tool; it’s an **image renderer**.

## Repo / crate layout

From `external/tdf/Cargo.toml`:

- Package: `tdf-viewer` (edition 2024, rust 1.86)
- Binary: `tdf` (`src/main.rs`)
- Library: `tdf` exists “for benching”, but it **exports the project’s modules** and includes a global allocator.

Important modules (from `external/tdf/src`):

- `main.rs`: orchestration; input loop; spawns renderer + converter; manages terminal setup and file watching.
- `renderer.rs`: **MuPDF rendering thread** (sync) producing `PageInfo` (image bytes + highlight rects).
- `converter.rs`: converts rendered images into a terminal-display protocol (`ratatui-image` or Kitty images) producing `ConvertedPage`.
- `tui.rs`: ratatui UI that lays out and displays pages, handles zoom/pan/search UI state.
- `kitty.rs`: Kitty graphics protocol integration via `kittage` (including shared memory images).
- `skip.rs`: a `Skip` widget and an iterator (`InterleavedAroundWithMax`) used for render scheduling.
- `lib.rs`: exports modules + defines `scale_img_for_area`, and sets a global allocator.

## Rendering pipeline (data flow)

At a high level:

1. **Main (tokio) thread**
   - Sets up terminal (`EnterAlternateScreen`, raw mode, mouse capture).
   - Picks an image protocol using `ratatui_image::Picker` (queries terminal; falls back to font size derived from `crossterm::terminal::window_size()`).
   - Spawns:
     - A **std::thread** for rendering (MuPDF is `!Send`, so it can’t live across `.await`).
     - A **tokio task** for converting rendered pages into terminal-ready images.

2. **Renderer thread (`renderer::start_rendering`)**
   - Opens the document with `mupdf::Document`.
   - Responds to `RenderNotif` messages such as:
     - area changes (re-render everything to new size),
     - jumping to a page,
     - search term changes,
     - reload,
     - invert,
     - fit vs fill.
   - Emits `RenderInfo` messages, especially `RenderInfo::Page(PageInfo)`, where:
     - `PageInfo.img_data.pixels` contains a PNM-encoded image of the rendered page.
     - `PageInfo.result_rects` contains highlight rectangles for search results.

3. **Converter task (`converter::run_conversion_loop`)**
   - Receives `PageInfo`, decodes PNM via `image`, paints search highlights into pixels, then:
     - If Kitty protocol: builds `kittage::image::Image` (optionally via shared memory) and yields `ConvertedImage::Kitty`.
     - Otherwise: uses `ratatui_image::Picker::new_protocol()` to produce a `ratatui_image::protocol::Protocol` and yields `ConvertedImage::Generic`.

4. **UI (`tui.rs`)**
   - Maintains UI state and displays `ConvertedImage` pages.
   - Handles zoom/pan, layout, help/status messages, etc.

## Key dependencies and implications

From `external/tdf/Cargo.toml` (abridged):

- **PDF rendering**: `mupdf` (git dependency, pinned rev) + system dependencies (fontconfig, clang).
- **Terminal graphics**:
  - `ratatui-image` (git dependency, custom branch)
  - `kittage` (Kitty protocol; supports shm and async IO)
- **Async runtime / channels**: `tokio`, `flume`, `futures-util`
- **File watching**: `notify` + `debounce`
- **Performance**: global allocator `mimalloc`, plus custom forks of `ratatui` and `ratatui-image`.

Practical consequences for Bookshelf:

- Pulling `tdf` in as a library would introduce **tokio** and a significant dependency surface.
- `tdf` uses **git dependencies** for core UI libs; in a restricted-network environment, this requires vendoring or path overrides.
- `external/tdf/ratatui` and `external/tdf/ratatui-image` are present as submodule directories but are **empty here** (submodules not initialized), so the “use local path instead of git” escape hatch is not currently usable without initializing those submodules.

## License / legal compatibility

- `tdf-viewer` declares `license = "AGPL-3.0-only"` and includes a full AGPLv3 license text (`external/tdf/LICENSE`).
- It also depends on **MuPDF** via `mupdf-rs`. MuPDF itself is typically AGPL/commercial; `tdf` is consistent with that.

What this means:

- If Bookshelf **links** to `tdf` code (by adding it as a Rust dependency or copying code), then Bookshelf becomes a **derivative work** and will generally need to comply with **AGPL** distribution requirements.
- If Bookshelf instead **executes `tdf` as a separate program** (e.g. “Open in tdf”), that’s not linking; licensing impact is different (still be careful if distributing them together).

This is the biggest non-technical decision to make before integrating.

## How `tdf` maps to Bookshelf’s needs (preview + reading modes)

Bookshelf today:

- Text mode: PDF text extraction (via `pdf` crate).
- Braille/Blocks modes: currently placeholders (not image-based rendering).

`tdf` is relevant because it already solves the hard part of **rasterizing a PDF page** and displaying it in a terminal-friendly way.

However, `tdf` does not “extract text nicely”; it is fundamentally a **render-to-image** approach.

## Integration options (recommended order)

### Option A — “Open in tdf” (lowest coupling)

- Keep Bookshelf’s current engine.
- Add a keybinding that spawns `tdf <path>` as an external viewer.
- Pros: no linking; minimal changes; reuse a mature viewer.
- Cons: context switch; not “embedded preview” inside Bookshelf; assumes `tdf` is installed and in PATH.

### Option B — Use `tdf` as a library (highest coupling)

- Add `tdf` as a path dependency:
  - dependency key example: `tdf = { package = "tdf-viewer", path = "external/tdf" }`
  - note: its `[lib] name = "tdf"` exports modules.
- Reuse `renderer`/`converter` pipeline (or subset) to build Blocks/Braille previews.
- Pros: you get a working renderer pipeline with caching and protocol negotiation.
- Cons:
  - **AGPL** implications,
  - heavy dependencies (tokio, mupdf, forks),
  - global allocator side effect (`mimalloc`),
  - architectural mismatch (tdf is a full-screen viewer with its own event loop).

### Option C — Reimplement a small subset (inspired by tdf, but not copying code)

- Add your own `engine` backend that:
  - rasterizes a PDF page (choose a renderer: MuPDF, PDFium, Poppler, etc),
  - converts the image to braille/blocks using a maintained crate (possibly `ratatui-image` from crates.io).
- Pros:
  - You control the API and can keep it minimal (only what Bookshelf needs).
  - Easier to integrate into the existing engine abstraction.
- Cons:
  - still need to pick and integrate a rasterizer, and its license may be restrictive (MuPDF).

### Option D — Alternative renderer (avoid AGPL where possible)

- If AGPL is not acceptable, consider PDFium-based rendering crates (licensing still needs verification) or other rasterizers.

## Concrete next steps (decision checklist)

1. Decide whether **AGPL code is acceptable** for Bookshelf.
2. Decide whether you want:
   - Embedded previews (within Bookshelf) → needs a rasterizer + `ratatui-image`-style conversion, or
   - External viewer integration (“Open in tdf”) → simplest.
3. If embedded previews are desired:
   - pick the rasterizer backend and confirm license,
   - define a small, stable Bookshelf-side API (e.g. `render_page(mode, page, area) -> Widget/Text`),
   - add caching and background rendering later.
