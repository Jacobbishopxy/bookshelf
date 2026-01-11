use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use bookshelf_application::AppContext;
use bookshelf_core::{Book, ScanScope, Settings, encode_path};
use bookshelf_storage::Storage;
use bookshelf_ui::{Ui, UiExit};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("get cwd")?;
    let cwd_str = cwd.to_string_lossy().to_string();

    let db_dir = cwd.join(".bookshelf");
    fs::create_dir_all(&db_dir).with_context(|| format!("create db dir {}", db_dir.display()))?;
    let db_path = db_dir.join("bookshelf.db");
    let storage = Storage::open(&db_path)?;
    let mut settings = storage.load_settings()?;

    if settings.library_roots.is_empty() {
        settings.library_roots.push(cwd_str.clone());
        settings.normalize();
        storage.save_settings(&settings)?;
    }

    sync_library(&storage, &settings, &cwd)?;
    let books = storage.list_books()?;
    let progress_by_path = storage.list_progress()?;
    let bookmarks_by_path = storage.list_bookmarks_by_path()?;
    let notes_by_path = storage.list_notes_by_path()?;

    let mut ctx = AppContext::new(settings)
        .with_library(cwd_str, books)
        .with_progress(progress_by_path)
        .with_bookmarks(bookmarks_by_path)
        .with_notes(notes_by_path);
    loop {
        let mut ui = Ui::new(ctx);
        let outcome = ui.run()?;
        ctx = outcome.ctx;
        storage.save_settings(&ctx.settings)?;
        for (path, last_page) in ctx.progress_by_path.iter() {
            storage.set_progress(path, *last_page)?;
        }
        for (path, opened_at) in ctx.opened_at_by_path.iter() {
            storage.set_last_opened(path, *opened_at)?;
        }
        ctx.opened_at_by_path.clear();
        let dirty_bookmark_paths = std::mem::take(&mut ctx.dirty_bookmark_paths);
        for path in dirty_bookmark_paths {
            let bookmarks = ctx
                .bookmarks_by_path
                .get(&path)
                .cloned()
                .unwrap_or_default();
            storage.replace_bookmarks(&path, &bookmarks)?;
        }
        let dirty_note_paths = std::mem::take(&mut ctx.dirty_note_paths);
        for path in dirty_note_paths {
            let notes = ctx.notes_by_path.get(&path).cloned().unwrap_or_default();
            storage.replace_notes(&path, &notes)?;
        }

        match outcome.exit {
            UiExit::Quit => break,
            UiExit::Rescan => {
                sync_library(&storage, &ctx.settings, &cwd)?;
                let books = storage.list_books()?;
                let progress_by_path = storage.list_progress()?;
                let bookmarks_by_path = storage.list_bookmarks_by_path()?;
                let notes_by_path = storage.list_notes_by_path()?;
                let cwd_str = ctx.cwd.clone();
                ctx = ctx
                    .with_library(cwd_str, books)
                    .with_progress(progress_by_path)
                    .with_bookmarks(bookmarks_by_path)
                    .with_notes(notes_by_path);
            }
        }
    }

    Ok(())
}

fn sync_library(storage: &Storage, settings: &Settings, cwd: &Path) -> anyhow::Result<()> {
    let scanned = scan_pdfs(settings, cwd)?;
    let mut scanned_set = std::collections::HashSet::new();
    for book in scanned {
        scanned_set.insert(book.path.clone());
        storage.upsert_book(&book)?;
    }

    let existing = storage.list_books()?;
    for book in existing {
        if !scanned_set.contains(&book.path) {
            storage.delete_book_by_path(&book.path)?;
        }
    }

    Ok(())
}

fn scan_pdfs(settings: &Settings, cwd: &Path) -> anyhow::Result<Vec<Book>> {
    let mut found = std::collections::BTreeMap::<String, Book>::new();

    for root in &settings.library_roots {
        let root_path = PathBuf::from(root);
        let root_path = if root_path.is_absolute() {
            root_path
        } else {
            cwd.join(root_path)
        };

        if root_path.is_file() {
            if is_pdf(&root_path) {
                add_book(&mut found, &root_path)?;
            }
            continue;
        }

        if !root_path.is_dir() {
            continue;
        }

        match settings.scan_scope {
            ScanScope::Direct => {
                for entry in fs::read_dir(&root_path)
                    .with_context(|| format!("read dir {}", root_path.display()))?
                {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_file() && is_pdf(&path) {
                        add_book(&mut found, &path)?;
                    }
                }
            }
            ScanScope::Recursive => {
                let mut stack = vec![root_path];
                while let Some(dir) = stack.pop() {
                    for entry in
                        fs::read_dir(&dir).with_context(|| format!("read dir {}", dir.display()))?
                    {
                        let entry = entry?;
                        let path = entry.path();
                        if path.is_dir() {
                            stack.push(path);
                        } else if path.is_file() && is_pdf(&path) {
                            add_book(&mut found, &path)?;
                        }
                    }
                }
            }
        }
    }

    Ok(found.into_values().collect())
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

fn add_book(out: &mut std::collections::BTreeMap<String, Book>, path: &Path) -> anyhow::Result<()> {
    let normalized = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => path.to_path_buf(),
    };
    let path_str = encode_path(&normalized);
    let title = normalized
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "untitled".to_string());

    out.insert(
        path_str.clone(),
        Book {
            path: path_str,
            title,
            last_opened: None,
        },
    );
    Ok(())
}
