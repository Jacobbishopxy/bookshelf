# Bookshelf Roadmap (TUI PDF Manager/Reader)

Scope: terminal UI for managing and reading PDFs using `ratatui` + `pdf` crates.
Assumptions: crossterm backend for input/output, local library folder, no external services.

## Phase 0 – Foundation

- Add core deps: `ratatui`, `crossterm`, `pdf` (with render feature as needed), `serde` + `serde_json`, `directories` (config paths), `anyhow`/`thiserror`, `tracing` + `tracing-subscriber`, `rayon` (optional), `fuzzy-matcher` (search).
- App skeleton: modules for `app` (state/event loop), `ui` (layouts/widgets), `library` (index/filters), `reader` (PDF loading/navigation), `storage` (metadata persistence), `commands` (keybindings).
- Event loop: tick-based render + input handling, graceful startup/shutdown, panic-safe terminal restore.

## Phase 1 – Library & Metadata Features

- Library indexer: scan configured folders for PDFs, extract basic metadata (file name, size, page count if cheap), maintain `Book` records with ids.
- Search & filters: fuzzy search by title, filter by tags, recents (last opened), favorites, unread/finished; sorting (title, added_at, last_opened).
- Tags & metadata editing: modal to edit title/tags; lightweight validation.
- Bookmarks & notes: per-PDF list (page + note text); quick add/remove; show counts in list.
- Reading progress: track last page + percent; display progress bar; resume from last location.

## Phase 2 – UI/UX (ratatui)

- Layout: split view (library list on left, detail/preview/right pane); status bar with mode + hints; pop-up toasts for actions.
- Keybindings: central map for actions; on-demand cheat sheet modal; modes for library vs reader vs search.
- Dialogs/modals: metadata editor, tag selector, confirm delete, bookmark manager; focus handling + esc/cancel flows.
- Visual polish: highlight selection, progress indicators, icons/markers for bookmarked/favorite/new.

## Phase 3 – Reader & PDF Navigation

- Navigation: next/prev page, jump to page number, jump by percent, go to first/last; smooth handling of page bounds.
- Outline/TOC: parse PDF outlines; navigable tree; jump to selected destination.
- Bookmarks & notes UI: list for current book, jump to bookmark, inline add note.
- Thumbnails/preview: lightweight page preview (render via `pdf`/`pdf_render` to text/braille blocks or show first lines); cache previews per page.
- Robust load: lazy-open PDFs, handle encrypted/unsupported gracefully with error messages.

## Phase 4 – Persistence & Sync

- Storage backend: default to sqlite in config dir (`~/.config/bookshelf/bookshelf.db`); keep store traits to allow other backends or export/backup to JSON.
- Persist: library index (paths, metadata), progress, bookmarks, notes, tags, user settings, cached outlines.
- Sync-friendly: debounced writes, versioned schema, backup file, migration path for future sqlite.
- Library refresh: manual and periodic rescan; detect deleted/renamed files; reconcile state.

## Phase 5 – Performance

- Lazy load: open PDFs on demand; load outlines/page count once and cache.
- Prefetch: when reading, pre-render/cache next/prev page; background worker thread/channel.
- Large-file handling: cap cache size (LRU), stream pages; avoid loading full file; progress indicator during heavy parse.
- Startup speed: load state from disk, defer expensive rescans until after UI is up.

## Phase 6 – Quality, Tests, Tooling

- Tests: unit tests for indexer (scan/filter/sort), metadata store (persist/load/migrate), progress math; fake PDFs or fixtures for parsing outlines.
- Snapshot tests: ratatui layout snapshots for key screens; golden files per layout size.
- Integration flow: scripted user journeys (load library, search, open reader, add bookmark) using headless backend events.
- Observability: `tracing` spans for IO/render steps; optional `RUST_LOG` env flag; minimal metrics (counts/durations) printed in debug mode.
- Dev UX: `cargo fmt`/`clippy` hooks, `just`/`make` tasks for run/test, sample config with keybindings.

## Open Questions / Choices to confirm

- Rendering style: text-only preview vs braille/blocks; preview depth is a user setting; acceptable deps for rendering.
- Directory strategy: single library root vs multiple roots; watch for changes?
- Minimum terminal size/support for mouse input?
