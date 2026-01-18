//! ratatui-based UI.

use std::collections::VecDeque;
use std::hash::Hasher;
use std::io::{self, Stdout};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use bookshelf_application::{AppContext, CollectionFilter, LabelCatalogOp, TagMatchMode};
use bookshelf_core::{
    Book, BookLabels, Bookmark, KittyImageQuality, Note, ReaderMode, ReaderTextMode, Settings,
    TagKind, Theme, TocItem,
};
use bookshelf_engine::{Engine, PageFurniture};
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
use ratatui_image::picker::{Picker, cap_parser::QueryStdioOptions};
use ratatui_image::protocol::Protocol as ImageProtocol;
use ratatui_image::protocol::kitty::Kitty;
use ratatui_image::{Image as ImageWidget, Resize};

mod image_protocol;
mod kitty_spawn;
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

pub struct Ui {
    ctx: AppContext,
    settings_panel: SettingsPanel,
    scan_panel: ScanPathPanel,
    search_panel: SearchPanel,
    label_manager_panel: LabelManagerPanel,
    assign_labels_panel: AssignLabelsPanel,
    label_catalog_input_panel: LabelCatalogInputPanel,
    goto_panel: GotoPanel,
    bookmarks_panel: BookmarksPanel,
    notes_panel: NotesPanel,
    toc_panel: TocPanel,
    reader: ReaderPanel,
    boot_reader_session: bool,
    ignore_next_esc_quit: bool,
    engine: Engine,
    image_picker: Picker,
    spawned_kitties: Vec<std::process::Child>,
    meta_cache: BookMetaCache,
}

impl Ui {
    pub fn new(mut ctx: AppContext) -> Self {
        ctx.settings.normalize();
        let settings_panel = SettingsPanel::default();
        let scan_panel = ScanPathPanel::new(join_roots(&ctx.settings));
        let search_panel = SearchPanel::default();
        let label_manager_panel = LabelManagerPanel::default();
        let assign_labels_panel = AssignLabelsPanel::default();
        let label_catalog_input_panel = LabelCatalogInputPanel::default();
        let goto_panel = GotoPanel::default();
        let bookmarks_panel = BookmarksPanel::default();
        let notes_panel = NotesPanel::default();
        let toc_panel = TocPanel::default();
        let reader = ReaderPanel::default();
        let meta_cache = BookMetaCache::default();
        let image_picker = Picker::halfblocks();
        let mut ui = Self {
            ctx,
            settings_panel,
            scan_panel,
            search_panel,
            label_manager_panel,
            assign_labels_panel,
            label_catalog_input_panel,
            goto_panel,
            bookmarks_panel,
            notes_panel,
            toc_panel,
            reader,
            boot_reader_session: false,
            ignore_next_esc_quit: false,
            engine: Engine::new(),
            image_picker,
            spawned_kitties: Vec::new(),
            meta_cache,
        };
        ui.bootstrap_reader_from_env();
        ui
    }

    pub fn run(&mut self) -> anyhow::Result<UiOutcome> {
        let mut terminal = setup_terminal()?;
        image_protocol::ensure_tmux_allow_passthrough();
        self.image_picker = if image_protocol::should_query_stdio() {
            let options = QueryStdioOptions {
                timeout: image_protocol::stdio_query_timeout(),
                text_sizing_protocol: false,
            };
            Picker::from_query_stdio_with_options(options).unwrap_or_else(|_| Picker::halfblocks())
        } else {
            Picker::halfblocks()
        };
        self.image_picker
            .set_background_color(image::Rgba([255u8, 255u8, 255u8, 255u8]));
        image_protocol::prefer_kitty_if_supported(&mut self.image_picker);
        terminal.clear().ok();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.event_loop(&mut terminal)
        }));
        let restore_result = restore_terminal(&mut terminal);

        match (result, restore_result) {
            (Ok(Ok(outcome)), Ok(())) => {
                if outcome.exit == UiExit::Quit {
                    self.kill_spawned_kitties();
                }
                Ok(outcome)
            }
            (Ok(Ok(outcome)), Err(err)) => {
                if outcome.exit == UiExit::Quit {
                    self.kill_spawned_kitties();
                }
                Err(err)
            }
            (Ok(Err(err)), Ok(())) => Err(err),
            (Ok(_), Err(err)) => Err(err),
            (Err(panic), Ok(())) => Err(anyhow::anyhow!(panic_to_string(panic))),
            (Err(panic), Err(err)) => Err(anyhow::anyhow!(
                "{}\n(additionally failed to restore terminal: {err})",
                panic_to_string(panic)
            )),
        }
    }

    fn kill_spawned_kitties(&mut self) {
        for mut child in self.spawned_kitties.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn accent_color(&self) -> Color {
        match self.ctx.settings.theme {
            Theme::Light => Color::Blue,
            Theme::Dark => Color::Yellow,
        }
    }

    fn bootstrap_reader_from_env(&mut self) {
        let boot = std::env::var("BOOKSHELF_BOOT_READER")
            .ok()
            .is_some_and(|v| !v.trim().is_empty() && v.trim() != "0");
        if !boot {
            return;
        }
        self.boot_reader_session = true;

        let Some(path) = std::env::var("BOOKSHELF_BOOT_READER_PATH").ok() else {
            return;
        };

        let page_index = std::env::var("BOOKSHELF_BOOT_READER_PAGE_INDEX")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);

        let mode = std::env::var("BOOKSHELF_BOOT_READER_MODE").ok();
        if let Some(mode) = mode {
            if mode.trim().eq_ignore_ascii_case("image") {
                self.ctx.settings.reader_mode = ReaderMode::Image;
            } else if mode.trim().eq_ignore_ascii_case("text") {
                self.ctx.settings.reader_mode = ReaderMode::Text;
            }
        }

        let book = self
            .ctx
            .books
            .iter()
            .find(|b| b.path == path)
            .cloned()
            .unwrap_or_else(|| {
                let decoded = bookshelf_core::decode_path(&path);
                let title = decoded
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "untitled".to_string());
                bookshelf_core::Book {
                    path: path.clone(),
                    title,
                    last_opened: None,
                    favorite: false,
                }
            });

        self.reader.open_book(&book, &self.ctx, &self.engine);
        self.reader.page = page_index;
        if let Some(total) = self.reader.total_pages
            && total > 0
        {
            self.reader.page = self.reader.page.min(total - 1);
        }
        self.reader.invalidate_render();

        // Best effort: clear env so we don't re-bootstrap on subsequent UI restarts.
        unsafe {
            std::env::remove_var("BOOKSHELF_BOOT_READER");
            std::env::remove_var("BOOKSHELF_BOOT_READER_PATH");
            std::env::remove_var("BOOKSHELF_BOOT_READER_PAGE_INDEX");
            std::env::remove_var("BOOKSHELF_BOOT_READER_MODE");
        }
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> anyhow::Result<UiOutcome> {
        let tick_rate = Duration::from_millis(250);
        let mut needs_redraw = true;

        loop {
            if needs_redraw {
                terminal.draw(|frame| self.draw(frame.area(), frame))?;
                needs_redraw = false;
            }

            if !event::poll(tick_rate)? {
                continue;
            }

            match event::read()? {
                Event::Resize(_, _) => {
                    needs_redraw = true;
                }
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }

                    needs_redraw = true;

                    if self.settings_panel.open {
                        if let Some(exit) = self.handle_settings_panel_key(key)? {
                            return Ok(UiOutcome {
                                ctx: self.ctx.clone(),
                                exit,
                            });
                        }
                    } else if self.label_catalog_input_panel.open {
                        if let Some(exit) = self.handle_label_catalog_input_panel_key(key)? {
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
                    } else if self.reader.open && self.goto_panel.open {
                        if let Some(exit) = self.handle_goto_panel_key(key)? {
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
                    } else if let Some(exit) = self.handle_main_key(key)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                if self.boot_reader_session && self.ignore_next_esc_quit {
                    self.ignore_next_esc_quit = false;
                    return Ok(None);
                }
                Ok(Some(UiExit::Quit))
            }
            KeyCode::Char('/') => {
                self.open_filters_panel();
                Ok(None)
            }
            KeyCode::Char('l') => {
                self.open_labels_panel();
                Ok(None)
            }
            KeyCode::Char('c') => {
                self.open_catalog_panel();
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

    fn open_filters_panel(&mut self) {
        self.open_search_panel_mode(SearchPanelMode::Filters);
        self.search_panel.focus = SearchFocus::Query;
    }

    fn open_labels_panel(&mut self) {
        self.open_search_panel_mode(SearchPanelMode::Labels);
    }

    fn open_catalog_panel(&mut self) {
        self.open_search_panel_mode(SearchPanelMode::Catalog);
    }

    fn open_search_panel_mode(&mut self, mode: SearchPanelMode) {
        self.search_panel.open = true;
        self.search_panel.mode = mode;
        self.search_panel.tab = SearchTab::Search;
        self.search_panel.focus = SearchFocus::Query;
        self.search_panel.collection_cursor = 0;
        self.search_panel.tag_cursor = 0;
        self.reset_search_overlay_state();
        self.search_panel.snapshot = Some(SearchSnapshot {
            library_query: self.ctx.library_query.clone(),
            favorites_only: self.ctx.favorites_only,
            collection_filter: self.ctx.collection_filter.clone(),
            tag_filters: self.ctx.tag_filters.clone(),
            tag_match_mode: self.ctx.tag_match_mode,
            selected_path: self
                .ctx
                .books
                .get(self.ctx.selected)
                .map(|b| b.path.clone()),
        });
        self.label_catalog_input_panel.open = false;
        self.settings_panel.open = false;
        self.scan_panel.open = false;
        self.sync_search_panel_tab();
        self.normalize_selection_to_visible();
    }

    fn reset_search_overlay_state(&mut self) {
        self.label_manager_panel.confirm_delete = None;
        self.label_manager_panel.error = None;
        self.label_manager_panel.filter_editing = false;
        self.assign_labels_panel.query_editing = false;
        self.assign_labels_panel.error = None;
        self.assign_labels_panel.book_path = None;
    }

    fn close_search_panel(&mut self) {
        self.search_panel.open = false;
        self.search_panel.snapshot = None;
        self.reset_search_overlay_state();
    }

    fn cancel_search_panel(&mut self) {
        if let Some(snapshot) = self.search_panel.snapshot.take() {
            self.ctx.library_query = snapshot.library_query;
            self.ctx.favorites_only = snapshot.favorites_only;
            self.ctx.collection_filter = snapshot.collection_filter;
            self.ctx.tag_filters = snapshot.tag_filters;
            self.ctx.tag_match_mode = snapshot.tag_match_mode;

            if let Some(path) = snapshot.selected_path {
                if let Some(idx) = self.ctx.books.iter().position(|b| b.path == path) {
                    self.ctx.selected = idx;
                }
            }
            self.normalize_selection_to_visible();
        }
        self.search_panel.open = false;
        self.reset_search_overlay_state();
    }

    fn handle_search_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        if key.code == KeyCode::Esc {
            if self.search_panel.tab == SearchTab::Assign && self.assign_labels_panel.query_editing
            {
                self.assign_labels_panel.query_editing = false;
                return Ok(None);
            }

            if self.search_panel.tab == SearchTab::Manage
                && (self.label_manager_panel.confirm_delete.is_some()
                    || self.label_manager_panel.filter_editing)
            {
                return self.handle_label_manager_panel_key(key);
            }

            self.cancel_search_panel();
            return Ok(None);
        }

        if key.code == KeyCode::Enter {
            if self.search_panel.tab == SearchTab::Assign && self.assign_labels_panel.query_editing
            {
                self.assign_labels_panel.query_editing = false;
                return Ok(None);
            }

            if self.search_panel.tab == SearchTab::Manage && self.label_manager_panel.filter_editing
            {
                self.label_manager_panel.filter_editing = false;
                return Ok(None);
            }

            match self.search_panel.tab {
                SearchTab::Search => {
                    self.close_search_panel();
                }
                SearchTab::Assign => {
                    self.commit_assign_labels_panel();
                    self.close_search_panel();
                }
                SearchTab::Manage => {
                    if self.label_manager_panel.confirm_delete.is_some() {
                        return self.handle_label_manager_panel_key(key);
                    }
                    self.close_search_panel();
                }
            }
            return Ok(None);
        }

        match self.search_panel.tab {
            SearchTab::Search => self.handle_search_filters_tab_key(key),
            SearchTab::Assign => self.handle_assign_labels_panel_key(key),
            SearchTab::Manage => self.handle_label_manager_panel_key(key),
        }
    }

    fn handle_search_filters_tab_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char('u') = key.code
        {
            self.ctx.library_query.clear();
            self.ctx.favorites_only = false;
            self.ctx.collection_filter = CollectionFilter::Any;
            self.ctx.tag_filters.clear();
            self.ctx.tag_match_mode = TagMatchMode::Or;
            self.normalize_selection_to_visible();
            return Ok(None);
        }

        match key.code {
            KeyCode::Tab => {
                self.search_panel.focus = self.search_panel.focus.next();
                Ok(None)
            }
            KeyCode::BackTab => {
                self.search_panel.focus = self.search_panel.focus.prev();
                Ok(None)
            }
            KeyCode::Left => {
                if self.ctx.tag_match_mode == TagMatchMode::And {
                    self.ctx.tag_match_mode = TagMatchMode::Or;
                }
                Ok(None)
            }
            KeyCode::Right => {
                if self.ctx.tag_match_mode == TagMatchMode::Or {
                    self.ctx.tag_match_mode = TagMatchMode::And;
                }
                Ok(None)
            }
            KeyCode::Up => {
                match self.search_panel.focus {
                    SearchFocus::Query => {}
                    SearchFocus::Collections => {
                        self.search_panel.collection_cursor =
                            self.search_panel.collection_cursor.saturating_sub(1);
                    }
                    SearchFocus::Tags => {
                        self.search_panel.tag_cursor =
                            self.search_panel.tag_cursor.saturating_sub(1);
                    }
                }
                Ok(None)
            }
            KeyCode::Down => {
                match self.search_panel.focus {
                    SearchFocus::Query => {}
                    SearchFocus::Collections => {
                        self.search_panel.collection_cursor =
                            self.search_panel.collection_cursor.saturating_add(1);
                    }
                    SearchFocus::Tags => {
                        self.search_panel.tag_cursor =
                            self.search_panel.tag_cursor.saturating_add(1);
                    }
                }
                Ok(None)
            }
            KeyCode::Char('f') => {
                self.ctx.favorites_only = !self.ctx.favorites_only;
                self.normalize_selection_to_visible();
                Ok(None)
            }
            KeyCode::Char(' ') => {
                match self.search_panel.focus {
                    SearchFocus::Query => {
                        self.ctx.library_query.push(' ');
                        self.normalize_selection_to_visible();
                    }
                    SearchFocus::Collections => {
                        self.apply_collection_cursor();
                        self.normalize_selection_to_visible();
                    }
                    SearchFocus::Tags => {
                        self.toggle_tag_cursor();
                        self.normalize_selection_to_visible();
                    }
                }
                Ok(None)
            }
            KeyCode::Backspace => {
                if self.search_panel.focus == SearchFocus::Query {
                    self.ctx.library_query.pop();
                    self.normalize_selection_to_visible();
                }
                Ok(None)
            }
            KeyCode::Char(ch) => {
                if self.search_panel.focus == SearchFocus::Query && !ch.is_control() {
                    self.ctx.library_query.push(ch);
                    self.normalize_selection_to_visible();
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn normalize_tag_filters(&mut self) {
        self.ctx.tag_filters.retain(|t| !t.trim().is_empty());
        self.ctx.tag_filters.sort_by(|a, b| {
            a.to_ascii_lowercase()
                .cmp(&b.to_ascii_lowercase())
                .then_with(|| a.cmp(b))
        });
        self.ctx
            .tag_filters
            .dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    }

    fn all_tag_names(&self) -> Vec<String> {
        let mut tags: Vec<String> = self
            .ctx
            .labels_by_path
            .values()
            .flat_map(|labels| labels.tags.iter().cloned())
            .collect();
        tags.retain(|t| !t.trim().is_empty());
        tags.sort_by(|a, b| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()));
        tags.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        tags
    }

    fn all_collection_names(&self) -> Vec<String> {
        let mut cols: Vec<String> = self
            .ctx
            .labels_by_path
            .values()
            .filter_map(|labels| labels.collection.clone())
            .collect();
        cols.retain(|c| !c.trim().is_empty());
        cols.sort_by(|a, b| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()));
        cols.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        cols
    }

    fn matches_query(&self, book: &Book) -> bool {
        let query = self.ctx.library_query.trim();
        if query.is_empty() {
            return true;
        }
        let query = query.to_ascii_lowercase();

        let title = book.title.to_ascii_lowercase();
        if title.contains(&query) {
            return true;
        }

        let path = bookshelf_core::display_path(&book.path).to_ascii_lowercase();
        path.contains(&query)
    }

    fn matches_favorites_only(&self, book: &Book) -> bool {
        !self.ctx.favorites_only || book.favorite
    }

    fn matches_collection(&self, book: &Book) -> bool {
        let labels = self
            .ctx
            .labels_by_path
            .get(&book.path)
            .cloned()
            .unwrap_or_default();
        matches_collection_filter(&self.ctx.collection_filter, labels.collection.as_deref())
    }

    fn matches_tags(&self, book: &Book) -> bool {
        let labels = self
            .ctx
            .labels_by_path
            .get(&book.path)
            .cloned()
            .unwrap_or_default();
        matches_tag_filter(&self.ctx.tag_filters, self.ctx.tag_match_mode, &labels.tags)
    }

    fn counts_by_collection_for_search(
        &self,
    ) -> (usize, usize, std::collections::HashMap<String, usize>) {
        let mut total = 0usize;
        let mut none_count = 0usize;
        let mut by_name: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for book in &self.ctx.books {
            if !self.matches_query(book) || !self.matches_favorites_only(book) {
                continue;
            }
            if !self.matches_tags(book) {
                continue;
            }
            total += 1;

            let labels = self
                .ctx
                .labels_by_path
                .get(&book.path)
                .cloned()
                .unwrap_or_default();
            if let Some(col) = labels.collection.as_deref() {
                *by_name.entry(col.to_string()).or_insert(0) += 1;
            } else {
                none_count += 1;
            }
        }

        (total, none_count, by_name)
    }

    fn counts_by_tag_for_search(&self) -> (usize, std::collections::HashMap<String, usize>) {
        let mut total = 0usize;
        let mut by_name: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for book in &self.ctx.books {
            if !self.matches_query(book) || !self.matches_favorites_only(book) {
                continue;
            }
            if !self.matches_collection(book) {
                continue;
            }
            total += 1;

            let labels = self
                .ctx
                .labels_by_path
                .get(&book.path)
                .cloned()
                .unwrap_or_default();
            for tag in labels.tags {
                if tag.trim().is_empty() {
                    continue;
                }
                *by_name.entry(tag).or_insert(0) += 1;
            }
        }

        (total, by_name)
    }

    fn collection_entries_for_search(&self) -> Vec<CollectionEntry> {
        let (total, none_count, counts) = self.counts_by_collection_for_search();
        let mut out = Vec::new();
        out.push(CollectionEntry {
            label: "Any".to_string(),
            filter: CollectionFilter::Any,
            count: total,
        });
        out.push(CollectionEntry {
            label: "None".to_string(),
            filter: CollectionFilter::None,
            count: none_count,
        });

        for name in self.all_collection_names() {
            let count = counts
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&name))
                .map(|(_, v)| *v)
                .unwrap_or(0);
            out.push(CollectionEntry {
                label: name.clone(),
                filter: CollectionFilter::Selected(name),
                count,
            });
        }
        out
    }

    fn tag_entries_for_search(&self) -> Vec<TagEntry> {
        let (_total, counts) = self.counts_by_tag_for_search();
        let mut out = Vec::new();
        for name in self.all_tag_names() {
            let count = counts
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&name))
                .map(|(_, v)| *v)
                .unwrap_or(0);
            out.push(TagEntry { name, count });
        }
        out
    }

    fn apply_collection_cursor(&mut self) {
        let entries = self.collection_entries_for_search();
        if entries.is_empty() {
            return;
        }
        self.search_panel.collection_cursor = self
            .search_panel
            .collection_cursor
            .min(entries.len().saturating_sub(1));
        let entry = &entries[self.search_panel.collection_cursor];
        self.ctx.collection_filter = entry.filter.clone();
        self.normalize_selection_to_visible();
    }

    fn toggle_tag_cursor(&mut self) {
        let entries = self.tag_entries_for_search();
        if entries.is_empty() {
            return;
        }
        self.search_panel.tag_cursor = self
            .search_panel
            .tag_cursor
            .min(entries.len().saturating_sub(1));
        let tag = &entries[self.search_panel.tag_cursor].name;

        if let Some(pos) = self
            .ctx
            .tag_filters
            .iter()
            .position(|t| t.eq_ignore_ascii_case(tag))
        {
            self.ctx.tag_filters.remove(pos);
        } else {
            self.ctx.tag_filters.push(tag.clone());
        }
        self.normalize_tag_filters();
        self.normalize_selection_to_visible();
    }

    fn handle_reader_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                if let Some(path) = self.reader.book_path.clone() {
                    self.ctx
                        .progress_by_path
                        .insert(path, self.reader.page.saturating_add(1));
                }
                if self.boot_reader_session {
                    return Ok(Some(UiExit::Quit));
                }
                self.reader = ReaderPanel::default();
                self.goto_panel = GotoPanel::default();
                self.bookmarks_panel = BookmarksPanel::default();
                self.notes_panel = NotesPanel::default();
                self.toc_panel = TocPanel::default();
                Ok(None)
            }
            KeyCode::Char('g') => {
                self.goto_panel.open = true;
                self.goto_panel.error = None;
                self.goto_panel.input = self.reader.page.saturating_add(1).to_string();
                self.bookmarks_panel.open = false;
                self.notes_panel.open = false;
                self.toc_panel.open = false;
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
                    let term = std::env::var("TERM").unwrap_or_default();
                    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
                    let tmux = std::env::var("TMUX").unwrap_or_default();
                    let kitty_window_id = std::env::var("KITTY_WINDOW_ID").unwrap_or_default();
                    let protocol_label = image_protocol::protocol_label(&self.image_picker);
                    let image_supported = image_protocol::image_supported(&self.image_picker);
                    let kitty_supported = image_protocol::kitty_supported(&self.image_picker);
                    let capabilities = self.image_picker.capabilities();
                    let (font_w, font_h) = self.image_picker.font_size();
                    let timing_block = self.reader.last_image_timings.map(|t| {
                        let rasterize_ms = t
                            .rasterize_ms
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".to_string());
                        format!(
                            "\nreader-image:\n  total_ms={}\n  rasterize_ms={}\n  viewport_ms={}\n  downscale_ms={}\n  protocol_ms={}\n  viewport_px={}x{}\n  transmit_px={}x{}\n  render_width_px={}\n",
                            t.total_ms,
                            rasterize_ms,
                            t.viewport_ms,
                            t.downscale_ms,
                            t.protocol_ms,
                            t.viewport_px.0,
                            t.viewport_px.1,
                            t.transmit_px.0,
                            t.transmit_px.1,
                            t.render_width_px,
                        )
                    });
                    let debug = format!(
                        "env:\n  TERM={term}\n  TERM_PROGRAM={term_program}\n  TMUX={tmux}\n  KITTY_WINDOW_ID={kitty_window_id}\n\nratatui-image:\n  protocol={protocol_label}\n  image_supported={image_supported}\n  kitty_supported={kitty_supported}\n  capabilities={capabilities:?}\n  font_size_px={font_w}x{font_h}{}\n\n-----\n\n{}",
                        timing_block.unwrap_or_default(),
                        self.engine.debug_page_text(&book, self.reader.page)?
                    );
                    std::fs::write(&path, debug)?;
                    self.reader.notice = Some(format!("wrote {}", path.display()));
                }
                Ok(None)
            }
            KeyCode::Char('b') => {
                self.bookmarks_panel.open = true;
                self.bookmarks_panel.selected = 0;
                self.goto_panel.open = false;
                self.notes_panel.open = false;
                self.toc_panel.open = false;
                Ok(None)
            }
            KeyCode::Char('n') => {
                self.notes_panel.open = true;
                self.notes_panel.selected = 0;
                self.notes_panel.error = None;
                self.goto_panel.open = false;
                self.bookmarks_panel.open = false;
                self.toc_panel.open = false;
                Ok(None)
            }
            KeyCode::Char('t') => {
                self.open_toc_panel();
                Ok(None)
            }
            KeyCode::Char('m') => {
                match self.ctx.settings.reader_mode {
                    ReaderMode::Text => {
                        if image_protocol::image_supported(&self.image_picker) {
                            image_protocol::prefer_kitty_if_supported(&mut self.image_picker);
                            self.ctx.settings.reader_mode = ReaderMode::Image;
                            self.reader.invalidate_render();
                            let label = image_protocol::protocol_label(&self.image_picker);
                            let mut notice = format!("mode: image ({label})");
                            if image_protocol::in_iterm_env() && label == "iterm2" {
                                notice.push_str("; enable iTerm2 inline images (imgcat) if blank");
                            }
                            self.reader.notice = Some(notice);
                        } else {
                            self.ctx.settings.reader_mode = ReaderMode::Text;
                            let in_tmux = std::env::var_os("TMUX").is_some();
                            self.reader.notice = Some(if in_tmux {
                                "image mode needs a graphics protocol + tmux allow-passthrough; press k to open kitty reader"
                                    .to_string()
                            } else {
                                "image mode requires kitty/iterm2 graphics; press k to open kitty reader"
                                    .to_string()
                            });
                        }
                    }
                    ReaderMode::Image => {
                        self.ctx.settings.reader_mode = ReaderMode::Text;
                        self.reader.invalidate_render();
                        self.reader.notice = Some("mode: text".to_string());
                    }
                }
                Ok(None)
            }
            KeyCode::Char('r') => {
                if self.ctx.settings.reader_mode == ReaderMode::Text {
                    self.ctx.settings.cycle_reader_text_mode();
                    self.reader.invalidate_render();
                    self.reader.notice =
                        Some(format!("text: {}", self.ctx.settings.reader_text_mode));
                }
                Ok(None)
            }
            KeyCode::Char('h') => {
                if self.ctx.settings.reader_mode == ReaderMode::Text {
                    self.ctx.settings.toggle_reader_trim_headers_footers();
                    self.reader.invalidate_render();
                    let status = if self.ctx.settings.reader_trim_headers_footers {
                        "on"
                    } else {
                        "off"
                    };
                    self.reader.notice = Some(format!("trim headers/footers: {status}"));
                }
                Ok(None)
            }
            KeyCode::Left => {
                if self.ctx.settings.reader_mode == ReaderMode::Image
                    && key.modifiers.contains(KeyModifiers::SHIFT)
                {
                    self.reader.pan_image_by_cells(&self.image_picker, -5, 0);
                } else {
                    self.reader.prev_page();
                }
                Ok(None)
            }
            KeyCode::Right => {
                if self.ctx.settings.reader_mode == ReaderMode::Image
                    && key.modifiers.contains(KeyModifiers::SHIFT)
                {
                    self.reader.pan_image_by_cells(&self.image_picker, 5, 0);
                } else {
                    self.reader.next_page();
                }
                Ok(None)
            }
            KeyCode::Up => {
                if self.ctx.settings.reader_mode == ReaderMode::Image {
                    self.reader.pan_image_by_cells(&self.image_picker, 0, -3);
                } else {
                    self.reader.scroll_up();
                }
                Ok(None)
            }
            KeyCode::Down => {
                if self.ctx.settings.reader_mode == ReaderMode::Image {
                    self.reader.pan_image_by_cells(&self.image_picker, 0, 3);
                } else {
                    self.reader.scroll_down();
                }
                Ok(None)
            }
            KeyCode::PageUp => {
                if self.ctx.settings.reader_mode == ReaderMode::Image {
                    let step = self
                        .reader
                        .render_key
                        .map(|k| k.height.saturating_sub(2))
                        .unwrap_or(10);
                    self.reader
                        .pan_image_by_cells(&self.image_picker, 0, -i32::from(step));
                } else {
                    for _ in 0..10 {
                        self.reader.scroll_up();
                    }
                }
                Ok(None)
            }
            KeyCode::PageDown => {
                if self.ctx.settings.reader_mode == ReaderMode::Image {
                    let step = self
                        .reader
                        .render_key
                        .map(|k| k.height.saturating_sub(2))
                        .unwrap_or(10);
                    self.reader
                        .pan_image_by_cells(&self.image_picker, 0, i32::from(step));
                } else {
                    for _ in 0..10 {
                        self.reader.scroll_down();
                    }
                }
                Ok(None)
            }
            KeyCode::Char('k') => {
                if self.ctx.settings.reader_mode == ReaderMode::Text
                    && !image_protocol::image_supported(&self.image_picker)
                {
                    let spawned = if let Some(path) = self.reader.book_path.as_deref() {
                        kitty_spawn::spawn_kitty_reader_with_current_exe(path, self.reader.page)
                    } else {
                        kitty_spawn::spawn_kitty_with_current_exe()
                    };
                    match spawned {
                        Ok(child) => {
                            self.spawned_kitties.push(child);
                            self.reader.notice = Some("spawned kitty reader".to_string());
                        }
                        Err(err) => self.reader.notice = Some(format!("kitty spawn failed: {err}")),
                    }
                }
                Ok(None)
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                if self.ctx.settings.reader_mode == ReaderMode::Image {
                    self.reader.zoom_image_in();
                }
                Ok(None)
            }
            KeyCode::Char('-') => {
                if self.ctx.settings.reader_mode == ReaderMode::Image {
                    self.reader.zoom_image_out();
                }
                Ok(None)
            }
            KeyCode::Char('0') => {
                if self.ctx.settings.reader_mode == ReaderMode::Image {
                    self.reader.reset_image_view();
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_goto_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => {
                self.goto_panel.open = false;
                self.goto_panel.input.clear();
                self.goto_panel.error = None;
                Ok(None)
            }
            KeyCode::Enter => {
                let input = self.goto_panel.input.trim();
                if input.is_empty() {
                    self.goto_panel.error = Some("Enter a page number".to_string());
                    return Ok(None);
                }

                let page = match input.parse::<u32>() {
                    Ok(p) if p >= 1 => p,
                    _ => {
                        self.goto_panel.error = Some("Invalid page number".to_string());
                        return Ok(None);
                    }
                };

                if let Some(total) = self.reader.total_pages
                    && page > total
                {
                    self.goto_panel.error = Some(format!("Page out of range (1..={total})"));
                    return Ok(None);
                }

                self.reader.page = page.saturating_sub(1);
                self.reader.invalidate_render();
                self.reader.notice = Some(format!("jumped to page {page}"));
                self.goto_panel.open = false;
                self.goto_panel.error = None;
                Ok(None)
            }
            KeyCode::Backspace => {
                self.goto_panel.input.pop();
                Ok(None)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.goto_panel.input.clear();
                Ok(None)
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                self.goto_panel.input.push(ch);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn open_toc_panel(&mut self) {
        self.toc_panel.open = true;
        self.toc_panel.error = None;
        self.toc_panel.query.clear();
        self.goto_panel.open = false;
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
        let visible = self.toc_visible_indices();
        self.toc_panel.selected = visible.iter().position(|idx| *idx == best).unwrap_or(0);
    }

    fn toc_visible_indices(&self) -> Vec<usize> {
        let query = self.toc_panel.query.trim().to_lowercase();
        if query.is_empty() {
            return (0..self.toc_panel.items.len()).collect();
        }

        self.toc_panel
            .items
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| {
                if item.title.to_lowercase().contains(&query) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    fn handle_toc_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        let visible = self.toc_visible_indices();
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
                let len = visible.len();
                if len > 0 {
                    self.toc_panel.selected = (self.toc_panel.selected + 1).min(len - 1);
                }
                Ok(None)
            }
            KeyCode::Enter => {
                let Some(item_idx) = visible.get(self.toc_panel.selected).copied() else {
                    return Ok(None);
                };
                let Some(item) = self.toc_panel.items.get(item_idx) else {
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
            KeyCode::Backspace => {
                self.toc_panel.query.pop();
                self.toc_panel.selected = 0;
                Ok(None)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.toc_panel.query.clear();
                self.toc_panel.selected = 0;
                Ok(None)
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
                    self.toc_panel.query.push(ch);
                    self.toc_panel.selected = 0;
                }
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
                self.settings_panel.selected = (self.settings_panel.selected + 1)
                    .min(SETTINGS_MENU_ITEM_COUNT.saturating_sub(1));
                Ok(None)
            }
            KeyCode::Left => {
                if self.settings_panel.selected == SETTINGS_MENU_KITTY_IMAGE_QUALITY {
                    self.ctx.settings.cycle_kitty_image_quality_prev();
                }
                if self.settings_panel.selected == SETTINGS_MENU_THEME {
                    self.ctx.settings.cycle_theme();
                }
                Ok(None)
            }
            KeyCode::Right => {
                if self.settings_panel.selected == SETTINGS_MENU_KITTY_IMAGE_QUALITY {
                    self.ctx.settings.cycle_kitty_image_quality_next();
                }
                if self.settings_panel.selected == SETTINGS_MENU_THEME {
                    self.ctx.settings.cycle_theme();
                }
                Ok(None)
            }
            KeyCode::Enter => {
                match self.settings_panel.selected {
                    SETTINGS_MENU_SCAN_PATHS => {
                        self.scan_panel.open = true;
                        self.scan_panel.selected = 0;
                        self.scan_panel.input = join_roots(&self.ctx.settings);
                        self.scan_panel.error = None;
                        self.settings_panel.open = false;
                    }
                    SETTINGS_MENU_KITTY_IMAGE_QUALITY => {
                        self.ctx.settings.cycle_kitty_image_quality_next();
                    }
                    SETTINGS_MENU_THEME => {
                        self.ctx.settings.cycle_theme();
                    }
                    _ => {}
                }
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

    fn selected_book_path(&self) -> Option<String> {
        self.selected_visible_index()
            .and_then(|idx| self.ctx.books.get(idx))
            .map(|b| b.path.clone())
    }

    fn sync_search_panel_tab(&mut self) {
        let tab = match self.search_panel.mode {
            SearchPanelMode::Filters => SearchTab::Search,
            SearchPanelMode::Labels => SearchTab::Assign,
            SearchPanelMode::Catalog => SearchTab::Manage,
        };

        if self.search_panel.tab != tab {
            self.label_manager_panel.confirm_delete = None;
            self.label_manager_panel.error = None;
            self.label_manager_panel.filter_editing = false;
            self.assign_labels_panel.query_editing = false;
            self.assign_labels_panel.error = None;

            self.search_panel.tab = tab;
        }

        if tab == SearchTab::Assign {
            let selected_path = self.selected_book_path();
            if selected_path != self.assign_labels_panel.book_path {
                self.prepare_assign_labels_panel();
            }
        }
    }

    fn prepare_assign_labels_panel(&mut self) {
        self.assign_labels_panel.focus = AssignFocus::Collections;
        self.assign_labels_panel.collection_cursor = 0;
        self.assign_labels_panel.tag_cursor = 0;
        self.assign_labels_panel.collection_query.clear();
        self.assign_labels_panel.tag_query.clear();
        self.assign_labels_panel.query_editing = false;
        self.assign_labels_panel.error = None;

        if let Some(path) = self.selected_book_path()
            && let Some(book) = self.ctx.books.iter().find(|b| b.path == path)
        {
            let mut labels = self
                .ctx
                .labels_by_path
                .get(&path)
                .cloned()
                .unwrap_or_default();
            labels.normalize();
            self.assign_labels_panel.book_path = Some(path);
            self.assign_labels_panel.staged_favorite = book.favorite;
            self.assign_labels_panel.staged = labels;
        } else {
            self.assign_labels_panel.book_path = None;
            self.assign_labels_panel.staged_favorite = false;
            self.assign_labels_panel.staged = BookLabels::default();
            self.assign_labels_panel.error = Some("No selection".to_string());
        }
    }

    fn open_label_catalog_input_panel(
        &mut self,
        mode: LabelCatalogInputMode,
        kind: TagKind,
        from: Option<String>,
        prefill: String,
    ) {
        self.label_catalog_input_panel.open = true;
        self.label_catalog_input_panel.mode = mode;
        self.label_catalog_input_panel.kind = kind;
        self.label_catalog_input_panel.from = from;
        self.label_catalog_input_panel.input = prefill;
        self.label_catalog_input_panel.error = None;
        self.settings_panel.open = false;
        self.scan_panel.open = false;
        self.normalize_selection_to_visible();
    }

    fn label_manager_entries(&self, tab: LabelManagerTab) -> Vec<(String, usize)> {
        let query = self.label_manager_panel.filter.trim().to_ascii_lowercase();

        let mut collection_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut tag_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for labels in self.ctx.labels_by_path.values() {
            if let Some(col) = labels.collection.as_deref() {
                let col = col.trim();
                if !col.is_empty() {
                    *collection_counts
                        .entry(col.to_ascii_lowercase())
                        .or_insert(0) += 1;
                }
            }
            for tag in &labels.tags {
                let tag = tag.trim();
                if tag.is_empty() {
                    continue;
                }
                *tag_counts.entry(tag.to_ascii_lowercase()).or_insert(0) += 1;
            }
        }

        let names: &[String] = match tab {
            LabelManagerTab::Collections => &self.ctx.known_collections,
            LabelManagerTab::Tags => &self.ctx.known_tags,
        };

        let mut out = Vec::new();
        for name in names {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !query.is_empty() && !trimmed.to_ascii_lowercase().contains(&query) {
                continue;
            }

            let count = match tab {
                LabelManagerTab::Collections => collection_counts
                    .get(&trimmed.to_ascii_lowercase())
                    .copied()
                    .unwrap_or(0),
                LabelManagerTab::Tags => tag_counts
                    .get(&trimmed.to_ascii_lowercase())
                    .copied()
                    .unwrap_or(0),
            };
            out.push((trimmed.to_string(), count));
        }
        out
    }

    fn label_manager_selected_target(&self) -> Option<LabelDeleteTarget> {
        let tab = self.label_manager_panel.tab;
        let entries = self.label_manager_entries(tab);
        let cursor = match tab {
            LabelManagerTab::Collections => self.label_manager_panel.collections_cursor,
            LabelManagerTab::Tags => self.label_manager_panel.tags_cursor,
        };
        let (name, _count) = entries.get(cursor).cloned()?;
        let kind = match tab {
            LabelManagerTab::Collections => TagKind::Collection,
            LabelManagerTab::Tags => TagKind::Tag,
        };
        Some(LabelDeleteTarget { kind, name })
    }

    fn handle_label_manager_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        if let Some(target) = self.label_manager_panel.confirm_delete.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.apply_delete_label(&target);
                    self.label_manager_panel.confirm_delete = None;
                    self.label_manager_panel.error = None;
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.label_manager_panel.confirm_delete = None;
                }
                _ => {}
            }
            return Ok(None);
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char('u') = key.code
        {
            self.label_manager_panel.filter.clear();
            self.label_manager_panel.filter_editing = false;
            self.label_manager_panel.error = None;
            return Ok(None);
        }

        if self.label_manager_panel.filter_editing {
            match key.code {
                KeyCode::Esc => {
                    self.label_manager_panel.filter_editing = false;
                }
                KeyCode::Backspace => {
                    self.label_manager_panel.filter.pop();
                }
                KeyCode::Tab => {
                    self.label_manager_panel.filter_editing = false;
                    self.label_manager_panel.tab = match self.label_manager_panel.tab {
                        LabelManagerTab::Collections => LabelManagerTab::Tags,
                        LabelManagerTab::Tags => LabelManagerTab::Collections,
                    };
                }
                KeyCode::Char(ch) => {
                    if !ch.is_control() {
                        self.label_manager_panel.filter.push(ch);
                    }
                }
                _ => {}
            }

            let entries = self.label_manager_entries(self.label_manager_panel.tab);
            let cursor = match self.label_manager_panel.tab {
                LabelManagerTab::Collections => &mut self.label_manager_panel.collections_cursor,
                LabelManagerTab::Tags => &mut self.label_manager_panel.tags_cursor,
            };
            if entries.is_empty() {
                *cursor = 0;
            } else {
                *cursor = (*cursor).min(entries.len().saturating_sub(1));
            }
            return Ok(None);
        }

        match key.code {
            KeyCode::Tab => {
                self.label_manager_panel.tab = match self.label_manager_panel.tab {
                    LabelManagerTab::Collections => LabelManagerTab::Tags,
                    LabelManagerTab::Tags => LabelManagerTab::Collections,
                };
                Ok(None)
            }
            KeyCode::Char('/') => {
                self.label_manager_panel.filter_editing = true;
                Ok(None)
            }
            KeyCode::Up => {
                let cursor = match self.label_manager_panel.tab {
                    LabelManagerTab::Collections => {
                        &mut self.label_manager_panel.collections_cursor
                    }
                    LabelManagerTab::Tags => &mut self.label_manager_panel.tags_cursor,
                };
                *cursor = cursor.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Down => {
                let entries = self.label_manager_entries(self.label_manager_panel.tab);
                let cursor = match self.label_manager_panel.tab {
                    LabelManagerTab::Collections => {
                        &mut self.label_manager_panel.collections_cursor
                    }
                    LabelManagerTab::Tags => &mut self.label_manager_panel.tags_cursor,
                };
                if !entries.is_empty() {
                    *cursor = (*cursor).saturating_add(1).min(entries.len() - 1);
                }
                Ok(None)
            }
            KeyCode::Char('n') => {
                let kind = match self.label_manager_panel.tab {
                    LabelManagerTab::Collections => TagKind::Collection,
                    LabelManagerTab::Tags => TagKind::Tag,
                };
                let prefill = self.label_manager_panel.filter.clone();
                self.open_label_catalog_input_panel(
                    LabelCatalogInputMode::Create,
                    kind,
                    None,
                    prefill,
                );
                Ok(None)
            }
            KeyCode::Char('r') => {
                let Some(target) = self.label_manager_selected_target() else {
                    self.label_manager_panel.error = Some("Nothing selected".to_string());
                    return Ok(None);
                };
                self.open_label_catalog_input_panel(
                    LabelCatalogInputMode::Rename,
                    target.kind,
                    Some(target.name.clone()),
                    target.name,
                );
                Ok(None)
            }
            KeyCode::Char('d') => {
                let Some(target) = self.label_manager_selected_target() else {
                    self.label_manager_panel.error = Some("Nothing selected".to_string());
                    return Ok(None);
                };
                self.label_manager_panel.confirm_delete = Some(target);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn assign_visible_collections(&self) -> Vec<String> {
        let query = self
            .assign_labels_panel
            .collection_query
            .trim()
            .to_ascii_lowercase();
        self.ctx
            .known_collections
            .iter()
            .filter(|c| {
                let c = c.trim();
                !c.is_empty() && (query.is_empty() || c.to_ascii_lowercase().contains(&query))
            })
            .cloned()
            .collect()
    }

    fn assign_visible_tags(&self) -> Vec<String> {
        let query = self
            .assign_labels_panel
            .tag_query
            .trim()
            .to_ascii_lowercase();
        self.ctx
            .known_tags
            .iter()
            .filter(|t| {
                let t = t.trim();
                !t.is_empty() && (query.is_empty() || t.to_ascii_lowercase().contains(&query))
            })
            .cloned()
            .collect()
    }

    fn commit_assign_labels_panel(&mut self) {
        let Some(path) = self.assign_labels_panel.book_path.clone() else {
            return;
        };

        if let Some(book) = self.ctx.books.iter_mut().find(|b| b.path == path) {
            if book.favorite != self.assign_labels_panel.staged_favorite {
                book.favorite = self.assign_labels_panel.staged_favorite;
                self.ctx.dirty_favorite_paths.insert(path.clone());
            }
        }

        let mut labels = self.assign_labels_panel.staged.clone();
        labels.normalize();
        self.ctx.ensure_known_labels(&labels);
        if labels.tags.is_empty() && labels.collection.is_none() {
            self.ctx.labels_by_path.remove(&path);
        } else {
            self.ctx.labels_by_path.insert(path.clone(), labels);
        }
        self.ctx.dirty_label_paths.insert(path);

        self.assign_labels_panel.query_editing = false;
        self.assign_labels_panel.error = None;
        self.normalize_selection_to_visible();
    }

    fn handle_assign_labels_panel_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        if self.assign_labels_panel.book_path.is_none() {
            return Ok(None);
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char('u') = key.code
        {
            match self.assign_labels_panel.focus {
                AssignFocus::Collections => self.assign_labels_panel.collection_query.clear(),
                AssignFocus::Tags => self.assign_labels_panel.tag_query.clear(),
            }
            return Ok(None);
        }

        if self.assign_labels_panel.query_editing {
            match key.code {
                KeyCode::Esc => {
                    self.assign_labels_panel.query_editing = false;
                }
                KeyCode::Enter => {
                    self.commit_assign_labels_panel();
                }
                KeyCode::Backspace => match self.assign_labels_panel.focus {
                    AssignFocus::Collections => {
                        self.assign_labels_panel.collection_query.pop();
                    }
                    AssignFocus::Tags => {
                        self.assign_labels_panel.tag_query.pop();
                    }
                },
                KeyCode::Tab => {
                    self.assign_labels_panel.query_editing = false;
                    self.assign_labels_panel.focus = match self.assign_labels_panel.focus {
                        AssignFocus::Collections => AssignFocus::Tags,
                        AssignFocus::Tags => AssignFocus::Collections,
                    };
                }
                KeyCode::Char(ch) => {
                    if !ch.is_control() {
                        match self.assign_labels_panel.focus {
                            AssignFocus::Collections => {
                                self.assign_labels_panel.collection_query.push(ch);
                            }
                            AssignFocus::Tags => {
                                self.assign_labels_panel.tag_query.push(ch);
                            }
                        }
                    }
                }
                _ => {}
            }

            let collections = self.assign_visible_collections();
            let tags = self.assign_visible_tags();
            self.assign_labels_panel.collection_cursor = self
                .assign_labels_panel
                .collection_cursor
                .min(collections.len()); // +1 for "(none)"
            if tags.is_empty() {
                self.assign_labels_panel.tag_cursor = 0;
            } else {
                self.assign_labels_panel.tag_cursor =
                    self.assign_labels_panel.tag_cursor.min(tags.len() - 1);
            }
            return Ok(None);
        }

        match key.code {
            KeyCode::Tab => {
                self.assign_labels_panel.focus = match self.assign_labels_panel.focus {
                    AssignFocus::Collections => AssignFocus::Tags,
                    AssignFocus::Tags => AssignFocus::Collections,
                };
                Ok(None)
            }
            KeyCode::Char('/') => {
                self.assign_labels_panel.query_editing = true;
                Ok(None)
            }
            KeyCode::Char('f') => {
                self.assign_labels_panel.staged_favorite =
                    !self.assign_labels_panel.staged_favorite;
                Ok(None)
            }
            KeyCode::Up => {
                match self.assign_labels_panel.focus {
                    AssignFocus::Collections => {
                        self.assign_labels_panel.collection_cursor =
                            self.assign_labels_panel.collection_cursor.saturating_sub(1);
                    }
                    AssignFocus::Tags => {
                        self.assign_labels_panel.tag_cursor =
                            self.assign_labels_panel.tag_cursor.saturating_sub(1);
                    }
                }
                Ok(None)
            }
            KeyCode::Down => {
                match self.assign_labels_panel.focus {
                    AssignFocus::Collections => {
                        let collections = self.assign_visible_collections();
                        let max = collections.len() + 1;
                        if max > 0 {
                            self.assign_labels_panel.collection_cursor = self
                                .assign_labels_panel
                                .collection_cursor
                                .saturating_add(1)
                                .min(max - 1);
                        }
                    }
                    AssignFocus::Tags => {
                        let tags = self.assign_visible_tags();
                        if !tags.is_empty() {
                            self.assign_labels_panel.tag_cursor = self
                                .assign_labels_panel
                                .tag_cursor
                                .saturating_add(1)
                                .min(tags.len() - 1);
                        }
                    }
                }
                Ok(None)
            }
            KeyCode::Char(' ') => {
                match self.assign_labels_panel.focus {
                    AssignFocus::Collections => {
                        let collections = self.assign_visible_collections();
                        if self.assign_labels_panel.collection_cursor == 0 {
                            self.assign_labels_panel.staged.collection = None;
                        } else if let Some(name) = collections
                            .get(self.assign_labels_panel.collection_cursor - 1)
                            .cloned()
                        {
                            self.assign_labels_panel.staged.collection = Some(name);
                        }
                    }
                    AssignFocus::Tags => {
                        let tags = self.assign_visible_tags();
                        let Some(name) = tags.get(self.assign_labels_panel.tag_cursor).cloned()
                        else {
                            return Ok(None);
                        };
                        if let Some(pos) = self
                            .assign_labels_panel
                            .staged
                            .tags
                            .iter()
                            .position(|t| t.eq_ignore_ascii_case(&name))
                        {
                            self.assign_labels_panel.staged.tags.remove(pos);
                        } else {
                            self.assign_labels_panel.staged.tags.push(name);
                        }
                        self.assign_labels_panel.staged.normalize();
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn remove_name_case_insensitive(values: &mut Vec<String>, name: &str) {
        values.retain(|v| !v.eq_ignore_ascii_case(name));
    }

    fn apply_delete_label(&mut self, target: &LabelDeleteTarget) {
        self.ctx
            .dirty_label_catalog_ops
            .push(LabelCatalogOp::Delete {
                kind: target.kind,
                name: target.name.clone(),
            });
        match target.kind {
            TagKind::Tag => {
                Self::remove_name_case_insensitive(&mut self.ctx.known_tags, &target.name);
                self.ctx
                    .tag_filters
                    .retain(|t| !t.eq_ignore_ascii_case(&target.name));
                self.normalize_tag_filters();
            }
            TagKind::Collection => {
                Self::remove_name_case_insensitive(&mut self.ctx.known_collections, &target.name);
                if let CollectionFilter::Selected(selected) = &self.ctx.collection_filter
                    && selected.eq_ignore_ascii_case(&target.name)
                {
                    self.ctx.collection_filter = CollectionFilter::Any;
                }
            }
        }
        self.ctx.normalize_label_catalog();

        let mut to_remove = Vec::new();
        for (path, labels) in self.ctx.labels_by_path.iter_mut() {
            match target.kind {
                TagKind::Tag => {
                    labels
                        .tags
                        .retain(|t| !t.eq_ignore_ascii_case(&target.name));
                }
                TagKind::Collection => {
                    if labels
                        .collection
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(&target.name))
                    {
                        labels.collection = None;
                    }
                }
            }
            labels.normalize();
            if labels.tags.is_empty() && labels.collection.is_none() {
                to_remove.push(path.clone());
            }
        }
        for path in to_remove {
            self.ctx.labels_by_path.remove(&path);
        }

        match target.kind {
            TagKind::Tag => {
                self.assign_labels_panel
                    .staged
                    .tags
                    .retain(|t| !t.eq_ignore_ascii_case(&target.name));
                self.assign_labels_panel.staged.normalize();
            }
            TagKind::Collection => {
                if self
                    .assign_labels_panel
                    .staged
                    .collection
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case(&target.name))
                {
                    self.assign_labels_panel.staged.collection = None;
                }
            }
        }
    }

    fn apply_rename_label(&mut self, kind: TagKind, from: &str, to: &str) -> bool {
        let from = from.trim();
        let to = to.trim();
        if from.is_empty() || to.is_empty() {
            return false;
        }

        let list = match kind {
            TagKind::Tag => &mut self.ctx.known_tags,
            TagKind::Collection => &mut self.ctx.known_collections,
        };
        let Some(pos) = list.iter().position(|v| v.eq_ignore_ascii_case(from)) else {
            return false;
        };
        list[pos] = to.to_string();
        self.ctx.normalize_label_catalog();

        match kind {
            TagKind::Tag => {
                for tag in &mut self.ctx.tag_filters {
                    if tag.eq_ignore_ascii_case(from) {
                        *tag = to.to_string();
                    }
                }
                self.normalize_tag_filters();
            }
            TagKind::Collection => {
                if let CollectionFilter::Selected(selected) = &mut self.ctx.collection_filter
                    && selected.eq_ignore_ascii_case(from)
                {
                    *selected = to.to_string();
                }
            }
        }

        let mut to_remove = Vec::new();
        for (path, labels) in self.ctx.labels_by_path.iter_mut() {
            match kind {
                TagKind::Tag => {
                    for tag in &mut labels.tags {
                        if tag.eq_ignore_ascii_case(from) {
                            *tag = to.to_string();
                        }
                    }
                }
                TagKind::Collection => {
                    if labels
                        .collection
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(from))
                    {
                        labels.collection = Some(to.to_string());
                    }
                }
            }
            labels.normalize();
            if labels.tags.is_empty() && labels.collection.is_none() {
                to_remove.push(path.clone());
            }
        }
        for path in to_remove {
            self.ctx.labels_by_path.remove(&path);
        }

        match kind {
            TagKind::Tag => {
                for tag in &mut self.assign_labels_panel.staged.tags {
                    if tag.eq_ignore_ascii_case(from) {
                        *tag = to.to_string();
                    }
                }
                self.assign_labels_panel.staged.normalize();
            }
            TagKind::Collection => {
                if self
                    .assign_labels_panel
                    .staged
                    .collection
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case(from))
                {
                    self.assign_labels_panel.staged.collection = Some(to.to_string());
                }
            }
        }
        true
    }

    fn handle_label_catalog_input_panel_key(
        &mut self,
        key: KeyEvent,
    ) -> anyhow::Result<Option<UiExit>> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char('u') = key.code
        {
            self.label_catalog_input_panel.input.clear();
            return Ok(None);
        }

        match key.code {
            KeyCode::Esc => {
                self.label_catalog_input_panel.open = false;
                self.label_catalog_input_panel.error = None;
                self.label_catalog_input_panel.input.clear();
                self.label_catalog_input_panel.from = None;
                Ok(None)
            }
            KeyCode::Enter => {
                let name = self.label_catalog_input_panel.input.trim().to_string();
                if name.is_empty() {
                    self.label_catalog_input_panel.error =
                        Some(match self.label_catalog_input_panel.kind {
                            TagKind::Tag => "Enter a tag name".to_string(),
                            TagKind::Collection => "Enter a collection name".to_string(),
                        });
                    return Ok(None);
                }

                match self.label_catalog_input_panel.mode {
                    LabelCatalogInputMode::Create => {
                        let exists = match self.label_catalog_input_panel.kind {
                            TagKind::Tag => self
                                .ctx
                                .known_tags
                                .iter()
                                .any(|t| t.eq_ignore_ascii_case(&name)),
                            TagKind::Collection => self
                                .ctx
                                .known_collections
                                .iter()
                                .any(|c| c.eq_ignore_ascii_case(&name)),
                        };
                        if exists {
                            self.label_catalog_input_panel.error =
                                Some("Already exists".to_string());
                            return Ok(None);
                        }

                        match self.label_catalog_input_panel.kind {
                            TagKind::Tag => self.ctx.known_tags.push(name.clone()),
                            TagKind::Collection => self.ctx.known_collections.push(name.clone()),
                        }
                        self.ctx.normalize_label_catalog();
                        self.ctx
                            .dirty_label_catalog_ops
                            .push(LabelCatalogOp::Create {
                                kind: self.label_catalog_input_panel.kind,
                                name: name.clone(),
                            });
                    }
                    LabelCatalogInputMode::Rename => {
                        let Some(from) = self.label_catalog_input_panel.from.clone() else {
                            self.label_catalog_input_panel.error =
                                Some("Nothing selected".to_string());
                            return Ok(None);
                        };

                        let conflicts = match self.label_catalog_input_panel.kind {
                            TagKind::Tag => self.ctx.known_tags.iter().any(|t| {
                                t.eq_ignore_ascii_case(&name) && !t.eq_ignore_ascii_case(&from)
                            }),
                            TagKind::Collection => self.ctx.known_collections.iter().any(|c| {
                                c.eq_ignore_ascii_case(&name) && !c.eq_ignore_ascii_case(&from)
                            }),
                        };
                        if conflicts {
                            self.label_catalog_input_panel.error =
                                Some("Already exists".to_string());
                            return Ok(None);
                        }

                        if !self.apply_rename_label(
                            self.label_catalog_input_panel.kind,
                            &from,
                            &name,
                        ) {
                            self.label_catalog_input_panel.error =
                                Some("Nothing selected".to_string());
                            return Ok(None);
                        }

                        self.ctx
                            .dirty_label_catalog_ops
                            .push(LabelCatalogOp::Rename {
                                kind: self.label_catalog_input_panel.kind,
                                from,
                                to: name.clone(),
                            });
                    }
                }

                self.label_catalog_input_panel.open = false;
                self.label_catalog_input_panel.error = None;
                self.label_catalog_input_panel.input.clear();
                self.label_catalog_input_panel.from = None;
                Ok(None)
            }
            KeyCode::Backspace => {
                self.label_catalog_input_panel.input.pop();
                Ok(None)
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
                    self.label_catalog_input_panel.input.push(ch);
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.ctx.library_query.trim().to_ascii_lowercase();
        let mut out = Vec::new();
        for (idx, book) in self.ctx.books.iter().enumerate() {
            if self.ctx.favorites_only && !book.favorite {
                continue;
            }

            let labels = self
                .ctx
                .labels_by_path
                .get(&book.path)
                .cloned()
                .unwrap_or_default();
            if !matches_collection_filter(&self.ctx.collection_filter, labels.collection.as_deref())
            {
                continue;
            }
            if !matches_tag_filter(&self.ctx.tag_filters, self.ctx.tag_match_mode, &labels.tags) {
                continue;
            }

            if query.is_empty() {
                out.push(idx);
                continue;
            }

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

    fn main_footer_lines(&self) -> Vec<Line<'static>> {
        if self.label_catalog_input_panel.open {
            return vec![Line::from(vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cancel  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" apply  "),
                Span::styled("Backspace", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" delete  "),
                Span::styled("Ctrl+u", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" clear"),
            ])];
        }

        if self.search_panel.open {
            let enter_action = match self.search_panel.tab {
                SearchTab::Search => "close",
                SearchTab::Assign => {
                    if self.assign_labels_panel.query_editing {
                        "done"
                    } else {
                        "apply + close"
                    }
                }
                SearchTab::Manage => {
                    if self.label_manager_panel.confirm_delete.is_some() {
                        "delete"
                    } else if self.label_manager_panel.filter_editing {
                        "done"
                    } else {
                        "close"
                    }
                }
            };

            return vec![Line::from(vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cancel  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {enter_action}")),
            ])];
        }

        vec![
            Line::from(vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" quit  "),
                Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" move  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" read"),
            ]),
            Line::from(vec![
                Span::styled("/", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" filters  "),
                Span::styled("l", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" labels  "),
                Span::styled("c", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" catalog  "),
                Span::styled("s", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" settings"),
            ]),
        ]
    }

    fn main_header_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(vec![
            Span::styled("Bookshelf", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" — library"),
        ])];

        if let Some(status_line) = self.active_filter_status_line() {
            lines.push(status_line);
        }

        lines
    }

    fn active_filter_status_line(&self) -> Option<Line<'static>> {
        if self.search_panel.open {
            return None;
        }

        let mut parts: Vec<Span<'static>> = Vec::new();
        let style = Style::default().fg(Color::Cyan);

        let query = self.ctx.library_query.trim();
        if !query.is_empty() {
            parts.push(Span::styled(format!("filter: {query}"), style));
        }

        if self.ctx.favorites_only {
            parts.push(Span::styled("favorites only".to_string(), style));
        }

        if let Some(label) = self.active_label_filter_summary() {
            parts.push(Span::styled(label, style));
        }

        if parts.is_empty() {
            None
        } else {
            let mut spans = Vec::with_capacity(parts.len() * 2 - 1);
            for (idx, part) in parts.into_iter().enumerate() {
                if idx > 0 {
                    spans.push(Span::raw("  |  "));
                }
                spans.push(part);
            }
            Some(Line::from(spans))
        }
    }

    fn active_label_filter_summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        match &self.ctx.collection_filter {
            CollectionFilter::Any => {}
            CollectionFilter::None => parts.push("collection:none".to_string()),
            CollectionFilter::Selected(name) => parts.push(format!("collection:{name}")),
        }

        if !self.ctx.tag_filters.is_empty() {
            let mode = match self.ctx.tag_match_mode {
                TagMatchMode::And => "and",
                TagMatchMode::Or => "or",
            };
            parts.push(format!("tags({mode}): {}", self.ctx.tag_filters.join(", ")));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
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
                Constraint::Length(3),
            ])
            .split(area);

        let title = Paragraph::new(Text::from(self.main_header_lines()))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(title, layout[0]);

        let body_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(layout[1]);

        self.draw_library(frame, body_layout[0]);
        frame.render_widget(self.draw_details(body_layout[1]), body_layout[1]);

        let footer = Paragraph::new(Text::from(self.main_footer_lines()))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::TOP));
        frame.render_widget(footer, layout[2]);

        if self.settings_panel.open {
            self.draw_settings_panel(area, frame);
        }

        if self.scan_panel.open {
            self.draw_scan_panel(area, frame);
        }

        if self.search_panel.open {
            self.draw_search_panel(area, frame);
        }

        if self.label_catalog_input_panel.open {
            self.draw_label_catalog_input_panel(area, frame);
        }
    }

    fn draw_search_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(90, 78, area);
        frame.render_widget(Clear, popup_area);

        let title = match self.search_panel.mode {
            SearchPanelMode::Filters => "Filters".to_string(),
            SearchPanelMode::Labels => "Labels".to_string(),
            SearchPanelMode::Catalog => "Catalog".to_string(),
        };

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        match self.search_panel.mode {
            SearchPanelMode::Filters => {
                self.draw_search_filters_tab(inner, frame);
            }
            SearchPanelMode::Labels => self.draw_search_assign_tab(inner, frame),
            SearchPanelMode::Catalog => self.draw_search_manage_tab(inner, frame),
        }
    }

    fn draw_search_filters_tab(&self, area: Rect, frame: &mut ratatui::Frame) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(7),
                Constraint::Min(0),
                Constraint::Length(4),
            ])
            .split(area);

        let filters_lines = self.search_panel_summary_lines(self.search_panel.focus);
        let header = Paragraph::new(Text::from(filters_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(header, sections[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(sections[1]);

        self.draw_search_collections_list(body[0], frame);
        self.draw_search_tags_list(body[1], frame);

        let help_lines = vec![
            Line::from(vec![
                Span::styled("Tab", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" focus  "),
                Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" move  "),
                Span::styled("Space", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" toggle"),
            ]),
            Line::from(vec![
                Span::styled("←/→", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" tags AND/OR  "),
                Span::styled("f", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" favorites-only  "),
                Span::styled("Ctrl+u", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" clear all"),
            ]),
            Line::from(vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cancel  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" close"),
            ]),
        ];
        let help = Paragraph::new(Text::from(help_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(help, sections[2]);
    }

    fn search_panel_summary_lines(&self, focus: SearchFocus) -> Vec<Line<'static>> {
        let base_label_style = Style::default().add_modifier(Modifier::BOLD);
        let focus_style = Style::default()
            .fg(self.accent_color())
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);

        let query_label_style = if focus == SearchFocus::Query {
            focus_style
        } else {
            base_label_style
        };
        let query_value_style = if focus == SearchFocus::Query {
            focus_style
        } else {
            Style::default()
        };

        let collection_label_style = if focus == SearchFocus::Collections {
            focus_style
        } else {
            base_label_style
        };
        let collection_value_style = if focus == SearchFocus::Collections {
            focus_style
        } else {
            Style::default()
        };

        let tags_label_style = if focus == SearchFocus::Tags {
            focus_style
        } else {
            base_label_style
        };
        let tags_value_style = if focus == SearchFocus::Tags {
            focus_style
        } else {
            Style::default()
        };

        let query = self.ctx.library_query.clone();
        let fav = if self.ctx.favorites_only { "on" } else { "off" };
        let collection = match &self.ctx.collection_filter {
            CollectionFilter::Any => "any".to_string(),
            CollectionFilter::None => "none".to_string(),
            CollectionFilter::Selected(name) => name.clone(),
        };
        let mode = match self.ctx.tag_match_mode {
            TagMatchMode::And => "AND",
            TagMatchMode::Or => "OR",
        };
        let tags = if self.ctx.tag_filters.is_empty() {
            "(none)".to_string()
        } else {
            self.ctx.tag_filters.join(", ")
        };

        vec![
            Line::from(vec![
                Span::styled("Query: ", query_label_style),
                Span::styled(query, query_value_style),
            ]),
            Line::from(vec![
                Span::styled("Favorites only: ", base_label_style),
                Span::raw(fav),
                Span::raw("  "),
                Span::styled("Collection: ", collection_label_style),
                Span::styled(collection, collection_value_style),
            ]),
            Line::from(vec![
                Span::styled("Tags: ", tags_label_style),
                Span::styled(tags, tags_value_style),
            ]),
            Line::from(vec![
                Span::styled("Tag match: ", tags_label_style),
                Span::styled(mode, tags_value_style),
            ]),
        ]
    }

    fn draw_search_collections_list(&self, area: Rect, frame: &mut ratatui::Frame) {
        let entries = self.collection_entries_for_search();
        let mut cursor = self.search_panel.collection_cursor;
        if !entries.is_empty() {
            cursor = cursor.min(entries.len().saturating_sub(1));
        } else {
            cursor = 0;
        }

        let focus = self.search_panel.focus == SearchFocus::Collections;
        let title_style = if focus {
            Style::default()
                .fg(self.accent_color())
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default()
        };

        let items: Vec<ListItem> = if entries.is_empty() {
            vec![ListItem::new(Line::raw("(none)"))]
        } else {
            entries
                .iter()
                .map(|e| {
                    let selected = e.filter == self.ctx.collection_filter;
                    let prefix = if selected { "●" } else { " " };
                    ListItem::new(Line::raw(format!("{prefix} {} ({})", e.label, e.count)))
                })
                .collect()
        };

        let focused_list_style = Style::default()
            .fg(Color::Black)
            .bg(self.accent_color())
            .add_modifier(Modifier::BOLD);
        let unfocused_list_style = Style::default().fg(Color::White).bg(Color::Gray);

        let highlight_style = if focus {
            focused_list_style
        } else {
            unfocused_list_style
        };

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if focus {
                        Style::default().fg(self.accent_color())
                    } else {
                        Style::default()
                    })
                    .title(Span::styled("Collections", title_style)),
            )
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);
        let mut state = ListState::default();
        if !entries.is_empty() {
            state.select(Some(cursor));
        }
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_search_tags_list(&self, area: Rect, frame: &mut ratatui::Frame) {
        let entries = self.tag_entries_for_search();
        let mut cursor = self.search_panel.tag_cursor;
        if !entries.is_empty() {
            cursor = cursor.min(entries.len().saturating_sub(1));
        } else {
            cursor = 0;
        }

        let focus = self.search_panel.focus == SearchFocus::Tags;
        let mode = match self.ctx.tag_match_mode {
            TagMatchMode::And => "AND",
            TagMatchMode::Or => "OR",
        };
        let title_style = if focus {
            Style::default()
                .fg(self.accent_color())
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default()
        };
        let title = format!("Tags ({mode})");

        let items: Vec<ListItem> = if entries.is_empty() {
            vec![ListItem::new(Line::raw("(none)"))]
        } else {
            entries
                .iter()
                .map(|e| {
                    let selected = self
                        .ctx
                        .tag_filters
                        .iter()
                        .any(|t| t.eq_ignore_ascii_case(&e.name));
                    let prefix = if selected { "[x]" } else { "[ ]" };
                    ListItem::new(Line::raw(format!("{prefix} {} ({})", e.name, e.count)))
                })
                .collect()
        };

        let focused_list_style = Style::default()
            .fg(Color::Black)
            .bg(self.accent_color())
            .add_modifier(Modifier::BOLD);
        let unfocused_list_style = Style::default().fg(Color::White).bg(Color::Gray);

        let highlight_style = if focus {
            focused_list_style
        } else {
            unfocused_list_style
        };

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if focus {
                        Style::default().fg(self.accent_color())
                    } else {
                        Style::default()
                    })
                    .title(Span::styled(title, title_style)),
            )
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);
        let mut state = ListState::default();
        if !entries.is_empty() {
            state.select(Some(cursor));
        }
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_search_manage_tab(&self, area: Rect, frame: &mut ratatui::Frame) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(0),
                Constraint::Length(4),
            ])
            .split(area);

        let filter_style = if self.label_manager_panel.filter_editing {
            Style::default()
                .fg(self.accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let filter_label = match self.label_manager_panel.tab {
            LabelManagerTab::Collections => "Filter (Collections): ",
            LabelManagerTab::Tags => "Filter (Tags): ",
        };

        let header_lines = vec![Line::from(vec![
            Span::styled(filter_label, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(self.label_manager_panel.filter.clone(), filter_style),
            Span::raw("  "),
            Span::raw("(/ to edit, Ctrl+u clear)"),
        ])];
        let header = Paragraph::new(Text::from(header_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(header, sections[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(sections[1]);

        let collections_entries = self.label_manager_entries(LabelManagerTab::Collections);
        let tags_entries = self.label_manager_entries(LabelManagerTab::Tags);

        let mut collections_cursor = self.label_manager_panel.collections_cursor;
        if !collections_entries.is_empty() {
            collections_cursor = collections_cursor.min(collections_entries.len() - 1);
        } else {
            collections_cursor = 0;
        }

        let mut tags_cursor = self.label_manager_panel.tags_cursor;
        if !tags_entries.is_empty() {
            tags_cursor = tags_cursor.min(tags_entries.len() - 1);
        } else {
            tags_cursor = 0;
        }

        let focused_list_style = Style::default()
            .fg(Color::Black)
            .bg(self.accent_color())
            .add_modifier(Modifier::BOLD);
        let unfocused_list_style = Style::default().fg(Color::White).bg(Color::Gray);

        // Collections
        let collections_focus = self.label_manager_panel.tab == LabelManagerTab::Collections;
        let collections_title_style = if collections_focus {
            Style::default()
                .fg(self.accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let collections_items: Vec<ListItem> = if collections_entries.is_empty() {
            vec![ListItem::new(Line::raw("(none)"))]
        } else {
            collections_entries
                .iter()
                .map(|(name, count)| ListItem::new(Line::raw(format!("{name} ({count})"))))
                .collect()
        };
        let collections_list = List::new(collections_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if collections_focus {
                        Style::default().fg(self.accent_color())
                    } else {
                        Style::default()
                    })
                    .title(Span::styled("Collections", collections_title_style)),
            )
            .highlight_style(if collections_focus {
                focused_list_style
            } else {
                unfocused_list_style
            })
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);
        let mut collections_state = ListState::default();
        if !collections_entries.is_empty() {
            collections_state.select(Some(collections_cursor));
        }
        frame.render_stateful_widget(collections_list, body[0], &mut collections_state);

        // Tags
        let tags_focus = self.label_manager_panel.tab == LabelManagerTab::Tags;
        let tags_title_style = if tags_focus {
            Style::default()
                .fg(self.accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let tags_items: Vec<ListItem> = if tags_entries.is_empty() {
            vec![ListItem::new(Line::raw("(none)"))]
        } else {
            tags_entries
                .iter()
                .map(|(name, count)| ListItem::new(Line::raw(format!("{name} ({count})"))))
                .collect()
        };
        let tags_list = List::new(tags_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if tags_focus {
                        Style::default().fg(self.accent_color())
                    } else {
                        Style::default()
                    })
                    .title(Span::styled("Tags", tags_title_style)),
            )
            .highlight_style(if tags_focus {
                focused_list_style
            } else {
                unfocused_list_style
            })
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);
        let mut tags_state = ListState::default();
        if !tags_entries.is_empty() {
            tags_state.select(Some(tags_cursor));
        }
        frame.render_stateful_widget(tags_list, body[1], &mut tags_state);

        let enter_action = if self.label_manager_panel.confirm_delete.is_some() {
            "delete"
        } else if self.label_manager_panel.filter_editing {
            "done"
        } else {
            "close"
        };

        let mut footer_lines = Vec::new();
        footer_lines.push(Line::from(vec![
            Span::styled("Tab", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" switch panel  "),
            Span::styled("/", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" filter"),
        ]));
        footer_lines.push(Line::from(vec![
            Span::styled("n", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" new  "),
            Span::styled("r", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" rename  "),
            Span::styled("d", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" delete  "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel  "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(" {enter_action}")),
        ]));

        if let Some(target) = &self.label_manager_panel.confirm_delete {
            footer_lines.push(Line::raw(""));
            footer_lines.push(Line::styled(
                format!("Delete {} \"{}\"? (y/n)", target.kind, target.name),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        } else if let Some(err) = &self.label_manager_panel.error {
            footer_lines.push(Line::raw(""));
            footer_lines.push(Line::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }

        let footer = Paragraph::new(Text::from(footer_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(footer, sections[2]);
    }

    fn draw_search_assign_tab(&self, area: Rect, frame: &mut ratatui::Frame) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(6),
                Constraint::Min(0),
                Constraint::Length(4),
            ])
            .split(area);

        let (filter_label, filter_value) = match self.assign_labels_panel.focus {
            AssignFocus::Collections => (
                "Filter (collections): ",
                &self.assign_labels_panel.collection_query,
            ),
            AssignFocus::Tags => ("Filter (tags): ", &self.assign_labels_panel.tag_query),
        };
        let filter_style = if self.assign_labels_panel.query_editing {
            Style::default()
                .fg(self.accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let book_title = self
            .assign_labels_panel
            .book_path
            .as_deref()
            .and_then(|path| self.ctx.books.iter().find(|b| b.path == path))
            .map(|b| b.title.clone())
            .unwrap_or_else(|| "(no selection)".to_string());

        let fav = if self.assign_labels_panel.staged_favorite {
            "yes"
        } else {
            "no"
        };

        let mut header_lines = Vec::new();
        header_lines.push(Line::from(vec![
            Span::styled("Selected: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(book_title),
        ]));
        header_lines.push(Line::from(vec![
            Span::styled("Favorite: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(fav),
            Span::raw("  "),
            Span::raw("(f toggles)"),
        ]));
        header_lines.push(Line::from(vec![
            Span::styled(filter_label, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(filter_value.clone(), filter_style),
            Span::raw("  "),
            Span::raw("(/ to edit, Ctrl+u clear)"),
        ]));

        if let Some(err) = &self.assign_labels_panel.error {
            header_lines.push(Line::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        } else {
            header_lines.push(Line::raw(""));
        }
        header_lines.push(Line::raw(""));

        let header = Paragraph::new(Text::from(header_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(header, sections[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(sections[1]);

        let focused_list_style = Style::default()
            .fg(Color::Black)
            .bg(self.accent_color())
            .add_modifier(Modifier::BOLD);
        let unfocused_list_style = Style::default().fg(Color::White).bg(Color::Gray);

        // Collections
        let collections_focus = self.assign_labels_panel.focus == AssignFocus::Collections;
        let collections = self.assign_visible_collections();
        let mut collection_cursor = self.assign_labels_panel.collection_cursor;
        let collection_max = collections.len() + 1;
        if collection_max > 0 {
            collection_cursor = collection_cursor.min(collection_max - 1);
        } else {
            collection_cursor = 0;
        }

        let mut collection_items = Vec::new();
        let none_selected = self.assign_labels_panel.staged.collection.is_none();
        let prefix = if none_selected { "●" } else { " " };
        collection_items.push(ListItem::new(Line::raw(format!("{prefix} (none)"))));
        for name in &collections {
            let selected = self
                .assign_labels_panel
                .staged
                .collection
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(name));
            let prefix = if selected { "●" } else { " " };
            collection_items.push(ListItem::new(Line::raw(format!("{prefix} {name}"))));
        }

        let collections_title_style = if self.assign_labels_panel.focus == AssignFocus::Collections
        {
            Style::default()
                .fg(self.accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let collections_list = List::new(collection_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if collections_focus {
                        Style::default().fg(self.accent_color())
                    } else {
                        Style::default()
                    })
                    .title(Span::styled("Collection", collections_title_style)),
            )
            .highlight_style(if collections_focus {
                focused_list_style
            } else {
                unfocused_list_style
            })
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);
        let mut collection_state = ListState::default();
        if collection_max > 0 {
            collection_state.select(Some(collection_cursor));
        }
        frame.render_stateful_widget(collections_list, body[0], &mut collection_state);

        // Tags
        let tags_focus = self.assign_labels_panel.focus == AssignFocus::Tags;
        let tags = self.assign_visible_tags();
        let mut tag_cursor = self.assign_labels_panel.tag_cursor;
        if !tags.is_empty() {
            tag_cursor = tag_cursor.min(tags.len() - 1);
        } else {
            tag_cursor = 0;
        }

        let mut tag_items = Vec::new();
        if tags.is_empty() {
            tag_items.push(ListItem::new(Line::raw("(none)")));
        } else {
            for tag in &tags {
                let selected = self
                    .assign_labels_panel
                    .staged
                    .tags
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(tag));
                let prefix = if selected { "[x]" } else { "[ ]" };
                tag_items.push(ListItem::new(Line::raw(format!("{prefix} {tag}"))));
            }
        }

        let tags_title_style = if self.assign_labels_panel.focus == AssignFocus::Tags {
            Style::default()
                .fg(self.accent_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let tags_list = List::new(tag_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if tags_focus {
                        Style::default().fg(self.accent_color())
                    } else {
                        Style::default()
                    })
                    .title(Span::styled("Tags", tags_title_style)),
            )
            .highlight_style(if tags_focus {
                focused_list_style
            } else {
                unfocused_list_style
            })
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);
        let mut tag_state = ListState::default();
        if !tags.is_empty() {
            tag_state.select(Some(tag_cursor));
        }
        frame.render_stateful_widget(tags_list, body[1], &mut tag_state);

        let enter_action = if self.assign_labels_panel.query_editing {
            "done"
        } else {
            "apply + close"
        };

        let footer_lines = vec![
            Line::from(vec![
                Span::styled("Tab", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" focus  "),
                Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" move"),
            ]),
            Line::from(vec![
                Span::styled("/", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" filter  "),
                Span::styled("f", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" favorite  "),
                Span::styled("Space", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" toggle"),
            ]),
            Line::from(vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cancel  "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {enter_action}")),
            ]),
        ];
        let footer = Paragraph::new(Text::from(footer_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(footer, sections[2]);
    }

    fn draw_label_catalog_input_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(70, 35, area);
        frame.render_widget(Clear, popup_area);

        let title = match (
            self.label_catalog_input_panel.mode,
            self.label_catalog_input_panel.kind,
        ) {
            (LabelCatalogInputMode::Create, TagKind::Tag) => "New tag",
            (LabelCatalogInputMode::Create, TagKind::Collection) => "New collection",
            (LabelCatalogInputMode::Rename, TagKind::Tag) => "Rename tag",
            (LabelCatalogInputMode::Rename, TagKind::Collection) => "Rename collection",
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
                Constraint::Length(5),
            ])
            .split(inner);

        let prompt = match self.label_catalog_input_panel.kind {
            TagKind::Tag => "Tag: ",
            TagKind::Collection => "Collection: ",
        };
        let header = Paragraph::new(Line::from(vec![
            Span::styled(prompt, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.label_catalog_input_panel.input.clone()),
        ]))
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Left);
        frame.render_widget(header, sections[0]);

        let mut help_lines = Vec::new();
        help_lines.push(Line::from(vec![
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" apply  "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel  "),
            Span::styled("Backspace", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" delete  "),
            Span::styled("Ctrl+U", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" clear"),
        ]));

        if let Some(err) = &self.label_catalog_input_panel.error {
            help_lines.push(Line::raw(""));
            help_lines.push(Line::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }

        let help = Paragraph::new(Text::from(help_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(help, sections[2]);
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
            .bg(self.accent_color())
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
            .bg(self.accent_color())
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

        let visible = self.toc_visible_indices();

        let title = match self.toc_panel.error.as_deref() {
            Some(_) => "Table of Contents (error)".to_string(),
            None => {
                if self.toc_panel.query.trim().is_empty() {
                    format!("Table of Contents — {}", self.toc_panel.items.len())
                } else {
                    format!(
                        "Table of Contents — {}/{}",
                        visible.len(),
                        self.toc_panel.items.len()
                    )
                }
            }
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
        header_lines.push(Line::from(vec![
            Span::styled("Filter: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.toc_panel.query.clone()),
        ]));
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
        } else if !self.toc_panel.query.trim().is_empty() && visible.is_empty() {
            vec![ListItem::new(Line::raw("(no matches)"))]
        } else {
            visible
                .iter()
                .filter_map(|idx| self.toc_panel.items.get(*idx))
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
            .bg(self.accent_color())
            .add_modifier(Modifier::BOLD);

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always);

        let mut state = ListState::default();
        if !visible.is_empty() {
            state.select(Some(self.toc_panel.selected.min(visible.len() - 1)));
        }
        frame.render_stateful_widget(list, sections[1], &mut state);

        let footer = Paragraph::new(Line::from(vec![
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" close  "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" jump  "),
            Span::styled("Ctrl+u", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" clear"),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(footer, sections[2]);
    }

    fn draw_reader(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        if self.ctx.settings.reader_mode == ReaderMode::Image
            && !image_protocol::image_supported(&self.image_picker)
        {
            self.ctx.settings.reader_mode = ReaderMode::Text;
            self.reader.current_image = None;
            self.reader.render_key = None;
            self.reader.notice =
                Some("image protocol not detected; image mode disabled".to_string());
        }

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .split(area);

        let title_text = match &self.reader.book_title {
            Some(title) => format!("Reader — {title}"),
            None => "Reader".to_string(),
        };

        if self.ctx.settings.reader_mode == ReaderMode::Image {
            image_protocol::prefer_kitty_if_supported(&mut self.image_picker);
        }

        let page_title = {
            let page = self.reader.page.saturating_add(1);
            let page_part = if let Some(total) = self.reader.total_pages {
                format!("p{page}/{total}")
            } else {
                format!("p{page}")
            };

            let mode_part = match self.ctx.settings.reader_mode {
                ReaderMode::Text => self.ctx.settings.reader_text_mode.to_string(),
                ReaderMode::Image => {
                    let (fw, fh) = self.image_picker.font_size();
                    format!(
                        "{} {}% · {}x{}px",
                        image_protocol::protocol_label(&self.image_picker),
                        self.reader.image_zoom_percent,
                        fw,
                        fh
                    )
                }
            };

            format!("{page_part} · {mode_part}")
        };

        let header = Paragraph::new(Line::from(vec![Span::styled(
            title_text,
            Style::default().add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(header, layout[0]);

        let inner_width = layout[1].width.saturating_sub(2);
        let inner_height = layout[1].height.saturating_sub(2);
        self.reader.ensure_rendered(
            &self.ctx,
            &self.engine,
            &self.image_picker,
            inner_width,
            inner_height,
        );

        if self.ctx.settings.reader_mode == ReaderMode::Image {
            let block = Block::default().borders(Borders::ALL).title(page_title);
            frame.render_widget(block.clone(), layout[1]);
            let inner = block.inner(layout[1]);

            if let Some(protocol) = self.reader.current_image.as_ref() {
                let proto_area = protocol.area();
                let draw_width = proto_area.width.min(inner.width);
                let draw_height = proto_area.height.min(inner.height);
                let draw_area = Rect::new(
                    inner.x + inner.width.saturating_sub(draw_width) / 2,
                    inner.y + inner.height.saturating_sub(draw_height) / 2,
                    draw_width,
                    draw_height,
                );
                frame.render_widget(ImageWidget::new(protocol), draw_area);
            } else {
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
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
        } else {
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
                .block(Block::default().borders(Borders::ALL).title(page_title));
            frame.render_widget(body, layout[1]);
        }

        let up_down_label = if self.ctx.settings.reader_mode == ReaderMode::Image {
            "pan-y"
        } else {
            "scroll"
        };

        let mut footer_spans = vec![
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" back  "),
            Span::styled("←/→", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" page  "),
            Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(" {up_down_label}  ")),
            Span::styled("g", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" goto  "),
            Span::styled("t", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" toc  "),
            Span::styled("b", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" bookmarks  "),
            Span::styled("n", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" notes  "),
            Span::styled("d", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" dump"),
        ];

        let image_ok = image_protocol::image_supported(&self.image_picker);
        if self.ctx.settings.reader_mode == ReaderMode::Image || image_ok {
            footer_spans.push(Span::raw("  "));
            footer_spans.push(Span::styled(
                "m",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw(" mode"));
        }

        if self.ctx.settings.reader_mode == ReaderMode::Text && !image_ok {
            footer_spans.push(Span::raw("  "));
            footer_spans.push(Span::styled(
                "k",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw(" kitty-reader"));
        }

        if self.ctx.settings.reader_mode == ReaderMode::Text {
            footer_spans.push(Span::raw("  "));
            footer_spans.push(Span::styled(
                "r",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw(" raw/wrap/reflow"));

            footer_spans.push(Span::raw("  "));
            footer_spans.push(Span::styled(
                "h",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw(" trim-hf"));
        }

        if self.ctx.settings.reader_mode == ReaderMode::Image {
            footer_spans.push(Span::raw("  "));
            footer_spans.push(Span::styled(
                "+/-",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw(" zoom  "));
            footer_spans.push(Span::styled(
                "0",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw(" reset  "));
            footer_spans.push(Span::styled(
                "Shift+←/→",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw(" pan-x  "));
            footer_spans.push(Span::styled(
                "PgUp/PgDn",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw(" page-pan"));
        }

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
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::TOP));
        frame.render_widget(footer, layout[2]);

        if self.bookmarks_panel.open {
            self.draw_bookmarks_panel(area, frame);
        }
        if self.goto_panel.open {
            self.draw_goto_panel(area, frame);
        }
        if self.toc_panel.open {
            self.draw_toc_panel(area, frame);
        }
        if self.notes_panel.open {
            self.draw_notes_panel(area, frame);
        }
    }

    fn draw_goto_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(48, 28, area);
        frame.render_widget(Clear, popup_area);

        let title = match self.reader.total_pages {
            Some(total) => format!("Go to page (1..={total})"),
            None => "Go to page".to_string(),
        };

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Page: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(self.goto_panel.input.clone()),
            ]),
            Line::raw(""),
            Line::raw("Enter jumps, Esc cancels, Ctrl+u clears."),
        ];

        if let Some(err) = &self.goto_panel.error {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        }

        let paragraph = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(paragraph, inner);
    }

    fn draw_settings_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(45, 30, area);
        frame.render_widget(Clear, popup_area);

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            "Settings",
            Style::default().add_modifier(Modifier::BOLD),
        ));

        frame.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(inner);

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(self.accent_color())
            .add_modifier(Modifier::BOLD);

        let kitty_quality_row_selected =
            self.settings_panel.selected == SETTINGS_MENU_KITTY_IMAGE_QUALITY;
        let theme_row_selected = self.settings_panel.selected == SETTINGS_MENU_THEME;
        let items = vec![
            ListItem::new(Line::raw("Scan Paths")),
            ListItem::new(Line::from(vec![
                Span::styled(
                    "Kitty image quality: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                option_chip(
                    "fast",
                    self.ctx.settings.kitty_image_quality == KittyImageQuality::Fast,
                    kitty_quality_row_selected,
                ),
                Span::raw(" "),
                option_chip(
                    "balanced",
                    self.ctx.settings.kitty_image_quality == KittyImageQuality::Balanced,
                    kitty_quality_row_selected,
                ),
                Span::raw(" "),
                option_chip(
                    "sharp",
                    self.ctx.settings.kitty_image_quality == KittyImageQuality::Sharp,
                    kitty_quality_row_selected,
                ),
            ])),
            ListItem::new(Line::from(vec![
                Span::styled("Theme: ", Style::default().add_modifier(Modifier::BOLD)),
                option_chip(
                    "dark",
                    self.ctx.settings.theme == Theme::Dark,
                    theme_row_selected,
                ),
                Span::raw(" "),
                option_chip(
                    "light",
                    self.ctx.settings.theme == Theme::Light,
                    theme_row_selected,
                ),
            ])),
        ];

        let list = List::new(items)
            .highlight_style(highlight_style)
            .highlight_symbol("> ")
            .highlight_spacing(HighlightSpacing::Always)
            .block(Block::default());

        let mut state = ListState::default();
        state.select(Some(
            self.settings_panel
                .selected
                .min(SETTINGS_MENU_ITEM_COUNT.saturating_sub(1)),
        ));
        frame.render_stateful_widget(list, sections[0], &mut state);

        let help_lines = vec![Line::from(vec![
            Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" select  "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" open/toggle  "),
            Span::styled("←/→", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" adjust  "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" close"),
        ])];
        let help = Paragraph::new(Text::from(help_lines))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left);
        frame.render_widget(help, sections[1]);
    }

    fn draw_library(&self, frame: &mut ratatui::Frame, area: Rect) {
        let visible = self.visible_indices();
        let has_filters = !self.ctx.library_query.trim().is_empty()
            || self.ctx.favorites_only
            || !matches!(self.ctx.collection_filter, CollectionFilter::Any)
            || !self.ctx.tag_filters.is_empty();
        let title = if has_filters {
            format!(
                "Library — {}/{} matches",
                visible.len(),
                self.ctx.books.len()
            )
        } else {
            "Library".to_string()
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
            let query = self.ctx.library_query.trim();
            if !query.is_empty() {
                lines.push(Line::raw(""));
                lines.push(Line::raw(format!("Query: {query}")));
                lines.push(Line::raw("Tip: press / to edit filters, Ctrl+u to clear."));
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
                let label = if book.favorite {
                    format!("★ {}", book.title)
                } else {
                    format!("  {}", book.title)
                };
                let wrapped = wrap_text(&label, max_title_width.max(8));
                let lines = wrapped.into_iter().map(Line::raw).collect::<Vec<_>>();
                ListItem::new(Text::from(lines))
            })
            .collect();

        let highlight_style = Style::default()
            .fg(Color::Black)
            .bg(self.accent_color())
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

    fn draw_details(&self, _area: Rect) -> Paragraph<'static> {
        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("Reader: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.reader_mode.to_string()),
            Span::raw("  "),
            Span::styled("Scan: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.scan_scope.to_string()),
            Span::raw("  "),
            Span::styled("Roots: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.library_roots.len().to_string()),
        ]));
        let fav_status = if self.ctx.favorites_only { "on" } else { "off" };
        let collection = match &self.ctx.collection_filter {
            CollectionFilter::Any => "any".to_string(),
            CollectionFilter::None => "none".to_string(),
            CollectionFilter::Selected(name) => name.clone(),
        };
        let tag_mode = match self.ctx.tag_match_mode {
            TagMatchMode::And => "and",
            TagMatchMode::Or => "or",
        };
        let tags = if self.ctx.tag_filters.is_empty() {
            "-".to_string()
        } else {
            self.ctx.tag_filters.join(", ")
        };
        let query = if self.ctx.library_query.trim().is_empty() {
            "-".to_string()
        } else {
            self.ctx.library_query.trim().to_string()
        };
        lines.push(Line::from(vec![
            Span::styled("Filters: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("fav:{fav_status} (←/→)  ")),
            Span::raw(format!("collection:{collection}  ")),
            Span::raw(format!("tags({tag_mode}): {tags}  ")),
            Span::raw(format!("query:{query}")),
        ]));
        lines.push(Line::raw(""));

        if let Some(book) = self.ctx.books.get(self.ctx.selected) {
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
            let labels = self
                .ctx
                .labels_by_path
                .get(&book.path)
                .cloned()
                .unwrap_or_default();
            let tags = if labels.tags.is_empty() {
                "(none)".to_string()
            } else {
                labels.tags.join(", ")
            };
            let collection = labels.collection.unwrap_or_else(|| "-".to_string());
            lines.push(Line::from(vec![
                Span::styled("Favorite: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(if book.favorite { "yes" } else { "no" }),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    "Collection: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(collection),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Tags: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(tags),
            ]));
            lines.push(Line::raw(""));
        } else {
            lines.push(Line::raw("No selection."));
        }

        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title("Details"))
            .wrap(Wrap { trim: true })
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
            .bg(self.accent_color())
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

const SETTINGS_MENU_SCAN_PATHS: usize = 0;
const SETTINGS_MENU_KITTY_IMAGE_QUALITY: usize = 1;
const SETTINGS_MENU_THEME: usize = 2;
const SETTINGS_MENU_ITEM_COUNT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchPanelMode {
    Filters,
    Labels,
    Catalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchTab {
    Search,
    Assign,
    Manage,
}

#[derive(Debug, Clone)]
struct SearchPanel {
    open: bool,
    mode: SearchPanelMode,
    tab: SearchTab,
    focus: SearchFocus,
    collection_cursor: usize,
    tag_cursor: usize,
    snapshot: Option<SearchSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchFocus {
    Query,
    Collections,
    Tags,
}

impl SearchFocus {
    fn next(self) -> Self {
        match self {
            SearchFocus::Query => SearchFocus::Collections,
            SearchFocus::Collections => SearchFocus::Tags,
            SearchFocus::Tags => SearchFocus::Query,
        }
    }

    fn prev(self) -> Self {
        match self {
            SearchFocus::Query => SearchFocus::Tags,
            SearchFocus::Collections => SearchFocus::Query,
            SearchFocus::Tags => SearchFocus::Collections,
        }
    }
}

#[derive(Debug, Clone)]
struct SearchSnapshot {
    library_query: String,
    favorites_only: bool,
    collection_filter: CollectionFilter,
    tag_filters: Vec<String>,
    tag_match_mode: TagMatchMode,
    selected_path: Option<String>,
}

impl Default for SearchPanel {
    fn default() -> Self {
        Self {
            open: false,
            mode: SearchPanelMode::Filters,
            tab: SearchTab::Search,
            focus: SearchFocus::Query,
            collection_cursor: 0,
            tag_cursor: 0,
            snapshot: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabelManagerTab {
    Collections,
    Tags,
}

#[derive(Debug, Clone)]
struct LabelManagerPanel {
    tab: LabelManagerTab,
    collections_cursor: usize,
    tags_cursor: usize,
    filter_editing: bool,
    filter: String,
    confirm_delete: Option<LabelDeleteTarget>,
    error: Option<String>,
}

impl Default for LabelManagerPanel {
    fn default() -> Self {
        Self {
            tab: LabelManagerTab::Collections,
            collections_cursor: 0,
            tags_cursor: 0,
            filter_editing: false,
            filter: String::new(),
            confirm_delete: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
struct LabelDeleteTarget {
    kind: TagKind,
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssignFocus {
    Collections,
    Tags,
}

#[derive(Debug, Clone)]
struct AssignLabelsPanel {
    focus: AssignFocus,
    collection_cursor: usize,
    tag_cursor: usize,
    collection_query: String,
    tag_query: String,
    query_editing: bool,
    book_path: Option<String>,
    staged_favorite: bool,
    staged: BookLabels,
    error: Option<String>,
}

impl Default for AssignLabelsPanel {
    fn default() -> Self {
        Self {
            focus: AssignFocus::Collections,
            collection_cursor: 0,
            tag_cursor: 0,
            collection_query: String::new(),
            tag_query: String::new(),
            query_editing: false,
            book_path: None,
            staged_favorite: false,
            staged: BookLabels::default(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabelCatalogInputMode {
    Create,
    Rename,
}

#[derive(Debug, Clone)]
struct LabelCatalogInputPanel {
    open: bool,
    mode: LabelCatalogInputMode,
    kind: TagKind,
    from: Option<String>,
    input: String,
    error: Option<String>,
}

impl Default for LabelCatalogInputPanel {
    fn default() -> Self {
        Self {
            open: false,
            mode: LabelCatalogInputMode::Create,
            kind: TagKind::Tag,
            from: None,
            input: String::new(),
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
struct CollectionEntry {
    label: String,
    filter: CollectionFilter,
    count: usize,
}

#[derive(Debug, Clone)]
struct TagEntry {
    name: String,
    count: usize,
}

#[derive(Debug, Clone, Default)]
struct GotoPanel {
    open: bool,
    input: String,
    error: Option<String>,
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
    query: String,
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
    mode: ReaderMode,
    text_mode: ReaderTextMode,
    width: u16,
    height: u16,
}

#[derive(Clone)]
struct CachedPageImage {
    page: u32,
    zoom_percent: u16,
    render_width_px: u32,
    font_size: (u16, u16),
    image: Arc<image::DynamicImage>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ReaderImageTimings {
    total_ms: u128,
    rasterize_ms: Option<u128>,
    viewport_ms: u128,
    downscale_ms: u128,
    protocol_ms: u128,
    viewport_px: (u32, u32),
    transmit_px: (u32, u32),
    render_width_px: u32,
}

#[derive(Clone)]
struct ReaderPanel {
    open: bool,
    book_path: Option<String>,
    book_title: Option<String>,
    page: u32,
    total_pages: Option<u32>,
    scroll: u16,
    image_zoom_percent: u16,
    image_pan_x_px: u32,
    image_pan_y_px: u32,
    page_image: Option<CachedPageImage>,
    page_image_cache: VecDeque<CachedPageImage>,
    page_furniture: Option<PageFurniture>,
    current_text: Option<String>,
    current_image: Option<ImageProtocol>,
    last_error: Option<String>,
    notice: Option<String>,
    render_key: Option<ReaderRenderKey>,
    last_image_timings: Option<ReaderImageTimings>,
    next_kitty_image_id: u32,
}

impl Default for ReaderPanel {
    fn default() -> Self {
        Self {
            open: false,
            book_path: None,
            book_title: None,
            page: 0,
            total_pages: None,
            scroll: 0,
            image_zoom_percent: 100,
            image_pan_x_px: 0,
            image_pan_y_px: 0,
            page_image: None,
            page_image_cache: VecDeque::new(),
            page_furniture: None,
            current_text: None,
            current_image: None,
            last_error: None,
            notice: None,
            render_key: None,
            last_image_timings: None,
            next_kitty_image_id: 1,
        }
    }
}

impl ReaderPanel {
    fn open_book(&mut self, book: &bookshelf_core::Book, ctx: &AppContext, engine: &Engine) {
        self.open = true;
        self.book_path = Some(book.path.clone());
        self.book_title = Some(book.title.clone());
        self.page_image_cache.clear();
        self.page_furniture = None;
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
            favorite: false,
        })
    }

    fn invalidate_render(&mut self) {
        self.scroll = 0;
        self.image_pan_x_px = 0;
        self.image_pan_y_px = 0;
        self.page_image = None;
        self.current_text = None;
        self.current_image = None;
        self.last_error = None;
        self.notice = None;
        self.render_key = None;
        self.last_image_timings = None;
    }

    fn cache_page_image(&mut self, image: CachedPageImage) {
        const MAX: usize = 3;
        if let Some(pos) = self.page_image_cache.iter().position(|c| {
            c.page == image.page
                && c.zoom_percent == image.zoom_percent
                && c.render_width_px == image.render_width_px
                && c.font_size == image.font_size
        }) {
            let _ = self.page_image_cache.remove(pos);
        }
        self.page_image_cache.push_front(image);
        while self.page_image_cache.len() > MAX {
            self.page_image_cache.pop_back();
        }
    }

    fn ensure_rendered(
        &mut self,
        ctx: &AppContext,
        engine: &Engine,
        picker: &Picker,
        width: u16,
        height: u16,
    ) {
        let width = width.max(1);
        let height = height.max(1);
        let mode = ctx.settings.reader_mode;
        let text_mode = ctx.settings.reader_text_mode;

        let Some(book) = self.current_book() else {
            self.current_text = None;
            self.last_error = Some("no book".to_string());
            self.render_key = Some(ReaderRenderKey {
                page: self.page,
                mode,
                text_mode,
                width,
                height,
            });
            return;
        };

        let key = ReaderRenderKey {
            page: self.page,
            mode,
            text_mode,
            width,
            height,
        };

        if (self.current_text.is_some() || self.current_image.is_some())
            && self.render_key == Some(key)
        {
            return;
        }

        match mode {
            ReaderMode::Image => {
                let total_start = Instant::now();
                let (font_w_px, font_h_px) = picker.font_size();
                let font_w_px = font_w_px.max(1);
                let font_h_px = font_h_px.max(1);

                let viewport_w_px = u32::from(width).saturating_mul(u32::from(font_w_px)).max(1);
                let viewport_h_px = u32::from(height)
                    .saturating_mul(u32::from(font_h_px))
                    .max(1);

                let fit_page_to_frame = self.image_zoom_percent == 100
                    && self.image_pan_x_px == 0
                    && self.image_pan_y_px == 0;

                let (page_w_pt, page_h_pt) = engine
                    .page_size_points(&book, self.page)
                    .unwrap_or((1.0, 1.0));
                let page_ratio = (page_w_pt as f64 / page_h_pt.max(1.0) as f64).clamp(0.05, 20.0);

                let base_render_width_px = if fit_page_to_frame {
                    let fit_w = (viewport_h_px as f64 * page_ratio).round().max(1.0) as u32;
                    viewport_w_px.min(fit_w)
                } else {
                    viewport_w_px
                };

                let render_width_px = (u64::from(base_render_width_px)
                    .saturating_mul(u64::from(self.image_zoom_percent.max(1))))
                    / 100;
                let render_width_px = render_width_px.clamp(1, i32::MAX as u64) as u32;

                let max_render_pixels = ctx.settings.kitty_image_quality.max_render_pixels().max(1);
                const MAX_RENDER_WIDTH_PX: u32 = 8192;
                let max_width_by_pixels = ((max_render_pixels as f64) * page_ratio)
                    .sqrt()
                    .floor()
                    .max(1.0) as u32;
                let render_width_px = render_width_px
                    .min(MAX_RENDER_WIDTH_PX)
                    .min(max_width_by_pixels)
                    .max(1);

                let need_new_page_image = match self.page_image.as_ref() {
                    None => true,
                    Some(cached) => {
                        cached.page != self.page
                            || cached.zoom_percent != self.image_zoom_percent
                            || cached.render_width_px != render_width_px
                            || cached.font_size != (font_w_px, font_h_px)
                    }
                };

                let mut rasterize_ms: Option<u128> = None;
                if need_new_page_image {
                    if let Some(pos) = self.page_image_cache.iter().position(|c| {
                        c.page == self.page
                            && c.zoom_percent == self.image_zoom_percent
                            && c.render_width_px == render_width_px
                            && c.font_size == (font_w_px, font_h_px)
                    }) && let Some(cached) = self.page_image_cache.remove(pos)
                    {
                        self.page_image = Some(cached);
                    } else {
                        let rasterize_start = Instant::now();
                        match render_page_image(engine, &book, self.page, render_width_px) {
                            Ok(image) => {
                                let cached = CachedPageImage {
                                    page: self.page,
                                    zoom_percent: self.image_zoom_percent,
                                    render_width_px,
                                    font_size: (font_w_px, font_h_px),
                                    image: Arc::new(image),
                                };
                                self.cache_page_image(cached.clone());
                                self.page_image = Some(cached);
                                rasterize_ms = Some(rasterize_start.elapsed().as_millis());
                            }
                            Err(err) => {
                                self.page_image = None;
                                let fallback = engine
                                    .render_page_text(&book, self.page)
                                    .unwrap_or_else(|_| "no text found".to_string());
                                self.current_text = Some(format!(
                                    "(image render failed; showing text)\n(error: {err})\n\n{fallback}"
                                ));
                                self.current_image = None;
                                self.last_error = None;
                                self.render_key = Some(key);
                                return;
                            }
                        }
                    }
                }

                let size = Rect::new(0, 0, width, height);
                let protocol_start = Instant::now();
                let mut downscale_ms = 0;
                let (protocol_result, viewport_ms, transmit_px) = if fit_page_to_frame {
                    let cached = match self.page_image.as_ref() {
                        Some(cached) => cached,
                        None => {
                            self.current_text = Some("no image cached".to_string());
                            self.current_image = None;
                            self.last_error = None;
                            self.render_key = Some(key);
                            return;
                        }
                    };
                    let proto = picker.new_protocol(
                        (*cached.image).clone(),
                        size,
                        Resize::Fit(Some(image::imageops::FilterType::Triangle)),
                    );
                    let (w, h) = (cached.image.width(), cached.image.height());
                    (proto, 0, (w, h))
                } else {
                    let viewport_start = Instant::now();
                    let (view_image, pan_x_px, pan_y_px) = {
                        let cached = match self.page_image.as_ref() {
                            Some(cached) => cached,
                            None => {
                                self.current_text = Some("no image cached".to_string());
                                self.current_image = None;
                                self.last_error = None;
                                self.render_key = Some(key);
                                return;
                            }
                        };
                        build_viewport_image(
                            cached.image.as_ref(),
                            viewport_w_px,
                            viewport_h_px,
                            self.image_pan_x_px,
                            self.image_pan_y_px,
                        )
                    };
                    let viewport_ms = viewport_start.elapsed().as_millis();
                    self.image_pan_x_px = pan_x_px;
                    self.image_pan_y_px = pan_y_px;

                    let kitty_ok = image_protocol::kitty_supported(picker);
                    let image_ok = image_protocol::image_supported(picker);
                    let max_transmit_px = ctx.settings.kitty_image_quality.max_transmit_pixels();
                    let (transmit_image, transmit_px) = {
                        let px = u64::from(view_image.width())
                            .saturating_mul(u64::from(view_image.height()));
                        if image_ok && px > max_transmit_px {
                            let scale = (max_transmit_px as f64 / px.max(1) as f64)
                                .sqrt()
                                .clamp(0.01, 1.0);
                            let new_w =
                                ((view_image.width() as f64) * scale).round().max(1.0) as u32;
                            let new_h =
                                ((view_image.height() as f64) * scale).round().max(1.0) as u32;
                            let downscale_start = Instant::now();
                            let resized = view_image.resize_exact(
                                new_w,
                                new_h,
                                image::imageops::FilterType::Triangle,
                            );
                            downscale_ms = downscale_start.elapsed().as_millis();
                            (resized, (new_w, new_h))
                        } else {
                            let w = view_image.width();
                            let h = view_image.height();
                            (view_image, (w, h))
                        }
                    };

                    let proto = if kitty_ok {
                        let cols = u16::try_from(
                            (transmit_px
                                .0
                                .saturating_add(u32::from(font_w_px).saturating_sub(1)))
                                / u32::from(font_w_px),
                        )
                        .unwrap_or(width)
                        .max(1)
                        .min(width);
                        let rows = u16::try_from(
                            (transmit_px
                                .1
                                .saturating_add(u32::from(font_h_px).saturating_sub(1)))
                                / u32::from(font_h_px),
                        )
                        .unwrap_or(height)
                        .max(1)
                        .min(height);
                        let kitty_area = Rect::new(0, 0, cols, rows);

                        let id = self.next_kitty_image_id;
                        self.next_kitty_image_id = self.next_kitty_image_id.wrapping_add(1).max(1);
                        let is_tmux = std::env::var_os("TMUX").is_some();
                        Kitty::new(transmit_image, kitty_area, id, is_tmux)
                            .map(ImageProtocol::Kitty)
                    } else {
                        picker.new_protocol(transmit_image, size, Resize::Fit(None))
                    };
                    (proto, viewport_ms, transmit_px)
                };

                match protocol_result {
                    Ok(protocol) => {
                        let protocol_ms = protocol_start.elapsed().as_millis();
                        self.current_text = None;
                        self.current_image = Some(protocol);
                        self.last_error = None;
                        self.last_image_timings = Some(ReaderImageTimings {
                            total_ms: total_start.elapsed().as_millis(),
                            rasterize_ms,
                            viewport_ms,
                            downscale_ms,
                            protocol_ms,
                            viewport_px: (viewport_w_px, viewport_h_px),
                            transmit_px,
                            render_width_px,
                        });
                    }
                    Err(err) => {
                        let fallback = engine
                            .render_page_text(&book, self.page)
                            .unwrap_or_else(|_| "no text found".to_string());
                        let protocol_ms = protocol_start.elapsed().as_millis();
                        self.current_text = Some(format!(
                            "(image protocol failed; showing text)\n(error: {err})\n\n{fallback}"
                        ));
                        self.current_image = None;
                        self.last_error = None;
                        self.last_image_timings = Some(ReaderImageTimings {
                            total_ms: total_start.elapsed().as_millis(),
                            rasterize_ms,
                            viewport_ms,
                            downscale_ms,
                            protocol_ms,
                            viewport_px: (viewport_w_px, viewport_h_px),
                            transmit_px,
                            render_width_px,
                        });
                    }
                }
            }
            _ => {
                let furniture = if mode == ReaderMode::Text
                    && text_mode != ReaderTextMode::Raw
                    && ctx.settings.reader_trim_headers_footers
                {
                    if self.page_furniture.is_none() {
                        self.page_furniture = engine.detect_page_furniture(&book).ok();
                    }
                    self.page_furniture.as_ref()
                } else {
                    None
                };

                match engine.render_page_for_reader(
                    &book, self.page, mode, text_mode, furniture, width, height,
                ) {
                    Ok(text) => {
                        let text = if is_non_text_page(&text) {
                            let image_ok = image_protocol::image_supported(picker);
                            let hint = if image_ok {
                                "image/chart (m: image mode)"
                            } else {
                                "image/chart (k: kitty-reader)"
                            };
                            non_text_placeholder(width, height, hint)
                        } else {
                            match text_mode {
                                ReaderTextMode::Raw => text,
                                ReaderTextMode::Wrap => {
                                    wrap_preserving_lines(&text, width as usize)
                                }
                                ReaderTextMode::Reflow => wrap_reflow_text(&text, width as usize),
                            }
                        };
                        let lines = text.lines().count() as u16;
                        if lines == 0 {
                            self.scroll = 0;
                        } else {
                            self.scroll = self.scroll.min(lines.saturating_sub(1));
                        }
                        self.current_text = Some(text);
                        self.current_image = None;
                        self.last_error = None;
                        self.last_image_timings = None;
                    }
                    Err(err) => {
                        self.current_text = None;
                        self.current_image = None;
                        self.last_error = Some(err.to_string());
                        self.last_image_timings = None;
                    }
                }
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

    fn pan_image_by_cells(&mut self, picker: &Picker, dx_cols: i32, dy_rows: i32) {
        let (font_w_px, font_h_px) = picker.font_size();
        let font_w_px = i32::from(font_w_px.max(1));
        let font_h_px = i32::from(font_h_px.max(1));
        self.pan_image_by_pixels(
            dx_cols.saturating_mul(font_w_px),
            dy_rows.saturating_mul(font_h_px),
        );
    }

    fn pan_image_by_pixels(&mut self, dx_px: i32, dy_px: i32) {
        self.image_pan_x_px = add_signed_u32(self.image_pan_x_px, dx_px);
        self.image_pan_y_px = add_signed_u32(self.image_pan_y_px, dy_px);
        self.current_image = None;
        self.render_key = None;
    }

    fn zoom_image_in(&mut self) {
        const MAX: u16 = 400;
        const STEP: u16 = 25;
        let zoom = self.image_zoom_percent.saturating_add(STEP).min(MAX);
        self.set_image_zoom_percent(zoom);
    }

    fn zoom_image_out(&mut self) {
        const MIN: u16 = 50;
        const STEP: u16 = 25;
        let zoom = self.image_zoom_percent.saturating_sub(STEP).max(MIN);
        self.set_image_zoom_percent(zoom);
    }

    fn reset_image_view(&mut self) {
        self.image_pan_x_px = 0;
        self.image_pan_y_px = 0;
        self.set_image_zoom_percent(100);
        self.notice = Some("zoom: 100%".to_string());
    }

    fn set_image_zoom_percent(&mut self, zoom_percent: u16) {
        const MIN: u16 = 50;
        const MAX: u16 = 400;
        let zoom_percent = zoom_percent.clamp(MIN, MAX);
        if zoom_percent == self.image_zoom_percent {
            return;
        }
        self.image_zoom_percent = zoom_percent;
        self.image_pan_x_px = 0;
        self.image_pan_y_px = 0;
        self.page_image = None;
        self.current_image = None;
        self.render_key = None;
        self.notice = Some(format!("zoom: {zoom_percent}%"));
    }
}

fn add_signed_u32(value: u32, delta: i32) -> u32 {
    if delta >= 0 {
        value.saturating_add(delta as u32)
    } else {
        value.saturating_sub(delta.unsigned_abs())
    }
}

fn render_page_image(
    engine: &Engine,
    book: &bookshelf_core::Book,
    page_index: u32,
    target_width_px: u32,
) -> anyhow::Result<image::DynamicImage> {
    let target_width_px = i32::try_from(target_width_px.clamp(1, i32::MAX as u32))
        .unwrap_or(i32::MAX)
        .max(1);
    let bitmap = engine.render_page_bitmap_rgba(book, page_index, target_width_px, i32::MAX)?;
    let image =
        image::RgbaImage::from_raw(bitmap.width as u32, bitmap.height as u32, bitmap.pixels)
            .ok_or_else(|| anyhow::anyhow!("invalid RGBA pixel buffer from pdfium"))?;
    Ok(image::DynamicImage::ImageRgba8(image))
}

fn build_viewport_image(
    full: &image::DynamicImage,
    viewport_w_px: u32,
    viewport_h_px: u32,
    pan_x_px: u32,
    pan_y_px: u32,
) -> (image::DynamicImage, u32, u32) {
    let viewport_w_px = viewport_w_px.max(1);
    let viewport_h_px = viewport_h_px.max(1);
    let img_w = full.width();
    let img_h = full.height();

    let max_pan_x = img_w.saturating_sub(viewport_w_px);
    let max_pan_y = img_h.saturating_sub(viewport_h_px);

    let pan_x_px = pan_x_px.min(max_pan_x);
    let pan_y_px = pan_y_px.min(max_pan_y);

    let mut viewport: image::DynamicImage = image::ImageBuffer::from_pixel(
        viewport_w_px,
        viewport_h_px,
        image::Rgba([255u8, 255u8, 255u8, 255u8]),
    )
    .into();

    let crop_w = viewport_w_px.min(img_w.saturating_sub(pan_x_px));
    let crop_h = viewport_h_px.min(img_h.saturating_sub(pan_y_px));
    if crop_w > 0 && crop_h > 0 {
        let region = full.crop_imm(pan_x_px, pan_y_px, crop_w, crop_h);

        let dest_x = if img_w < viewport_w_px {
            i64::from((viewport_w_px - img_w) / 2)
        } else {
            0
        };
        let dest_y = if img_h < viewport_h_px {
            i64::from((viewport_h_px - img_h) / 2)
        } else {
            0
        };
        image::imageops::overlay(&mut viewport, &region, dest_x, dest_y);
    }

    (viewport, pan_x_px, pan_y_px)
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

fn matches_collection_filter(filter: &CollectionFilter, book_collection: Option<&str>) -> bool {
    match filter {
        CollectionFilter::Any => true,
        CollectionFilter::None => book_collection.is_none(),
        CollectionFilter::Selected(wanted) => book_collection
            .as_ref()
            .is_some_and(|c| c.eq_ignore_ascii_case(wanted)),
    }
}

fn matches_tag_filter(selected: &[String], mode: TagMatchMode, book_tags: &[String]) -> bool {
    if selected.is_empty() {
        return true;
    }
    if book_tags.is_empty() {
        return false;
    }

    match mode {
        TagMatchMode::And => selected.iter().all(|t| {
            let t = t.trim();
            !t.is_empty() && book_tags.iter().any(|bt| bt.eq_ignore_ascii_case(t))
        }),
        TagMatchMode::Or => selected.iter().any(|t| {
            let t = t.trim();
            !t.is_empty() && book_tags.iter().any(|bt| bt.eq_ignore_ascii_case(t))
        }),
    }
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

fn wrap_preserving_lines(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return text.to_string();
    }

    let mut out_lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            out_lines.push(String::new());
            continue;
        }

        if looks_preformatted(line) {
            out_lines.push(line.to_string());
            continue;
        }

        out_lines.extend(wrap_text(line, max_width));
    }

    while out_lines.last().is_some_and(|l| l.is_empty()) {
        out_lines.pop();
    }

    out_lines.join("\n")
}

fn wrap_reflow_text(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return text.to_string();
    }

    let mut out_lines: Vec<String> = Vec::new();
    let mut paragraph = String::new();

    let flush_paragraph = |out_lines: &mut Vec<String>, paragraph: &mut String| {
        let para = paragraph.trim();
        if para.is_empty() {
            paragraph.clear();
            return;
        }
        out_lines.extend(wrap_text(para, max_width));
        paragraph.clear();
    };

    for line in text.lines() {
        if line.trim().is_empty() {
            flush_paragraph(&mut out_lines, &mut paragraph);
            if !out_lines.last().is_some_and(|l| l.is_empty()) {
                out_lines.push(String::new());
            }
            continue;
        }

        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(line.trim());
    }

    flush_paragraph(&mut out_lines, &mut paragraph);
    while out_lines.last().is_some_and(|l| l.is_empty()) {
        out_lines.pop();
    }

    out_lines.join("\n")
}

fn looks_preformatted(line: &str) -> bool {
    line.contains('\t') || line.contains("  ")
}

fn is_non_text_page(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("no text found")
}

fn non_text_placeholder(width: u16, height: u16, label: &str) -> String {
    let width = width.max(10);
    let height = height.max(5);
    let inner_w = (width - 2) as usize;
    let inner_h = (height - 2) as usize;

    let mut out = String::new();
    out.push('┌');
    out.push_str(&"─".repeat(inner_w));
    out.push('┐');

    let label = label.trim();
    let label = if label.is_empty() {
        "image/chart"
    } else {
        label
    };

    for y in 0..inner_h {
        out.push('\n');
        out.push('│');
        if y == inner_h / 2 {
            let mut label = label.to_string();
            if label.chars().count() > inner_w {
                label = label.chars().take(inner_w).collect();
            }
            let label_len = label.chars().count();
            let pad_left = inner_w.saturating_sub(label_len) / 2;
            let pad_right = inner_w.saturating_sub(label_len).saturating_sub(pad_left);
            out.push_str(&"░".repeat(pad_left));
            out.push_str(&label);
            out.push_str(&"░".repeat(pad_right));
        } else {
            out.push_str(&"░".repeat(inner_w));
        }
        out.push('│');
    }

    out.push('\n');
    out.push('└');
    out.push_str(&"─".repeat(inner_w));
    out.push('┘');
    out
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
        Style::default().fg(Color::Gray)
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
