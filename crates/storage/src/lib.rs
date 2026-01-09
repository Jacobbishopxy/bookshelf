//! Sqlite-backed persistence.

use std::path::Path;

use anyhow::Context as _;
use bookshelf_core::{Book, PreviewMode, ScanScope, Settings};
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
        self.conn.execute_batch(
            r#"
            PRAGMA foreign_keys=ON;

            CREATE TABLE IF NOT EXISTS settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                preview_mode TEXT NOT NULL,
                preview_depth INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO settings (id, preview_mode, preview_depth)
            VALUES (1, 'text', 5);

            CREATE TABLE IF NOT EXISTS books (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                added_at INTEGER NOT NULL DEFAULT (unixepoch()),
                last_opened INTEGER
            );

            CREATE TABLE IF NOT EXISTS book_progress (
                path TEXT PRIMARY KEY REFERENCES books(path) ON DELETE CASCADE,
                last_page INTEGER NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (unixepoch())
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
            "ALTER TABLE settings ADD COLUMN preview_pages INTEGER NOT NULL DEFAULT 2",
            [],
        ) {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add settings.preview_pages column");
                }
            }
        }

        match self
            .conn
            .execute("ALTER TABLE settings ADD COLUMN library_roots_json TEXT NOT NULL DEFAULT '[]'", [])
        {
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(err).context("add settings.library_roots_json column");
                }
            }
        }

        Ok(())
    }

    pub fn load_settings(&self) -> anyhow::Result<Settings> {
        let row = self
            .conn
            .query_row(
                "SELECT preview_mode, preview_depth, preview_pages, scan_scope, library_roots_json FROM settings WHERE id = 1",
                [],
                |row| {
                    let preview_mode: String = row.get(0)?;
                    let preview_depth: i64 = row.get(1)?;
                    let preview_pages: i64 = row.get(2)?;
                    let scan_scope: String = row.get(3)?;
                    let library_roots_json: String = row.get(4)?;
                    Ok((preview_mode, preview_depth, preview_pages, scan_scope, library_roots_json))
                },
            )
            .optional()?;

        let (preview_mode, preview_depth, preview_pages, scan_scope, library_roots_json) = match row {
            Some(value) => value,
            None => ("text".to_string(), 5, 2, "recursive".to_string(), "[]".to_string()),
        };

        let preview_mode = preview_mode.parse::<PreviewMode>().unwrap_or(PreviewMode::Text);
        let preview_depth = usize::try_from(preview_depth).unwrap_or(5);
        let preview_pages = usize::try_from(preview_pages).unwrap_or(2);
        let scan_scope = scan_scope.parse::<ScanScope>().unwrap_or(ScanScope::Recursive);
        let library_roots: Vec<String> =
            serde_json::from_str(&library_roots_json).unwrap_or_else(|_| Vec::new());

        let mut settings = Settings {
            preview_mode,
            preview_depth,
            preview_pages,
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
            "UPDATE settings SET preview_mode = ?, preview_depth = ?, preview_pages = ?, scan_scope = ?, library_roots_json = ? WHERE id = 1",
            (
                settings.preview_mode.as_str(),
                settings.preview_depth as i64,
                settings.preview_pages as i64,
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
        let mut stmt = self
            .conn
            .prepare("SELECT path, title FROM books ORDER BY title COLLATE NOCASE")?;
        let rows = stmt.query_map([], |row| {
            Ok(Book {
                path: row.get(0)?,
                title: row.get(1)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
        settings.preview_mode = PreviewMode::Blocks;
        settings.preview_depth = 42;
        settings.preview_pages = 3;
        settings.scan_scope = ScanScope::Direct;
        settings.library_roots = vec!["/tmp".to_string()];
        storage.save_settings(&settings)?;

        let settings2 = storage.load_settings()?;
        assert_eq!(settings2.preview_mode, PreviewMode::Blocks);
        assert_eq!(settings2.preview_depth, 42);
        assert_eq!(settings2.preview_pages, 3);
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
        };
        storage.upsert_book(&book)?;
        let books = storage.list_books()?;
        assert_eq!(books, vec![book]);
        Ok(())
    }

    #[test]
    fn progress_roundtrip() -> anyhow::Result<()> {
        let storage = open_in_memory()?;
        let book = Book {
            path: "/a/b.pdf".to_string(),
            title: "b".to_string(),
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
}
