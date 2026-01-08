# Project Structure (Bookshelf)

Workspace members (proposed)

- crates/core: domain types + traits (Book, Tag, Bookmark, Note, Progress, Settings), error types, pagination/search filters; ports like LibraryStore, StateStore, PdfBackend, Clock.
- crates/application: orchestration/services that implement use-cases (scan/rescan, search/filter/sort, open book, navigation, bookmarks/notes, progress tracking, settings update). Depends on core.
- crates/storage: sqlite-backed storage implementing LibraryStore/StateStore; migrations bundled; caches outlines/progress/bookmarks/notes/tags/settings. Depends on core.
- crates/engine: wrapper over `pdf` crate for metadata, outlines, page count, and preview generation; supports configurable preview depth (from Settings). Depends on core.
- crates/ui: ratatui components, layouts, keymaps, event loop glue; presents library + reader views, modals (search, metadata edit, tags, bookmarks/notes, go-to). Depends on application/core.
- crates/app (binary): wires deps (sqlite path, config dirs), spawns threads/channels, launches TUI; owns CLI args and logging setup. Depends on ui/application/storage/engine.
- crates/test (dev): helpers/fixtures for fake PDFs, temp DBs, snapshot harness.
- Notes: package names avoid std clashes (`crates/core` package = `bookshelf-core`; `crates/test` package = `bookshelf-test`).

Layering (allowed deps)
app -> ui -> application -> core
storage --> core
engine --> core
ui/application may depend on both storage and engine through traits; use feature flags or constructors to keep tests light.

Key responsibilities by layer

- core: pure data + invariants; Settings includes `preview_depth` (lines/blocks to render) and is user-editable.
- application: state machines for library and reader; background job scheduling (prefetch, rescans) via channels; cache abstractions; debounce writes to storage.
- storage: schema versioning, migrations, adapters between DB rows and domain types; batch writes for progress updates.
- engine: safe PDF open, outline parsing, page count, preview rendering with depth parameter; error mapping to domain errors.
- ui: input handling (crossterm), keybinding map, mode management (library/reader/search/modal), status bar with hints, popups for edits.
- app: config discovery (`directories` crate), CLI flags (library roots override, log level), tracing setup, panic-safe terminal restore.

Concurrency/event model

- Central event loop in ui consuming AppEvents (input, tick, background results).
- Background workers (threads) for: library scan/rescan, PDF metadata load, prefetch/preview, storage writes; communicate via mpsc channels.
- Storage access is serialized via worker; PDF parsing isolated to avoid blocking UI.

Config and settings

- Default storage: sqlite in config dir (e.g., `~/.config/bookshelf/bookshelf.db`).
- Settings file (TOML/JSON) in config dir; users can edit manually; includes preview_depth, library_roots, theme, keybindings, cache sizes.
- In-app settings modal allows editing preview_depth and other toggles; writes back via application layer.
- Preview UI: a dedicated panel lets users switch preview mode (text/braille/blocks) and adjust depth; engine uses these settings when generating previews.

Testing approach per crate

- core/application: unit tests for filtering/sorting/progress math, command handlers.
- storage: migration tests, CRUD roundtrips with temp DBs.
- engine: fixtures for outlines/metadata parsing; preview depth boundaries.
- ui: ratatui snapshot tests for main layouts; event-flow tests using fake backend.
- app: thin smoke test to ensure wiring compiles; avoid heavy runtime deps.
