//! ratatui-based UI.

use std::io::{self, Stdout};
use std::hash::Hasher;
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use bookshelf_application::AppContext;
use bookshelf_core::{MAX_PREVIEW_DEPTH, MAX_PREVIEW_PAGES, PreviewMode, Settings};
use bookshelf_engine::Engine;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{event, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Terminal;
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
    reader: ReaderPanel,
    engine: Engine,
}

impl Ui {
    pub fn new(mut ctx: AppContext) -> Self {
        ctx.settings.normalize();
        let settings_panel = SettingsPanel::default();
        let preview_panel = PreviewPanel::new(ctx.settings.clone());
        let scan_panel = ScanPathPanel::new(join_roots(&ctx.settings));
        let reader = ReaderPanel::default();
        Self {
            ctx,
            settings_panel,
            preview_panel,
            scan_panel,
            reader,
            engine: Engine::new(),
        }
    }

    pub fn run(&mut self) -> anyhow::Result<UiOutcome> {
        let mut terminal = setup_terminal()?;
        let result = self.event_loop(&mut terminal);
        restore_terminal(&mut terminal)?;
        result
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> anyhow::Result<UiOutcome> {
        let tick_rate = Duration::from_millis(250);

        loop {
            terminal.draw(|frame| self.draw(frame.area(), frame))?;

            if event::poll(tick_rate)? {
                if let Event::Key(key) = event::read()? {
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
    }

    fn handle_main_key(&mut self, key: KeyEvent) -> anyhow::Result<Option<UiExit>> {
        match key.code {
            KeyCode::Esc => Ok(Some(UiExit::Quit)),
            KeyCode::Char('s') => {
                self.settings_panel.open = true;
                self.settings_panel.selected = 0;
                Ok(None)
            }
            KeyCode::Enter => {
                if let Some(book) = self.ctx.books.get(self.ctx.selected).cloned() {
                    self.reader.open_book(&book, &self.ctx, &self.engine);
                }
                Ok(None)
            }
            KeyCode::Down => {
                if !self.ctx.books.is_empty() {
                    self.ctx.selected = (self.ctx.selected + 1).min(self.ctx.books.len() - 1);
                }
                Ok(None)
            }
            KeyCode::Up => {
                self.ctx.selected = self.ctx.selected.saturating_sub(1);
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
                    self.preview_panel.draft.preview_mode = match self.preview_panel.draft.preview_mode
                    {
                        bookshelf_core::PreviewMode::Text => bookshelf_core::PreviewMode::Text,
                        bookshelf_core::PreviewMode::Braille => bookshelf_core::PreviewMode::Text,
                        bookshelf_core::PreviewMode::Blocks => {
                            bookshelf_core::PreviewMode::Braille
                        }
                    };
                }
                Ok(None)
            }
            KeyCode::Right => {
                if self.preview_panel.selected == 0 {
                    self.preview_panel.draft.preview_mode = match self.preview_panel.draft.preview_mode
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
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') => {
                    self.scan_panel.input.clear();
                    return Ok(None);
                }
                _ => {}
            }
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

    fn draw(
        &mut self,
        area: Rect,
        frame: &mut ratatui::Frame,
    ) {
        frame.render_widget(Clear, area);
        if self.reader.open {
            self.draw_reader(area, frame);
            return;
        }

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(2)])
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

        let footer = Paragraph::new(Line::from(vec![
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" quit  "),
            Span::styled("↑/↓", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" move  "),
            Span::styled("s", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" settings  "),
            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" read"),
        ]))
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
    }

    fn draw_reader(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(2)])
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
            Span::styled("d", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" dump"),
        ];

        if let Some(note) = &self.reader.notice {
            footer_spans.push(Span::raw("  |  "));
            footer_spans.push(Span::styled(
                note.clone(),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
        }

        let footer = Paragraph::new(Line::from(footer_spans))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::TOP));
        frame.render_widget(footer, layout[2]);
    }

    fn draw_settings_panel(&self, area: Rect, frame: &mut ratatui::Frame) {
        let popup_area = centered_rect(45, 30, area);
        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
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
        let block = Block::default().borders(Borders::ALL).title("Library");

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

        let max_title_width = area.width.saturating_sub(6) as usize;
        let items: Vec<ListItem> = self
            .ctx
            .books
            .iter()
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
        state.select(self.ctx.books.get(self.ctx.selected).map(|_| self.ctx.selected));
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
            let preview_width = area.width.saturating_sub(2);
            lines.push(Line::from(vec![
                Span::styled("Selected: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(book.title.clone()),
            ]));
            lines.push(Line::raw(book.path.clone()));
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

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
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
                Span::styled("Preview mode: ", Style::default().add_modifier(Modifier::BOLD)),
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
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
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
                Span::styled("Scan scope: ", Style::default().add_modifier(Modifier::BOLD)),
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
        if let Some(total) = self.total_pages {
            if total > 0 {
                self.page = self.page.min(total.saturating_sub(1));
            }
        }
        self.invalidate_render();
    }

    fn current_book(&self) -> Option<bookshelf_core::Book> {
        Some(bookshelf_core::Book {
            path: self.book_path.clone()?,
            title: self.book_title.clone()?,
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
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leave alt screen")?;
    Ok(())
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
        .split(|ch| ch == ';' || ch == ',')
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
