//! Sqlite-backed persistence.

use std::path::Path;

use anyhow::Context as _;
use bookshelf_core::{
    Book, BookLabels, Bookmark, KittyImageQuality, Note, ReaderMode, ReaderTextMode, ScanScope,
    Settings, TagKind,
};
use rusqlite::{Connection, OptionalExtension as _};

#[derive(Debug)]
pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let conn = Connection::open(path.as_ref())
            .with_context(|| format!("open sqlite db at {}", path.as_ref().display()))?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        self.conn
            .execute_batch("PRAGMA foreign_keys=ON;")
            .context("enable sqlite foreign keys")?;

        // Settings table: per-project settings only (preview has been removed).
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                reader_mode TEXT NOT NULL DEFAULT 'text',
                reader_text_mode TEXT NOT NULL DEFAULT 'reflow',
                reader_trim_headers_footers INTEGER NOT NULL DEFAULT 1,
                kitty_image_quality TEXT NOT NULL DEFAULT 'balanced',
                scan_scope TEXT NOT NULL DEFAULT 'recursive',
                library_roots_json TEXT NOT NULL DEFAULT '[]'
            );
            "#,
        )?;

        self.conn
            .execute("INSERT OR IGNORE INTO settings (id) VALUES (1)", [])
            .context("insert default settings row")?;

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS books (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                added_at INTEGER NOT NULL DEFAULT (unixepoch()),
                last_opened INTEGER,
                favorite INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS book_progress (
                path TEXT PRIMARY KEY REFERENCES books(path) ON DELETE CASCADE,
                last_page INTEGER NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            );

            CREATE TABLE IF NOT EXISTS bookmarks (
                path TEXT NOT NULL REFERENCES books(path) ON DELETE CASCADE,
                page INTEGER NOT NULL,
                label TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (path, page, label)
            );

            CREATE TABLE IF NOT EXISTS notes (
                path TEXT NOT NULL REFERENCES books(path) ON DELETE CASCADE,
                page INTEGER NOT NULL,
                body TEXT NOT NULL,
                PRIMARY KEY (path, page, body)
            );

            CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                UNIQUE (name, kind)
            );

            CREATE TABLE IF NOT EXISTS book_tags (
                path TEXT NOT NULL REFERENCES books(path) ON DELETE CASCADE,
                tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                PRIMARY KEY (path, tag_id)
            );
            "#,
        )?;

        match self.conn.execute(
            "ALTER TABLE settings ADD COLUMN scan_scope TEXT NOT NULL DEFAULT 'recursive'",
            [],
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add settings.scan_scope column");
                }
            }
        }

        self.conn.execute(
            "UPDATE settings SET scan_scope = 'recursive' WHERE scan_scope IS NULL",
            [],
        )?;

        match self.conn.execute(
            "ALTER TABLE settings ADD COLUMN reader_mode TEXT NOT NULL DEFAULT 'text'",
            [],
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add settings.reader_mode column");
                }
            }
        }

        match self.conn.execute(
            "ALTER TABLE settings ADD COLUMN reader_text_mode TEXT NOT NULL DEFAULT 'reflow'",
            [],
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add settings.reader_text_mode column");
                }
            }
        }

        match self.conn.execute(
            "ALTER TABLE settings ADD COLUMN reader_trim_headers_footers INTEGER NOT NULL DEFAULT 1",
            [],
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add settings.reader_trim_headers_footers column");
                }
            }
        }

        match self.conn.execute(
            "ALTER TABLE settings ADD COLUMN kitty_image_quality TEXT NOT NULL DEFAULT 'balanced'",
            [],
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add settings.kitty_image_quality column");
                }
            }
        }

        match self.conn.execute(
            "ALTER TABLE settings ADD COLUMN library_roots_json TEXT NOT NULL DEFAULT '[]'",
            [],
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add settings.library_roots_json column");
                }
            }
        }

        match self.conn.execute(
            "ALTER TABLE books ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0",
            [],
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add books.favorite column");
                }
            }
        }

        Ok(())
    }

    pub fn load_settings(&self) -> anyhow::Result<Settings> {
        let row = self
            .conn
            .query_row(
                "SELECT reader_mode, reader_text_mode, reader_trim_headers_footers, kitty_image_quality, scan_scope, library_roots_json FROM settings WHERE id = 1",
                [],
                |row| {
                    let reader_mode: String = row.get(0)?;
                    let reader_text_mode: String = row.get(1)?;
                    let reader_trim_headers_footers: i64 = row.get(2)?;
                    let kitty_image_quality: String = row.get(3)?;
                    let scan_scope: String = row.get(4)?;
                    let library_roots_json: String = row.get(5)?;
                    Ok((
                        reader_mode,
                        reader_text_mode,
                        reader_trim_headers_footers,
                        kitty_image_quality,
                        scan_scope,
                        library_roots_json,
                    ))
                },
            )
            .optional()?;

        let (
            reader_mode,
            reader_text_mode,
            reader_trim_headers_footers,
            kitty_image_quality,
            scan_scope,
            library_roots_json,
        ) = match row {
            Some(value) => value,
            None => (
                "text".to_string(),
                "reflow".to_string(),
                1,
                "balanced".to_string(),
                "recursive".to_string(),
                "[]".to_string(),
            ),
        };

        let reader_mode = reader_mode
            .parse::<ReaderMode>()
            .unwrap_or(ReaderMode::Text);
        let reader_text_mode = reader_text_mode
            .parse::<ReaderTextMode>()
            .unwrap_or(ReaderTextMode::Reflow);
        let kitty_image_quality = kitty_image_quality
            .parse::<KittyImageQuality>()
            .unwrap_or(KittyImageQuality::Balanced);
        let reader_trim_headers_footers = reader_trim_headers_footers != 0;
        let scan_scope = scan_scope
            .parse::<ScanScope>()
            .unwrap_or(ScanScope::Recursive);
        let library_roots: Vec<String> =
            serde_json::from_str(&library_roots_json).unwrap_or_else(|_| Vec::new());

        let mut settings = Settings {
            reader_mode,
            reader_text_mode,
            reader_trim_headers_footers,
            kitty_image_quality,
            scan_scope,
            library_roots,
        };
        settings.normalize();
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> anyhow::Result<()> {
        let mut settings = settings.clone();
        settings.normalize();
        let library_roots_json = serde_json::to_string(&settings.library_roots)?;

        self.conn.execute(
            "UPDATE settings SET reader_mode = ?, reader_text_mode = ?, reader_trim_headers_footers = ?, kitty_image_quality = ?, scan_scope = ?, library_roots_json = ? WHERE id = 1",
            (
                settings.reader_mode.as_str(),
                settings.reader_text_mode.as_str(),
                i64::from(settings.reader_trim_headers_footers),
                settings.kitty_image_quality.as_str(),
                settings.scan_scope.as_str(),
                library_roots_json,
            ),
        )?;
        Ok(())
    }

    pub fn upsert_book(&self, book: &Book) -> anyhow::Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO books (path, title) VALUES (?, ?)
            ON CONFLICT(path) DO UPDATE SET title = excluded.title
            "#,
            (&book.path, &book.title),
        )?;
        Ok(())
    }

    pub fn list_books(&self) -> anyhow::Result<Vec<Book>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, title, last_opened, favorite FROM books ORDER BY title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            let favorite: i64 = row.get(3)?;
            Ok(Book {
                path: row.get(0)?,
                title: row.get(1)?,
                last_opened: row.get(2)?,
                favorite: favorite != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_last_opened(&self, path: &str, last_opened: i64) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE books SET last_opened = ? WHERE path = ?",
            (last_opened, path),
        )?;
        Ok(())
    }

    pub fn set_favorite(&self, path: &str, favorite: bool) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE books SET favorite = ? WHERE path = ?",
            (i64::from(favorite), path),
        )?;
        Ok(())
    }

    pub fn list_labels_by_path(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, BookLabels>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT bt.path, t.name, t.kind
            FROM book_tags bt
            JOIN tags t ON t.id = bt.tag_id
            ORDER BY bt.path, t.kind, t.name COLLATE NOCASE
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let name: String = row.get(1)?;
            let kind: String = row.get(2)?;
            Ok((path, name, kind))
        })?;

        let mut out: std::collections::HashMap<String, BookLabels> =
            std::collections::HashMap::new();
        for row in rows {
            let (path, name, kind) = row?;
            let kind = kind.parse::<TagKind>().unwrap_or(TagKind::Tag);
            let entry = out.entry(path).or_default();
            match kind {
                TagKind::Tag => entry.tags.push(name),
                TagKind::Collection => {
                    if entry.collection.is_none() {
                        entry.collection = Some(name);
                    }
                }
            }
        }

        for labels in out.values_mut() {
            labels.normalize();
        }

        Ok(out)
    }

    pub fn save_labels(&self, path: &str, labels: &BookLabels) -> anyhow::Result<()> {
        let mut labels = labels.clone();
        labels.normalize();

        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM book_tags WHERE path = ?", [path])?;

        if let Some(collection) = labels.collection.as_deref() {
            let id = get_or_create_tag_id(&tx, collection, TagKind::Collection)
                .context("get/create collection tag")?;
            tx.execute(
                "INSERT OR IGNORE INTO book_tags (path, tag_id) VALUES (?, ?)",
                (path, id),
            )?;
        }

        for tag in &labels.tags {
            let id = get_or_create_tag_id(&tx, tag, TagKind::Tag).context("get/create tag")?;
            tx.execute(
                "INSERT OR IGNORE INTO book_tags (path, tag_id) VALUES (?, ?)",
                (path, id),
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn list_bookmarks_by_path(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, Vec<Bookmark>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, page, label FROM bookmarks ORDER BY path, page, label")?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let page: i64 = row.get(1)?;
            let label: String = row.get(2)?;
            let page = u32::try_from(page).unwrap_or(1).max(1);
            Ok((path, Bookmark { page, label }))
        })?;

        let mut out: std::collections::HashMap<String, Vec<Bookmark>> =
            std::collections::HashMap::new();
        for row in rows {
            let (path, bookmark) = row?;
            out.entry(path).or_default().push(bookmark);
        }
        Ok(out)
    }

    pub fn replace_bookmarks(&self, path: &str, bookmarks: &[Bookmark]) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM bookmarks WHERE path = ?", [path])?;
        for bookmark in bookmarks {
            let page = bookmark.page.max(1) as i64;
            tx.execute(
                "INSERT OR IGNORE INTO bookmarks (path, page, label) VALUES (?, ?, ?)",
                (path, page, bookmark.label.as_str()),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_notes_by_path(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, Vec<Note>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, page, body FROM notes ORDER BY path, page, body")?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let page: i64 = row.get(1)?;
            let body: String = row.get(2)?;
            let page = u32::try_from(page).unwrap_or(1).max(1);
            Ok((path, Note { page, body }))
        })?;

        let mut out: std::collections::HashMap<String, Vec<Note>> =
            std::collections::HashMap::new();
        for row in rows {
            let (path, note) = row?;
            out.entry(path).or_default().push(note);
        }
        Ok(out)
    }

    pub fn replace_notes(&self, path: &str, notes: &[Note]) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM notes WHERE path = ?", [path])?;
        for note in notes {
            let page = note.page.max(1) as i64;
            tx.execute(
                "INSERT OR IGNORE INTO notes (path, page, body) VALUES (?, ?, ?)",
                (path, page, note.body.as_str()),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_book_by_path(&self, path: &str) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM books WHERE path = ?", [path])?;
        Ok(())
    }

    pub fn list_progress(&self) -> anyhow::Result<std::collections::HashMap<String, u32>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, last_page FROM book_progress")?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let last_page: i64 = row.get(1)?;
            let last_page = u32::try_from(last_page).unwrap_or(1).max(1);
            Ok((path, last_page))
        })?;

        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (path, last_page) = row?;
            out.insert(path, last_page);
        }
        Ok(out)
    }

    pub fn set_progress(&self, path: &str, last_page: u32) -> anyhow::Result<()> {
        let last_page = last_page.max(1) as i64;
        self.conn.execute(
            r#"
            INSERT INTO book_progress (path, last_page, updated_at) VALUES (?, ?, unixepoch())
            ON CONFLICT(path) DO UPDATE SET last_page = excluded.last_page, updated_at = excluded.updated_at
            "#,
            (path, last_page),
        )?;
        Ok(())
    }
}

fn get_or_create_tag_id(
    tx: &rusqlite::Transaction<'_>,
    name: &str,
    kind: TagKind,
) -> anyhow::Result<i64> {
    let name = name.trim();
    tx.execute(
        "INSERT INTO tags (name, kind) VALUES (?, ?) ON CONFLICT(name, kind) DO NOTHING",
        (name, kind.as_str()),
    )?;
    let id: i64 = tx
        .query_row(
            "SELECT id FROM tags WHERE name = ? AND kind = ?",
            (name, kind.as_str()),
            |row| row.get(0),
        )
        .context("select tag id")?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_in_memory() -> anyhow::Result<Storage> {
        let conn = Connection::open_in_memory()?;
        let storage = Storage { conn };
        storage.migrate()?;
        Ok(storage)
    }

    #[test]
    fn settings_roundtrip() -> anyhow::Result<()> {
        let storage = open_in_memory()?;
        let mut settings = storage.load_settings()?;
        settings.reader_mode = ReaderMode::Image;
        settings.reader_text_mode = ReaderTextMode::Raw;
        settings.reader_trim_headers_footers = false;
        settings.kitty_image_quality = KittyImageQuality::Sharp;
        settings.scan_scope = ScanScope::Direct;
        settings.library_roots = vec!["/tmp".to_string()];
        storage.save_settings(&settings)?;

        let settings2 = storage.load_settings()?;
        assert_eq!(settings2.reader_mode, ReaderMode::Image);
        assert_eq!(settings2.reader_text_mode, ReaderTextMode::Raw);
        assert!(!settings2.reader_trim_headers_footers);
        assert_eq!(settings2.kitty_image_quality, KittyImageQuality::Sharp);
        assert_eq!(settings2.scan_scope, ScanScope::Direct);
        assert_eq!(settings2.library_roots, vec!["/tmp".to_string()]);
        Ok(())
    }

    #[test]
    fn book_roundtrip() -> anyhow::Result<()> {
        let storage = open_in_memory()?;
        let book = Book {
            path: "/a/b.pdf".to_string(),
            title: "b".to_string(),
            last_opened: None,
            favorite: false,
        };
        storage.upsert_book(&book)?;
        let books = storage.list_books()?;
        assert_eq!(books, vec![book]);
        Ok(())
    }

    #[test]
    fn favorite_roundtrip() -> anyhow::Result<()> {
        let storage = open_in_memory()?;
        let mut book = Book {
            path: "/a/b.pdf".to_string(),
            title: "b".to_string(),
            last_opened: None,
            favorite: false,
        };
        storage.upsert_book(&book)?;
        storage.set_favorite(&book.path, true)?;

        book.favorite = true;
        let books = storage.list_books()?;
        assert_eq!(books, vec![book]);
        Ok(())
    }

    #[test]
    fn labels_roundtrip_and_cascade() -> anyhow::Result<()> {
        let storage = open_in_memory()?;
        let book = Book {
            path: "/a/b.pdf".to_string(),
            title: "b".to_string(),
            last_opened: None,
            favorite: false,
        };
        storage.upsert_book(&book)?;

        storage.save_labels(
            &book.path,
            &BookLabels {
                tags: vec!["rust".to_string(), "tui".to_string()],
                collection: Some("work".to_string()),
            },
        )?;

        let labels = storage.list_labels_by_path()?;
        assert_eq!(
            labels.get(&book.path).cloned(),
            Some(BookLabels {
                tags: vec!["rust".to_string(), "tui".to_string()],
                collection: Some("work".to_string())
            })
        );

        storage.delete_book_by_path(&book.path)?;
        assert!(storage.list_labels_by_path()?.is_empty());
        Ok(())
    }

    #[test]
    fn progress_roundtrip() -> anyhow::Result<()> {
        let storage = open_in_memory()?;
        let book = Book {
            path: "/a/b.pdf".to_string(),
            title: "b".to_string(),
            last_opened: None,
            favorite: false,
        };
        storage.upsert_book(&book)?;

        storage.set_progress(&book.path, 3)?;
        let progress = storage.list_progress()?;
        assert_eq!(progress.get(&book.path).copied(), Some(3));

        storage.delete_book_by_path(&book.path)?;
        let progress = storage.list_progress()?;
        assert!(progress.is_empty());
        Ok(())
    }

    #[test]
    fn bookmarks_and_notes_cascade_on_delete() -> anyhow::Result<()> {
        let storage = open_in_memory()?;
        let book = Book {
            path: "/a/b.pdf".to_string(),
            title: "b".to_string(),
            last_opened: None,
            favorite: false,
        };
        storage.upsert_book(&book)?;

        storage.replace_bookmarks(
            &book.path,
            &[Bookmark {
                page: 2,
                label: "start".to_string(),
            }],
        )?;
        storage.replace_notes(
            &book.path,
            &[Note {
                page: 2,
                body: "hello".to_string(),
            }],
        )?;

        assert_eq!(
            storage.list_bookmarks_by_path()?.get(&book.path).cloned(),
            Some(vec![Bookmark {
                page: 2,
                label: "start".to_string()
            }])
        );
        assert_eq!(
            storage.list_notes_by_path()?.get(&book.path).cloned(),
            Some(vec![Note {
                page: 2,
                body: "hello".to_string()
            }])
        );

        storage.delete_book_by_path(&book.path)?;
        assert!(storage.list_bookmarks_by_path()?.is_empty());
        assert!(storage.list_notes_by_path()?.is_empty());
        Ok(())
    }
}
