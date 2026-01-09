# Storage Enhancement Plan (for Reader Mode + future tags/categories)

## What should be persisted (sqlite)

**Always persist (durable across runs):**

- **User settings**: preview mode/depth/pages, scan roots + scan scope, and future UI preferences.
- **Library catalog** (book records): stable `book_id`, `path`, `title`, timestamps; later add file metadata (`mtime`, `size`, `page_count`, optional `fingerprint`).
- **Reading state**: per-book `last_page`, `updated_at`, optional `total_pages`.
- **User metadata** (future): tags, categories/collections, favorites, bookmarks, notes.

**Never persist (runtime-only caches):**

- Rendered page buffers / braille-block output.
- Extracted text cache / parsed page ops.
- Open PDF handles, LRU caches, background worker queues.

Rule of thumb: **persist user intent**, not **derived/expensive-to-recompute views**.

## “Persist in memory and disk” (how we’ll model it)

We treat sqlite as the durable store and keep an **in-memory state cache** as the UI’s working set.
Writes are **write-through**: update memory first (for immediate UX), then write to sqlite in the same call/transaction.

### Proposed layering

- `bookshelf_storage` (durable + cached):
  - `trait StorageBackend` (sqlite implementation; later can add others)
  - `struct Store<B: StorageBackend>`: owns `B` + an in-memory `CachedState`
  - `CachedState`: `settings`, `books`, `progress`, `tags`, etc (authoritative for UI)
- `bookshelf_application`:
  - high-level operations (scan reconciliation, open book, update progress, edit tags)
  - calls into `Store` methods; does not do SQL
- `bookshelf_engine`:
  - runtime-only caches (per book/page) for rendered/extracted content

## Schema direction (sqlite)

Enable foreign keys (`PRAGMA foreign_keys=ON`) and use `ON DELETE CASCADE` to keep data consistent.

Minimum set for reader mode + future growth:

- `settings` (singleton row): already exists; keep versioned migrations
- `books`:
  - `id INTEGER PRIMARY KEY`
  - `path TEXT UNIQUE NOT NULL`
  - `title TEXT NOT NULL`
  - later: `mtime INTEGER`, `size INTEGER`, `page_count INTEGER`, `fingerprint TEXT`
- `book_progress`:
  - `book_id INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE`
  - `last_page INTEGER NOT NULL`
  - `updated_at INTEGER NOT NULL DEFAULT (unixepoch())`
- Future tables:
  - `tags(id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, kind TEXT NOT NULL DEFAULT 'tag')`
  - `book_tags(book_id INTEGER REFERENCES books(id) ON DELETE CASCADE, tag_id INTEGER REFERENCES tags(id) ON DELETE CASCADE, PRIMARY KEY(book_id, tag_id))`
  - `bookmarks(id INTEGER PRIMARY KEY, book_id INTEGER REFERENCES books(id) ON DELETE CASCADE, page INTEGER NOT NULL, label TEXT, created_at INTEGER NOT NULL DEFAULT (unixepoch()))`
  - `notes(id INTEGER PRIMARY KEY, book_id INTEGER REFERENCES books(id) ON DELETE CASCADE, page INTEGER, body TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT (unixepoch()))`

## API direction (what app code calls)

`Store` should expose intent-level methods, not SQL-shaped methods:

- `load_state()` / `refresh_state_from_disk()` (startup + debugging)
- `update_settings(Settings)`
- `reconcile_scan(found_books: Vec<Book>)` (upsert + delete missing, preserve ids)
- `get_books()` / `get_book(id)` (from cache)
- `set_progress(book_id, last_page)` (write-through)
- later: `add_tag(name)`, `toggle_tag(book_id, tag_id)`, `list_tags()`, etc

All writes should be transactional inside the backend, and update the cache only if the write succeeds (or revert on failure).

## Migration/testing strategy

- Migrations stay in `bookshelf_storage` and are idempotent.
- Use sqlite `open_in_memory()` for tests (already works) to validate:
  - schema migration
  - roundtrips for settings/progress/tags
  - cascade deletes (removing a book removes its progress/tags/bookmarks)

## Execution order (when we start implementing)

1. Add `book_progress` table + backend methods.
2. Introduce `Store` cache wrapper and move app to use it (minimal refactor).
3. Wire reader mode to update `book_progress`.
4. Add tags/categories tables + methods (after reader basics are stable).
