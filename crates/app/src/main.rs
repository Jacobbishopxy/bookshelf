use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use bookshelf_application::AppContext;
use bookshelf_core::{Book, ScanScope, Settings};
use bookshelf_storage::Storage;
use bookshelf_ui::{Ui, UiExit};
use directories::ProjectDirs;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let project_dirs =
        ProjectDirs::from("dev", "xiey", "bookshelf").context("resolve project dirs")?;

    let config_dir = project_dirs.config_dir();
    fs::create_dir_all(config_dir)
        .with_context(|| format!("create config dir {}", config_dir.display()))?;

    let db_path = config_dir.join("bookshelf.db");
    let storage = Storage::open(&db_path)?;
    let mut settings = storage.load_settings()?;

    let cwd = std::env::current_dir().context("get cwd")?;
    let cwd_str = cwd.to_string_lossy().to_string();
    if settings.library_roots.is_empty() {
        settings.library_roots.push(cwd_str.clone());
        settings.normalize();
        storage.save_settings(&settings)?;
    }

    sync_library(&storage, &settings, &cwd)?;
    let books = storage.list_books()?;

    let mut ctx = AppContext::new(settings).with_library(cwd_str, books);
    loop {
        let mut ui = Ui::new(ctx);
        let outcome = ui.run()?;
        ctx = outcome.ctx;
        storage.save_settings(&ctx.settings)?;

        match outcome.exit {
            UiExit::Quit => break,
            UiExit::Rescan => {
                sync_library(&storage, &ctx.settings, &cwd)?;
                let books = storage.list_books()?;
                let cwd_str = ctx.cwd.clone();
                ctx = ctx.with_library(cwd_str, books);
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
                    for entry in fs::read_dir(&dir)
                        .with_context(|| format!("read dir {}", dir.display()))?
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
    let path_str = normalized.to_string_lossy().to_string();
    let title = normalized
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();

    out.insert(
        path_str.clone(),
        Book {
            path: path_str,
            title,
        },
    );
    Ok(())
}
