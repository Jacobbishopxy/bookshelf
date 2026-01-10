//! ratatui-based UI.

use std::hash::Hasher;
use std::io::{self, Stdout};
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use bookshelf_application::AppContext;
use bookshelf_core::{
    Bookmark, MAX_PREVIEW_DEPTH, MAX_PREVIEW_PAGES, Note, PreviewMode, Settings, TocItem,
};
use bookshelf_engine::Engine;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{event, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap,
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiExit {
    Quit,
    Rescan,
}

#[derive(Debug, Clone)]
pub struct UiOutcome {
    pub ctx: AppContext,
    pub exit: UiExit,
}

#[derive(Debug)]
pub struct Ui {
    ctx: AppContext,
    settings_panel: SettingsPanel,
    preview_panel: PreviewPanel,
    scan_panel: ScanPathPanel,
    search_panel: SearchPanel,
    bookmarks_panel: BookmarksPanel,
    notes_panel: NotesPanel,
    toc_panel: TocPanel,
    reader: ReaderPanel,
    engine: Engine,
    meta_cache: BookMetaCache,
}

impl Ui {
    pub fn new(mut ctx: AppContext) -> Self {
        ctx.settings.normalize();
        let settings_panel = SettingsPanel::default();
        let preview_panel = PreviewPanel::new(ctx.settings.clone());
        let scan_panel = ScanPathPanel::new(join_roots(&ctx.settings));
        let search_panel = SearchPanel::default();
        let bookmarks_panel = BookmarksPanel::default();
        let notes_panel = NotesPanel::default();
        let toc_panel = TocPanel::default();
        let reader = ReaderPanel::default();
        let meta_cache = BookMetaCache::default();
        Self {
            ctx,
            settings_panel,
            preview_panel,
            scan_panel,
            search_panel,
            bookmarks_panel,
            notes_panel,
            toc_panel,
            reader,
            engine: Engine::new(),
            meta_cache,
        }
    }

    pub fn run(&mut self) -> anyhow::Result<UiOutcome> {
        let mut terminal = setup_terminal()?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.event_loop(&mut terminal)
        }));
        let restore_result = restore_terminal(&mut terminal);

        match (result, restore_result) {
            (Ok(res), Ok(())) => res,
            (Ok(_), Err(err)) => Err(err),
            (Err(panic), Ok(())) => Err(anyhow::anyhow!(panic_to_string(panic))),
            (Err(panic), Err(err)) => Err(anyhow::anyhow!(
                "{}\n(additionally failed to restore terminal: {err})",
                panic_to_string(panic)
            )),
        }
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> anyhow::Result<UiOutcome> {
        let tick_rate = Duration::from_millis(250);

        loop {
            terminal.draw(|frame| self.draw(frame.area(), frame))?;

            if event::poll(tick_rate)?
                && let Event::Key(key) = event::read()?
            {
                if key.kind == KeyEventKind::Release {
                    continue;
                }

                if self.settings_panel.open {
                    if let Some(exit) = self.handle_settings_panel_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                } else if self.search_panel.open {
                    if let Some(exit) = self.handle_search_panel_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                } else if self.reader.open && self.bookmarks_panel.open {
                    if let Some(exit) = self.handle_bookmarks_panel_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                } else if self.reader.open && self.toc_panel.open {
                    if let Some(exit) = self.handle_toc_panel_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                } else if self.reader.open && self.notes_panel.open {
                    if let Some(exit) = self.handle_notes_panel_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                } else if self.reader.open {
                    if let Some(exit) = self.handle_reader_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                } else if self.scan_panel.open {
                    if let Some(exit) = self.handle_scan_panel_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                } else if self.preview_panel.open {
                    if let Some(exit) = self.handle_preview_panel_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                } else if let Some(exit) = self.handle_main_key(key)? {
                    return Ok(UiOutcome {
                        ctx: self.ctx.clone(),
                        exit,
                    });
                }
            }
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => Ok(Some(UiExit::Quit)),
            KeyCode::Char('/') => {
                self.search_panel.open = true;
                self.normalize_selection_to_visible();
                Ok(None)
            }
            KeyCode::Char('s') => {
                self.settings_panel.open = true;
                self.settings_panel.selected = 0;
                Ok(None)
            }
            KeyCode::Enter => {
                if let Some(idx) = self.selected_visible_index() {
                    let opened_at = unix_now_secs();
                    if let Some(book) = self.ctx.books.get_mut(idx) {
                        book.last_opened = Some(opened_at);
                        self.ctx
                            .opened_at_by_path
                            .insert(book.path.clone(), opened_at);
                        let book = book.clone();
                        self.reader.open_book(&book, &self.ctx, &self.engine);
                    }
                }
                Ok(None)
            }
            KeyCode::Down => {
                self.select_next_visible();
                Ok(None)
            }
            KeyCode::Up => {
                self.select_prev_visible();
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_search_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                self.search_panel.open = false;
                Ok(None)
            }
            KeyCode::Enter => {
                if let Some(idx) = self.selected_visible_index() {
                    let opened_at = unix_now_secs();
                    if let Some(book) = self.ctx.books.get_mut(idx) {
                        book.last_opened = Some(opened_at);
                        self.ctx
                            .opened_at_by_path
                            .insert(book.path.clone(), opened_at);
                        let book = book.clone();
                        self.search_panel.open = false;
                        self.reader.open_book(&book, &self.ctx, &self.engine);
                    }
                }
                Ok(None)
            }
            KeyCode::Backspace => {
                self.search_panel.query.pop();
                self.normalize_selection_to_visible();
                Ok(None)
            }
            KeyCode::Up => {
                self.select_prev_visible();
                Ok(None)
            }
            KeyCode::Down => {
                self.select_next_visible();
                Ok(None)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_panel.query.clear();
                self.normalize_selection_to_visible();
                Ok(None)
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
                    self.search_panel.query.push(ch);
                    self.normalize_selection_to_visible();
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_reader_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                if let Some(path) = self.reader.book_path.clone() {
                    self.ctx
                        .progress_by_path
                        .insert(path, self.reader.page.saturating_add(1));
                }
                self.reader = ReaderPanel::default();
                self.bookmarks_panel = BookmarksPanel::default();
                self.notes_panel = NotesPanel::default();
                self.toc_panel = TocPanel::default();
                Ok(None)
            }
            KeyCode::Char('d') => {
                if let Some(book) = self.reader.current_book() {
                    let dir = Path::new(&self.ctx.cwd).join("tmp");
                    std::fs::create_dir_all(&dir)?;

                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    std::hash::Hash::hash(&book.path, &mut hasher);
                    let id = hasher.finish();

                    let path = dir.join(format!(
                        "bookshelf-reader-debug-{id:016x}-p{}.txt",
                        self.reader.page + 1
                    ));
                    let debug = self.engine.debug_page_text(&book, self.reader.page)?;
                    std::fs::write(&path, debug)?;
                    self.reader.notice = Some(format!("wrote {}", path.display()));
                }
                Ok(None)
            }
            KeyCode::Char('b') => {
                self.bookmarks_panel.open = true;
                self.bookmarks_panel.selected = 0;
                self.notes_panel.open = false;
                self.toc_panel.open = false;
                Ok(None)
            }
            KeyCode::Char('n') => {
                self.notes_panel.open = true;
                self.notes_panel.selected = 0;
                self.notes_panel.error = None;
                self.bookmarks_panel.open = false;
                self.toc_panel.open = false;
                Ok(None)
            }
            KeyCode::Char('t') => {
                self.open_toc_panel();
                Ok(None)
            }
            KeyCode::Left => {
                self.reader.prev_page();
                Ok(None)
            }
            KeyCode::Right => {
                self.reader.next_page();
                Ok(None)
            }
            KeyCode::Up => {
                self.reader.scroll_up();
                Ok(None)
            }
            KeyCode::Down => {
                self.reader.scroll_down();
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn open_toc_panel(&mut self) {
        self.toc_panel.open = true;
        self.toc_panel.error = None;
        self.bookmarks_panel.open = false;
        self.notes_panel.open = false;

        let Some(book) = self.reader.current_book() else {
            self.toc_panel.items.clear();
            self.toc_panel.path = None;
            self.toc_panel.error = Some("no book".to_string());
            return;
        };

        if self.toc_panel.path.as_deref() != Some(&book.path) {
            match self.engine.toc(&book) {
                Ok(items) => {
                    self.toc_panel.items = items;
                    self.toc_panel.path = Some(book.path.clone());
                    self.toc_panel.error = None;
                }
                Err(err) => {
                    self.toc_panel.items.clear();
                    self.toc_panel.path = Some(book.path.clone());
                    self.toc_panel.error = Some(err.to_string());
                }
            }
        }

        let current_page = self.reader.page.saturating_add(1);
        let mut best = 0usize;
        for (idx, item) in self.toc_panel.items.iter().enumerate() {
            if let Some(page) = item.page
                && page <= current_page
            {
                best = idx;
            }
        }
        self.toc_panel.selected = best;
    }

    fn handle_toc_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                self.toc_panel.open = false;
                Ok(None)
            }
            KeyCode::Up => {
                self.toc_panel.selected = self.toc_panel.selected.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Down => {
                let len = self.toc_panel.items.len();
                if len > 0 {
                    self.toc_panel.selected = (self.toc_panel.selected + 1).min(len - 1);
                }
                Ok(None)
            }
            KeyCode::Enter => {
                let Some(item) = self.toc_panel.items.get(self.toc_panel.selected) else {
                    return Ok(None);
                };
                let Some(page) = item.page else {
                    self.reader.notice = Some("TOC entry has no page".to_string());
                    return Ok(None);
                };
                self.reader.page = page.saturating_sub(1);
                self.reader.invalidate_render();
                self.toc_panel.open = false;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_bookmarks_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                self.bookmarks_panel.open = false;
                Ok(None)
            }
            KeyCode::Up => {
                self.bookmarks_panel.selected = self.bookmarks_panel.selected.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Down => {
                let len = self.current_bookmarks().len();
                if len > 0 {
                    self.bookmarks_panel.selected =
                        (self.bookmarks_panel.selected + 1).min(len - 1);
                }
                Ok(None)
            }
            KeyCode::Enter => {
                let Some(bookmark) = self
                    .current_bookmarks()
                    .get(self.bookmarks_panel.selected)
                    .cloned()
                else {
                    return Ok(None);
                };
                self.reader.page = bookmark.page.saturating_sub(1);
                self.reader.invalidate_render();
                self.bookmarks_panel.open = false;
                Ok(None)
            }
            KeyCode::Char('a') => {
                let Some(path) = self.reader.book_path.clone() else {
                    return Ok(None);
                };
                let page = self.reader.page.saturating_add(1);
                let bookmarks = self.ctx.bookmarks_by_path.entry(path.clone()).or_default();
                if !bookmarks
                    .iter()
                    .any(|b| b.page == page && b.label.is_empty())
                {
                    bookmarks.push(Bookmark {
                        page,
                        label: String::new(),
                    });
                    bookmarks.sort_by_key(|b| (b.page, b.label.clone()));
                    self.ctx.dirty_bookmark_paths.insert(path);
                    self.reader.notice = Some(format!("bookmarked page {page}"));
                    self.bookmarks_panel.selected = bookmarks
                        .iter()
                        .position(|b| b.page == page && b.label.is_empty())
                        .unwrap_or(0);
                }
                Ok(None)
            }
            KeyCode::Char('d') => {
                let Some(path) = self.reader.book_path.clone() else {
                    return Ok(None);
                };
                let bookmarks = self.ctx.bookmarks_by_path.entry(path.clone()).or_default();
                if self.bookmarks_panel.selected < bookmarks.len() {
                    bookmarks.remove(self.bookmarks_panel.selected);
                    bookmarks.sort_by_key(|b| (b.page, b.label.clone()));
                    self.ctx.dirty_bookmark_paths.insert(path);
                    if bookmarks.is_empty() {
                        self.bookmarks_panel.selected = 0;
                    } else {
                        self.bookmarks_panel.selected =
                            self.bookmarks_panel.selected.min(bookmarks.len() - 1);
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_notes_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        if self.notes_panel.input_open {
            return self.handle_notes_input_key(key);
        }

        match key.code {
            KeyCode::Esc => {
                self.notes_panel.open = false;
                self.notes_panel.error = None;
                Ok(None)
            }
            KeyCode::Up => {
                self.notes_panel.selected = self.notes_panel.selected.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Down => {
                let len = self.current_notes().len();
                if len > 0 {
                    self.notes_panel.selected = (self.notes_panel.selected + 1).min(len - 1);
                }
                Ok(None)
            }
            KeyCode::Enter => {
                let Some(note) = self.current_notes().get(self.notes_panel.selected).cloned()
                else {
                    return Ok(None);
                };
                self.reader.page = note.page.saturating_sub(1);
                self.reader.invalidate_render();
                self.notes_panel.open = false;
                Ok(None)
            }
            KeyCode::Char('a') => {
                self.notes_panel.input_open = true;
                self.notes_panel.input_page = self.reader.page.saturating_add(1);
                self.notes_panel.input.clear();
                self.notes_panel.error = None;
                Ok(None)
            }
            KeyCode::Char('d') => {
                let Some(path) = self.reader.book_path.clone() else {
                    return Ok(None);
                };
                let notes = self.ctx.notes_by_path.entry(path.clone()).or_default();
                if self.notes_panel.selected < notes.len() {
                    notes.remove(self.notes_panel.selected);
                    notes.sort_by_key(|n| (n.page, n.body.clone()));
                    self.ctx.dirty_note_paths.insert(path);
                    if notes.is_empty() {
                        self.notes_panel.selected = 0;
                    } else {
                        self.notes_panel.selected = self.notes_panel.selected.min(notes.len() - 1);
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_notes_input_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                self.notes_panel.input_open = false;
                self.notes_panel.input.clear();
                self.notes_panel.error = None;
                Ok(None)
            }
            KeyCode::Enter => {
                let body = self.notes_panel.input.trim().to_string();
                if body.is_empty() {
                    self.notes_panel.error = Some("Note cannot be empty".to_string());
                    return Ok(None);
                }
                let Some(path) = self.reader.book_path.clone() else {
                    return Ok(None);
                };
                let page = self.notes_panel.input_page.max(1);
                let notes = self.ctx.notes_by_path.entry(path.clone()).or_default();
                notes.push(Note { page, body });
                notes.sort_by_key(|n| (n.page, n.body.clone()));
                self.ctx.dirty_note_paths.insert(path);
                self.notes_panel.input_open = false;
                self.notes_panel.input.clear();
                self.notes_panel.error = None;
                Ok(None)
            }
            KeyCode::Backspace => {
                self.notes_panel.input.pop();
                Ok(None)
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
                    self.notes_panel.input.push(ch);
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_settings_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                self.settings_panel.open = false;
                Ok(None)
            }
            KeyCode::Up => {
                self.settings_panel.selected = self.settings_panel.selected.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Down => {
                self.settings_panel.selected = (self.settings_panel.selected + 1).min(1);
                Ok(None)
            }
            KeyCode::Enter => {
                match self.settings_panel.selected {
                    0 => {
                        self.scan_panel.open = true;
                        self.scan_panel.selected = 0;
                        self.scan_panel.input = join_roots(&self.ctx.settings);
                        self.scan_panel.error = None;
                    }
                    1 => {
                        self.preview_panel.open = true;
                        self.preview_panel.draft = self.ctx.settings.clone();
                        self.preview_panel.begin_editing();
                    }
                    _ => {}
                }
                self.settings_panel.open = false;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_preview_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                self.preview_panel.open = false;
                self.preview_panel.editing_numeric = false;
                Ok(None)
            }
            KeyCode::Enter => {
                self.preview_panel.reset_invalid_inputs();
                self.preview_panel.sync_inputs_to_draft();
                self.ctx.settings = self.preview_panel.draft.clone();
                self.ctx.settings.normalize();
                self.preview_panel.open = false;
                self.preview_panel.editing_numeric = false;
                Ok(None)
            }
            KeyCode::Up => {
                self.preview_panel.selected = self.preview_panel.selected.saturating_sub(1);
                self.preview_panel.editing_numeric = false;
                self.preview_panel.reset_invalid_inputs();
                Ok(None)
            }
            KeyCode::Down => {
                self.preview_panel.selected = (self.preview_panel.selected + 1).min(2);
                self.preview_panel.editing_numeric = false;
                self.preview_panel.reset_invalid_inputs();
                Ok(None)
            }
            KeyCode::Char('m') | KeyCode::Tab => {
                if self.preview_panel.selected == 0 {
                    self.preview_panel.draft.cycle_preview_mode();
                }
                Ok(None)
            }
            KeyCode::Left => {
                if self.preview_panel.selected == 0 {
                    self.preview_panel.draft.preview_mode = match self
                        .preview_panel
                        .draft
                        .preview_mode
                    {
                        bookshelf_core::PreviewMode::Text => bookshelf_core::PreviewMode::Text,
                        bookshelf_core::PreviewMode::Braille => bookshelf_core::PreviewMode::Text,
                        bookshelf_core::PreviewMode::Blocks => bookshelf_core::PreviewMode::Braille,
                    };
                }
                Ok(None)
            }
            KeyCode::Right => {
                if self.preview_panel.selected == 0 {
                    self.preview_panel.draft.preview_mode = match self
                        .preview_panel
                        .draft
                        .preview_mode
                    {
                        bookshelf_core::PreviewMode::Text => bookshelf_core::PreviewMode::Braille,
                        bookshelf_core::PreviewMode::Braille => bookshelf_core::PreviewMode::Blocks,
                        bookshelf_core::PreviewMode::Blocks => bookshelf_core::PreviewMode::Blocks,
                    };
                }
                Ok(None)
            }
            KeyCode::Backspace => {
                self.preview_panel.backspace();
                Ok(None)
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                self.preview_panel.push_digit(ch);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_scan_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char('u') = key.code
        {
            self.scan_panel.input.clear();
            return Ok(None);
        }

        match key.code {
            KeyCode::Esc => {
                self.scan_panel.open = false;
                self.scan_panel.error = None;
                Ok(None)
            }
            KeyCode::Up => {
                self.scan_panel.selected = self.scan_panel.selected.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Down => {
                self.scan_panel.selected = (self.scan_panel.selected + 1).min(1);
                Ok(None)
            }
            KeyCode::Left | KeyCode::Right => {
                if self.scan_panel.selected == 1 {
                    self.ctx.settings.cycle_scan_scope();
                }
                Ok(None)
            }
            KeyCode::Enter => {
                let roots = parse_roots_input(&self.scan_panel.input);
                if roots.is_empty() {
                    self.scan_panel.error = Some("Enter at least one path".to_string());
                    return Ok(None);
                }

                self.ctx.settings.library_roots = roots;
                self.ctx.settings.normalize();
                self.scan_panel.open = false;
                self.scan_panel.error = None;
                Ok(Some(UiExit::Rescan))
            }
            KeyCode::Backspace => {
                if self.scan_panel.selected == 0 {
                    self.scan_panel.input.pop();
                }
                Ok(None)
            }
            KeyCode::Char(ch) => {
                if self.scan_panel.selected == 0 && !ch.is_control() {
                    self.scan_panel.input.push(ch);
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.search_panel.query.trim();
        if query.is_empty() {
            return (0..self.ctx.books.len()).collect();
        }

        let query = query.to_ascii_lowercase();
        let mut out = Vec::new();
        for (idx, book) in self.ctx.books.iter().enumerate() {
            let title = book.title.to_ascii_lowercase();
            if title.contains(&query) {
                out.push(idx);
                continue;
            }

            let path = bookshelf_core::display_path(&book.path).to_ascii_lowercase();
            if path.contains(&query) {
                out.push(idx);
            }
        }
        out
    }

    fn normalize_selection_to_visible(&mut self) {
        if self.ctx.books.is_empty() {
            self.ctx.selected = 0;
            return;
        }

        if self.ctx.selected >= self.ctx.books.len() {
            self.ctx.selected = 0;
        }

        let visible = self.visible_indices();
        if visible.is_empty() {
            self.ctx.selected = 0;
            return;
        }

        if !visible.contains(&self.ctx.selected) {
            self.ctx.selected = visible[0];
        }
    }

    fn selected_visible_index(&self) -> Option<usize> {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return None;
        }

        let selected = self
            .ctx
            .selected
            .min(self.ctx.books.len().saturating_sub(1));
        if visible.contains(&selected) {
            Some(selected)
        } else {
            Some(visible[0])
        }
    }

    fn select_next_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }

        let Some(pos) = visible.iter().position(|idx| *idx == self.ctx.selected) else {
            self.ctx.selected = visible[0];
            return;
        };
        if pos + 1 < visible.len() {
            self.ctx.selected = visible[pos + 1];
        }
    }

    fn select_prev_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }

        let Some(pos) = visible.iter().position(|idx| *idx == self.ctx.selected) else {
            self.ctx.selected = visible[0];
            return;
        };
        if pos > 0 {
            self.ctx.selected = visible[pos - 1];
        }
    }

    fn main_footer_spans(&self) -> Vec<Span<'static>> {
        let query = self.search_panel.query.trim();

        let mut spans = if self.search_panel.open {
            vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" close  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" open  "),
                Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" move  "),
                Span::styled("Ctrl+u", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" clear"),
            ]
        } else {
            vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" quit  "),
                Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" move  "),
                Span::styled("/", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" search  "),
                Span::styled("s", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" settings  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" read"),
            ]
        };

        if !query.is_empty() && !self.search_panel.open {
            spans.push(Span::raw("  |  "));
            spans.push(Span::styled(
                format!("filter: {query}"),
                Style::default().fg(Color::Cyan),
            ));
        }

        spans
    }

    fn refresh_meta_cache(&mut self) {
        let Some(selected_idx) = self.selected_visible_index() else {
            self.meta_cache = BookMetaCache::default();
            return;
        };
        let Some(book) = self.ctx.books.get(selected_idx) else {
            self.meta_cache = BookMetaCache::default();
            return;
        };

        if self.meta_cache.path.as_deref() == Some(&book.path) {
            return;
        }

        let decoded = bookshelf_core::decode_path(&book.path);
        let size_bytes = std::fs::metadata(&decoded).ok().map(|m| m.len());
        let page_count = self.engine.page_count(book).ok();

        self.meta_cache = BookMetaCache {
            path: Some(book.path.clone()),
            size_bytes,
            page_count,
        };
    }

    fn current_bookmarks(&self) -> Vec<Bookmark> {
        let Some(path) = self.reader.book_path.as_ref() else {
            return Vec::new();
        };
        self.ctx
            .bookmarks_by_path
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    fn current_notes(&self) -> Vec<Note> {
        let Some(path) = self.reader.book_path.as_ref() else {
            return Vec::new();
        };
        self.ctx
            .notes_by_path
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    fn draw(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        frame.render_widget(Clear, area);
        if self.reader.open {
            self.draw_reader(area, frame);
            return;
        }

        self.normalize_selection_to_visible();
        self.refresh_meta_cache();

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .split(area);

        let title = Paragraph::new(Line::from(vec![
            Span::styled("Bookshelf", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" — library"),
        ]))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(title, layout[0]);

        let body_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(layout[1]);

        self.draw_library(frame, body_layout[0]);
        frame.render_widget(self.draw_details(body_layout[1]), body_layout[1]);

        let footer = Paragraph::new(Line::from(self.main_footer_spans()))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::TOP));
        frame.render_widget(footer, layout[2]);

        if self.settings_panel.open {
            self.draw_settings_panel(area, frame);
        }

        if self.preview_panel.open {
            self.draw_preview_panel(area, frame);
        }

        if self.scan_panel.open {
            self.draw_scan_panel(area, frame);
        }

        if self.search_panel.open {
            self.draw_search_panel(area, frame);
        }
    }

    fn draw_search_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(70, 28, area);
        frame.render_widget(Clear, popup_area);

        let query = self.search_panel.query.trim();
        let visible = self.visible_indices();
        let title = if query.is_empty() {
            "Search".to_string()
        } else {
            format!("Search — {}/{}", visible.len(), self.ctx.books.len())
        };

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let lines = vec![
            Line::from(vec![
                Span::styled("Query: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(self.search_panel.query.clone()),
            ]),
            Line::raw(""),
            Line::raw("Type to filter by title or path."),
            Line::raw("Esc closes, Enter opens, Ctrl+u clears."),
        ];

        let paragraph = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(paragraph, inner);
    }

    fn draw_bookmarks_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(70, 55, area);
        frame.render_widget(Clear, popup_area);

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            "Bookmarks",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(2)])
            .split(inner);
        let bookmarks = self.current_bookmarks();
        let items = if bookmarks.is_empty() {
            vec![ListItem::new(Line::raw("(none)"))]
        } else {
            bookmarks
                .iter()
                .map(|b| {
                    let label = if b.label.trim().is_empty() {
                        format!("Page {}", b.page)
                    } else {
                        format!("Page {} — {}", b.page, b.label.trim())
                    };
                    ListItem::new(Line::raw(label))
                })
                .collect()
        };

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);

        let mut state = ListState::default();
        if !bookmarks.is_empty() {
            state.select(Some(self.bookmarks_panel.selected.min(bookmarks.len() - 1)));
        }
        frame.render_stateful_widget(list, sections[0], &mut state);

        let footer = Paragraph::new(Line::from(vec![
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" close  "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" jump  "),
            Span::styled("a", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" add current  "),
            Span::styled("d", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" delete"),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(footer, sections[1]);
    }

    fn draw_notes_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(80, 60, area);
        frame.render_widget(Clear, popup_area);

        let title = if self.notes_panel.input_open {
            "Notes — Add"
        } else {
            "Notes"
        };
        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .split(inner);

        let mut header_lines = Vec::new();
        if self.notes_panel.input_open {
            header_lines.push(Line::from(vec![
                Span::styled("Page: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(self.notes_panel.input_page.to_string()),
            ]));
            header_lines.push(Line::from(vec![
                Span::styled("Text: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(self.notes_panel.input.clone()),
            ]));
        } else {
            header_lines.push(Line::raw("Notes are single-line for now."));
            header_lines.push(Line::raw("Use 'a' to add a note for the current page."));
        }
        if let Some(err) = &self.notes_panel.error {
            header_lines.push(Line::from(vec![Span::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )]));
        }
        let header = Paragraph::new(Text::from(header_lines)).wrap(Wrap { trim: true });
        frame.render_widget(header, sections[0]);

        let notes = self.current_notes();
        let items = if notes.is_empty() {
            vec![ListItem::new(Line::raw("(none)"))]
        } else {
            notes
                .iter()
                .map(|n| {
                    let body = n.body.trim();
                    let label = if body.is_empty() {
                        format!("Page {}", n.page)
                    } else {
                        format!("Page {} — {}", n.page, body)
                    };
                    ListItem::new(Line::raw(label))
                })
                .collect()
        };

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);

        let mut state = ListState::default();
        if !notes.is_empty() {
            state.select(Some(self.notes_panel.selected.min(notes.len() - 1)));
        }
        frame.render_stateful_widget(list, sections[1], &mut state);

        let footer_spans = if self.notes_panel.input_open {
            vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cancel  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" save"),
            ]
        } else {
            vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" close  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" jump  "),
                Span::styled("a", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" add  "),
                Span::styled("d", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" delete"),
            ]
        };
        let footer = Paragraph::new(Line::from(footer_spans)).alignment(Alignment::Center);
        frame.render_widget(footer, sections[2]);
    }

    fn draw_toc_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(80, 70, area);
        frame.render_widget(Clear, popup_area);

        let title = match self.toc_panel.error.as_deref() {
            Some(_) => "Table of Contents (error)".to_string(),
            None => format!("Table of Contents — {}", self.toc_panel.items.len()),
        };
        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .split(inner);

        let mut header_lines = Vec::new();
        header_lines.push(Line::raw("↑/↓ select, Enter jump, Esc close."));
        if let Some(err) = &self.toc_panel.error {
            header_lines.push(Line::from(vec![Span::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )]));
        }
        frame.render_widget(
            Paragraph::new(Text::from(header_lines)).wrap(Wrap { trim: true }),
            sections[0],
        );

        let items: Vec<ListItem> = if self.toc_panel.items.is_empty() {
            vec![ListItem::new(Line::raw("(no outline found)"))]
        } else {
            self.toc_panel
                .items
                .iter()
                .map(|item| {
                    let indent = "  ".repeat(item.depth.min(12));
                    let page = item
                        .page
                        .map(|p| format!("p{p}"))
                        .unwrap_or_else(|| "-".to_string());
                    ListItem::new(Line::raw(format!("{indent}{}  [{page}]", item.title)))
                })
                .collect()
        };

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);

        let mut state = ListState::default();
        if !self.toc_panel.items.is_empty() {
            state.select(Some(
                self.toc_panel.selected.min(self.toc_panel.items.len() - 1),
            ));
        }
        frame.render_stateful_widget(list, sections[1], &mut state);

        let footer = Paragraph::new(Line::from(vec![
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" close  "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" jump"),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(footer, sections[2]);
    }

    fn draw_reader(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .split(area);

        let title_text = match (&self.reader.book_title, self.reader.total_pages) {
            (Some(title), Some(total)) => format!(
                "Reader — {}  (page {}/{})",
                title,
                self.reader.page.saturating_add(1),
                total
            ),
            (Some(title), None) => format!("Reader — {}  (page {})", title, self.reader.page + 1),
            _ => "Reader".to_string(),
        };

        let header = Paragraph::new(Line::from(vec![Span::styled(
            title_text,
            Style::default().add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(header, layout[0]);

        let inner_width = layout[1].width.saturating_sub(2);
        let inner_height = layout[1].height.saturating_sub(2);
        self.reader
            .ensure_rendered(&self.ctx, &self.engine, inner_width, inner_height);

        let content = self.reader.current_text.clone().unwrap_or_else(|| {
            self.reader
                .last_error
                .clone()
                .unwrap_or_else(|| "loading...".to_string())
        });

        let text = Text::from(
            content
                .lines()
                .skip(self.reader.scroll as usize)
                .map(|line| Line::raw(line.to_string()))
                .collect::<Vec<_>>(),
        );

        let body = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("Page"))
            .wrap(Wrap { trim: false });
        frame.render_widget(body, layout[1]);

        let mut footer_spans = vec![
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" back  "),
            Span::styled("←/→", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" page  "),
            Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" scroll  "),
            Span::styled("t", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" toc  "),
            Span::styled("b", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" bookmarks  "),
            Span::styled("n", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" notes  "),
            Span::styled("d", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" dump"),
        ];

        if let Some(note) = &self.reader.notice {
            footer_spans.push(Span::raw("  |  "));
            footer_spans.push(Span::styled(
                note.clone(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        let footer = Paragraph::new(Line::from(footer_spans))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::TOP));
        frame.render_widget(footer, layout[2]);

        if self.bookmarks_panel.open {
            self.draw_bookmarks_panel(area, frame);
        }
        if self.toc_panel.open {
            self.draw_toc_panel(area, frame);
        }
        if self.notes_panel.open {
            self.draw_notes_panel(area, frame);
        }
    }

    fn draw_settings_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(45, 30, area);
        frame.render_widget(Clear, popup_area);

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            "Settings",
            Style::default().add_modifier(Modifier::BOLD),
        ));

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let items = vec![
            ListItem::new(Line::raw("Scan Paths")),
            ListItem::new(Line::raw("Preview Settings")),
        ];

        let list = List::new(items)
            .block(block)
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);

        let mut state = ListState::default();
        state.select(Some(self.settings_panel.selected.min(1)));
        frame.render_stateful_widget(list, popup_area, &mut state);
    }

    fn draw_library(&self, frame: &mut ratatui::Frame, area: Rect) {
        let visible = self.visible_indices();
        let query = self.search_panel.query.trim();
        let title = if query.is_empty() {
            "Library".to_string()
        } else {
            format!(
                "Library — {}/{} matches",
                visible.len(),
                self.ctx.books.len()
            )
        };
        let block = Block::default().borders(Borders::ALL).title(title);

        if self.ctx.books.is_empty() {
            let mut lines = Vec::new();
            lines.push(Line::raw("No PDFs found."));
            lines.push(Line::raw(""));
            lines.push(Line::raw("Roots:"));
            if self.ctx.settings.library_roots.is_empty() {
                lines.push(Line::raw("(empty)"));
            } else {
                for root in &self.ctx.settings.library_roots {
                    lines.push(Line::raw(format!("- {root}")));
                }
            }

            let paragraph = Paragraph::new(Text::from(lines))
                .block(block)
                .wrap(Wrap { trim: true });
            frame.render_widget(paragraph, area);
            return;
        }

        if visible.is_empty() {
            let mut lines = Vec::new();
            lines.push(Line::raw("No matches."));
            if !query.is_empty() {
                lines.push(Line::raw(""));
                lines.push(Line::raw(format!("Query: {query}")));
                lines.push(Line::raw("Tip: press / to edit, Ctrl+u to clear."));
            }
            let paragraph = Paragraph::new(Text::from(lines))
                .block(block)
                .wrap(Wrap { trim: true });
            frame.render_widget(paragraph, area);
            return;
        }

        let max_title_width = area.width.saturating_sub(6) as usize;
        let items: Vec<ListItem> = visible
            .iter()
            .filter_map(|idx| self.ctx.books.get(*idx))
            .map(|book| {
                let wrapped = wrap_text(&book.title, max_title_width.max(8));
                let lines = wrapped.into_iter().map(Line::raw).collect::<Vec<_>>();
                ListItem::new(Text::from(lines))
            })
            .collect();

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .block(block)
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);

        let mut state = ListState::default();
        let visible_pos = visible.iter().position(|idx| *idx == self.ctx.selected);
        state.select(visible_pos);
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_details(&self, area: Rect) -> Paragraph<'static> {
        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("Preview: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.preview_mode.to_string()),
            Span::raw("  "),
            Span::styled("Depth: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.preview_depth.to_string()),
            Span::raw("  "),
            Span::styled("Pages: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.preview_pages.to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Scan: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.scan_scope.to_string()),
            Span::raw("  "),
            Span::styled("Roots: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.library_roots.len().to_string()),
        ]));
        lines.push(Line::raw(""));

        if let Some(book) = self.ctx.books.get(self.ctx.selected) {
            let preview_width = area.width.saturating_sub(2).max(1);
            lines.push(Line::from(vec![
                Span::styled("Selected: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(book.title.clone()),
            ]));
            lines.push(Line::raw(bookshelf_core::display_path(&book.path)));
            lines.push(Line::raw(""));

            let size = format_bytes_opt(self.meta_cache.size_bytes);
            let pages = self
                .meta_cache
                .page_count
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string());
            lines.push(Line::from(vec![
                Span::styled("Size: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(size),
                Span::raw("  "),
                Span::styled("Pages: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(pages),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    "Last opened: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format_last_opened(book.last_opened)),
            ]));
            lines.push(Line::raw(""));

            let preview = self
                .engine
                .render_preview_for(book, &self.ctx.settings, preview_width);
            for line in preview.lines().take(self.ctx.settings.preview_depth.max(1)) {
                lines.push(Line::raw(line.to_string()));
            }
        } else {
            lines.push(Line::raw("No selection."));
        }

        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title("Details"))
            .wrap(Wrap { trim: true })
    }

    fn draw_preview_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(60, 40, area);
        frame.render_widget(Clear, popup_area);

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            "Preview Settings",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(5)])
            .split(inner);

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let depth_input_ok = self.preview_panel.depth_input.parse::<usize>().is_ok();
        let pages_input_ok = self.preview_panel.pages_input.parse::<usize>().is_ok();

        let mode_row_selected = self.preview_panel.selected == 0;

        let items = vec![
            ListItem::new(Line::from(vec![
                Span::styled(
                    "Preview mode: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                option_chip(
                    "text",
                    self.preview_panel.draft.preview_mode == bookshelf_core::PreviewMode::Text,
                    mode_row_selected,
                ),
                Span::raw(" "),
                option_chip(
                    "braille",
                    self.preview_panel.draft.preview_mode == bookshelf_core::PreviewMode::Braille,
                    mode_row_selected,
                ),
                Span::raw(" "),
                option_chip(
                    "blocks",
                    self.preview_panel.draft.preview_mode == bookshelf_core::PreviewMode::Blocks,
                    mode_row_selected,
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("Preview depth (max {MAX_PREVIEW_DEPTH}): "),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    self.preview_panel.depth_input.clone(),
                    Style::default().fg(if depth_input_ok {
                        Color::Cyan
                    } else {
                        Color::Red
                    }),
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("Preview pages (max {MAX_PREVIEW_PAGES}): "),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    self.preview_panel.pages_input.clone(),
                    Style::default().fg(if pages_input_ok {
                        Color::Cyan
                    } else {
                        Color::Red
                    }),
                ),
            ])),
        ];

        let list = List::new(items)
            .highlight_style(highlight_style)
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_symbol("> ");

        let mut state = ListState::default();
        state.select(Some(self.preview_panel.selected.min(2)));
        frame.render_stateful_widget(list, sections[0], &mut state);

        let help = Paragraph::new(Text::from(vec![
            Line::from(vec![
                Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" select  "),
                Span::styled("Tab/m/←/→", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cycle mode"),
            ]),
            Line::from(vec![
                Span::styled("0-9", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" edit number  "),
                Span::styled("Backspace", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" delete"),
            ]),
            Line::from(vec![
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" apply  "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cancel"),
            ]),
        ]))
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Left);
        frame.render_widget(help, sections[1]);
    }

    fn draw_scan_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(80, 40, area);
        frame.render_widget(Clear, popup_area);

        let title = "Scan Paths";
        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));

        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(6)])
            .split(inner);

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let scope_row_selected = self.scan_panel.selected == 1;

        let items = vec![
            ListItem::new(Line::from(vec![
                Span::styled("Paths: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(self.scan_panel.input.clone()),
            ])),
            ListItem::new(Line::from(vec![
                Span::styled(
                    "Scan scope: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                option_chip(
                    "direct",
                    self.ctx.settings.scan_scope == bookshelf_core::ScanScope::Direct,
                    scope_row_selected,
                ),
                Span::raw(" "),
                option_chip(
                    "recursive",
                    self.ctx.settings.scan_scope == bookshelf_core::ScanScope::Recursive,
                    scope_row_selected,
                ),
            ])),
        ];

        let list = List::new(items)
            .highlight_style(highlight_style)
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_symbol("> ");

        let mut state = ListState::default();
        state.select(Some(self.scan_panel.selected.min(1)));
        frame.render_stateful_widget(list, sections[0], &mut state);

        let mut help_lines = Vec::new();
        help_lines.push(Line::from(vec![
            Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" select  "),
            Span::styled("←/→", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" change scope  "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" apply + rescan"),
        ]));
        help_lines.push(Line::from(vec![
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel  "),
            Span::styled("Backspace", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" delete  "),
            Span::styled("Ctrl+U", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" clear"),
        ]));

        if let Some(err) = &self.scan_panel.error {
            help_lines.push(Line::raw(""));
            help_lines.push(Line::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }

        let help = Paragraph::new(Text::from(help_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(help, sections[1]);
    }
}

#[derive(Debug, Clone)]
struct PreviewPanel {
    open: bool,
    draft: Settings,
    selected: usize,
    depth_input: String,
    pages_input: String,
    editing_numeric: bool,
}

impl PreviewPanel {
    fn new(settings: Settings) -> Self {
        Self {
            open: false,
            draft: settings,
            selected: 0,
            depth_input: String::new(),
            pages_input: String::new(),
            editing_numeric: false,
        }
    }

    fn begin_editing(&mut self) {
        self.depth_input = self.draft.preview_depth.to_string();
        self.pages_input = self.draft.preview_pages.to_string();
        self.selected = 0;
        self.editing_numeric = false;
    }

    fn reset_invalid_inputs(&mut self) {
        if self.depth_input.parse::<usize>().is_err() {
            self.depth_input = self.draft.preview_depth.to_string();
        }
        if self.pages_input.parse::<usize>().is_err() {
            self.pages_input = self.draft.preview_pages.to_string();
        }
    }

    fn push_digit(&mut self, digit: char) {
        let buf = match self.selected {
            1 => &mut self.depth_input,
            2 => &mut self.pages_input,
            _ => return,
        };

        if !self.editing_numeric {
            buf.clear();
            self.editing_numeric = true;
        }

        if buf.len() >= 4 {
            return;
        }
        buf.push(digit);
        self.sync_inputs_to_draft();
    }

    fn backspace(&mut self) {
        let buf = match self.selected {
            1 => &mut self.depth_input,
            2 => &mut self.pages_input,
            _ => return,
        };
        self.editing_numeric = true;
        buf.pop();
        self.sync_inputs_to_draft();
    }

    fn sync_inputs_to_draft(&mut self) {
        let mut depth_ok = false;
        let mut pages_ok = false;
        if let Ok(value) = self.depth_input.parse::<usize>() {
            self.draft.preview_depth = value;
            depth_ok = true;
        }
        if let Ok(value) = self.pages_input.parse::<usize>() {
            self.draft.preview_pages = value;
            pages_ok = true;
        }
        self.draft.normalize();

        if depth_ok {
            self.depth_input = self.draft.preview_depth.to_string();
        }
        if pages_ok {
            self.pages_input = self.draft.preview_pages.to_string();
        }
    }
}

#[derive(Debug, Clone)]
struct ScanPathPanel {
    open: bool,
    selected: usize,
    input: String,
    error: Option<String>,
}

impl ScanPathPanel {
    fn new(input: String) -> Self {
        Self {
            open: false,
            selected: 0,
            input,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct SettingsPanel {
    open: bool,
    selected: usize,
}

#[derive(Debug, Clone, Default)]
struct SearchPanel {
    open: bool,
    query: String,
}

#[derive(Debug, Clone, Default)]
struct BookmarksPanel {
    open: bool,
    selected: usize,
}

#[derive(Debug, Clone, Default)]
struct TocPanel {
    open: bool,
    selected: usize,
    path: Option<String>,
    items: Vec<TocItem>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct NotesPanel {
    open: bool,
    selected: usize,
    input_open: bool,
    input_page: u32,
    input: String,
    error: Option<String>,
}

impl Default for NotesPanel {
    fn default() -> Self {
        Self {
            open: false,
            selected: 0,
            input_open: false,
            input_page: 1,
            input: String::new(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct BookMetaCache {
    path: Option<String>,
    size_bytes: Option<u64>,
    page_count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReaderRenderKey {
    page: u32,
    mode: PreviewMode,
    width: u16,
    height: u16,
}

#[derive(Debug, Clone, Default)]
struct ReaderPanel {
    open: bool,
    book_path: Option<String>,
    book_title: Option<String>,
    page: u32,
    total_pages: Option<u32>,
    scroll: u16,
    current_text: Option<String>,
    last_error: Option<String>,
    notice: Option<String>,
    render_key: Option<ReaderRenderKey>,
}

impl ReaderPanel {
    fn open_book(&mut self, book: &bookshelf_core::Book, ctx: &AppContext, engine: &Engine) {
        self.open = true;
        self.book_path = Some(book.path.clone());
        self.book_title = Some(book.title.clone());
        let saved = ctx.progress_by_path.get(&book.path).copied().unwrap_or(1);
        self.page = saved.saturating_sub(1);
        self.total_pages = engine.page_count(book).ok();
        if let Some(total) = self.total_pages
            && total > 0
        {
            self.page = self.page.min(total.saturating_sub(1));
        }
        self.invalidate_render();
    }

    fn current_book(&self) -> Option<bookshelf_core::Book> {
        Some(bookshelf_core::Book {
            path: self.book_path.clone()?,
            title: self.book_title.clone()?,
            last_opened: None,
        })
    }

    fn invalidate_render(&mut self) {
        self.scroll = 0;
        self.current_text = None;
        self.last_error = None;
        self.notice = None;
        self.render_key = None;
    }

    fn ensure_rendered(&mut self, ctx: &AppContext, engine: &Engine, width: u16, height: u16) {
        let width = width.max(1);
        let height = height.max(1);
        let mode = ctx.settings.preview_mode;

        let Some(book) = self.current_book() else {
            self.current_text = None;
            self.last_error = Some("no book".to_string());
            self.render_key = Some(ReaderRenderKey {
                page: self.page,
                mode,
                width,
                height,
            });
            return;
        };

        let key = ReaderRenderKey {
            page: self.page,
            mode,
            width,
            height,
        };

        if self.current_text.is_some() && self.render_key == Some(key) {
            return;
        }

        match engine.render_page_for_reader(&book, self.page, mode, width, height) {
            Ok(text) => {
                let lines = text.lines().count() as u16;
                if lines == 0 {
                    self.scroll = 0;
                } else {
                    self.scroll = self.scroll.min(lines.saturating_sub(1));
                }
                self.current_text = Some(text);
                self.last_error = None;
            }
            Err(err) => {
                self.current_text = None;
                self.last_error = Some(err.to_string());
            }
        }

        self.render_key = Some(key);
    }

    fn next_page(&mut self) {
        let Some(total) = self.total_pages else {
            self.page = self.page.saturating_add(1);
            self.invalidate_render();
            return;
        };
        if total == 0 {
            return;
        }
        self.page = (self.page + 1).min(total - 1);
        self.invalidate_render();
    }

    fn prev_page(&mut self) {
        self.page = self.page.saturating_sub(1);
        self.invalidate_render();
    }

    fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    fn scroll_down(&mut self) {
        let Some(text) = &self.current_text else {
            return;
        };
        let lines = text.lines().count() as u16;
        self.scroll = (self.scroll + 1).min(lines.saturating_sub(1));
    }
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    terminal::enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("create terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
    terminal::disable_raw_mode().context("disable raw mode")?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("leave alt screen")?;
    Ok(())
}

fn panic_to_string(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        format!("panic: {s}")
    } else if let Some(s) = panic.downcast_ref::<String>() {
        format!("panic: {s}")
    } else {
        "panic: (unknown payload)".to_string()
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn join_roots(settings: &Settings) -> String {
    settings.library_roots.join(";")
}

fn parse_roots_input(input: &str) -> Vec<String> {
    input
        .split([';', ','])
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        let sep_width = if current.is_empty() { 0 } else { 1 };

        if current_width + sep_width + word_width <= max_width {
            if !current.is_empty() {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_width;
            continue;
        }

        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }

        if word_width <= max_width {
            current.push_str(word);
            current_width = word_width;
            continue;
        }

        let mut chunk = String::new();
        let mut chunk_width = 0usize;
        for ch in word.chars() {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            let w = UnicodeWidthStr::width(s);
            if chunk_width + w > max_width && !chunk.is_empty() {
                lines.push(std::mem::take(&mut chunk));
                chunk_width = 0;
            }
            chunk.push(ch);
            chunk_width += w;
        }
        if !chunk.is_empty() {
            lines.push(std::mem::take(&mut chunk));
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn option_chip(label: &str, selected: bool, row_selected: bool) -> Span<'static> {
    let base = if selected && row_selected {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    Span::styled(label.to_string(), base)
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn format_last_opened(last_opened: Option<i64>) -> String {
    let Some(last_opened) = last_opened else {
        return "never".to_string();
    };

    let now = unix_now_secs();
    let delta = now.saturating_sub(last_opened);
    if delta < 10 {
        return "just now".to_string();
    }
    if delta < 60 {
        return format!("{delta}s ago");
    }
    if delta < 60 * 60 {
        return format!("{}m ago", delta / 60);
    }
    if delta < 60 * 60 * 24 {
        return format!("{}h ago", delta / (60 * 60));
    }
    format!("{}d ago", delta / (60 * 60 * 24))
}

fn format_bytes_opt(bytes: Option<u64>) -> String {
    bytes.map(format_bytes).unwrap_or_else(|| "-".to_string())
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
