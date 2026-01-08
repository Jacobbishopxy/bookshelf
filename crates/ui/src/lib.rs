//! ratatui-based UI.

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Context as _;
use bookshelf_application::AppContext;
use bookshelf_core::Settings;
use bookshelf_engine::Engine;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{event, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Terminal;

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
    preview_panel: PreviewPanel,
    scan_panel: ScanPathPanel,
    engine: Engine,
}

impl Ui {
    pub fn new(mut ctx: AppContext) -> Self {
        ctx.settings.normalize();
        let preview_panel = PreviewPanel::new(ctx.settings.clone());
        let scan_panel = ScanPathPanel::new(join_roots(&ctx.settings));
        Self {
            ctx,
            preview_panel,
            scan_panel,
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

                    if self.scan_panel.open {
                        if let Some(exit) = self.handle_scan_panel_key(key.code)? {
                            return Ok(UiOutcome {
                                ctx: self.ctx.clone(),
                                exit,
                            });
                        }
                    } else if self.preview_panel.open {
                        if let Some(exit) = self.handle_preview_panel_key(key.code)? {
                            return Ok(UiOutcome {
                                ctx: self.ctx.clone(),
                                exit,
                            });
                        }
                    } else if let Some(exit) = self.handle_main_key(key.code)? {
                        return Ok(UiOutcome {
                            ctx: self.ctx.clone(),
                            exit,
                        });
                    }
                }
            }
        }
    }

    fn handle_main_key(&mut self, code: KeyCode) -> anyhow::Result<Option<UiExit>> {
        match code {
            KeyCode::Char('q') => Ok(Some(UiExit::Quit)),
            KeyCode::Char('o') => {
                self.scan_panel.open = true;
                self.scan_panel.input = join_roots(&self.ctx.settings);
                self.scan_panel.error = None;
                Ok(None)
            }
            KeyCode::Char('p') => {
                self.preview_panel.open = true;
                self.preview_panel.draft = self.ctx.settings.clone();
                Ok(None)
            }
            KeyCode::Char('m') => {
                self.ctx.settings.cycle_preview_mode();
                Ok(None)
            }
            KeyCode::Char('s') => {
                self.ctx.settings.cycle_scan_scope();
                Ok(None)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.ctx.books.is_empty() {
                    self.ctx.selected = (self.ctx.selected + 1).min(self.ctx.books.len() - 1);
                }
                Ok(None)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.ctx.selected = self.ctx.selected.saturating_sub(1);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_preview_panel_key(&mut self, code: KeyCode) -> anyhow::Result<Option<UiExit>> {
        match code {
            KeyCode::Esc => {
                self.preview_panel.open = false;
                Ok(None)
            }
            KeyCode::Enter => {
                self.ctx.settings = self.preview_panel.draft.clone();
                self.ctx.settings.normalize();
                self.preview_panel.open = false;
                Ok(None)
            }
            KeyCode::Char('q') => Ok(Some(UiExit::Quit)),
            KeyCode::Char('m') | KeyCode::Tab => {
                self.preview_panel.draft.cycle_preview_mode();
                Ok(None)
            }
            KeyCode::Char('s') => {
                self.preview_panel.draft.cycle_scan_scope();
                Ok(None)
            }
            KeyCode::Char('+') | KeyCode::Up => {
                self.preview_panel.draft.preview_depth =
                    self.preview_panel.draft.preview_depth.saturating_add(1);
                self.preview_panel.draft.normalize();
                Ok(None)
            }
            KeyCode::Char('-') | KeyCode::Down => {
                self.preview_panel.draft.preview_depth =
                    self.preview_panel.draft.preview_depth.saturating_sub(1);
                self.preview_panel.draft.normalize();
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_scan_panel_key(&mut self, code: KeyCode) -> anyhow::Result<Option<UiExit>> {
        match code {
            KeyCode::Esc => {
                self.scan_panel.open = false;
                self.scan_panel.error = None;
                Ok(None)
            }
            KeyCode::Enter | KeyCode::Char('o') => {
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
                self.scan_panel.input.pop();
                Ok(None)
            }
            KeyCode::Char('c') => {
                self.scan_panel.input.clear();
                Ok(None)
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
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

        frame.render_widget(self.draw_library(), body_layout[0]);
        frame.render_widget(self.draw_details(), body_layout[1]);

        let footer = Paragraph::new(Line::from(vec![
            Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" quit  "),
            Span::styled("j/k", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" move  "),
            Span::styled("o", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" scan paths  "),
            Span::styled("p", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" preview panel  "),
            Span::styled("m", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" cycle preview  "),
            Span::styled("s", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" toggle scan"),
        ]))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::TOP));
        frame.render_widget(footer, layout[2]);

        if self.preview_panel.open {
            self.draw_preview_panel(area, frame);
        }

        if self.scan_panel.open {
            self.draw_scan_panel(area, frame);
        }
    }

    fn draw_library(&self) -> Paragraph<'static> {
        let mut lines = Vec::new();
        if self.ctx.books.is_empty() {
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
        } else {
            for (idx, book) in self.ctx.books.iter().enumerate() {
                let is_selected = idx == self.ctx.selected;
                let style = if is_selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::styled(book.title.clone(), style));
            }
        }

        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title("Library"))
            .wrap(Wrap { trim: true })
    }

    fn draw_details(&self) -> Paragraph<'static> {
        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("Preview: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.preview_mode.to_string()),
            Span::raw("  "),
            Span::styled("Depth: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.settings.preview_depth.to_string()),
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
            lines.push(Line::from(vec![
                Span::styled("Selected: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(book.title.clone()),
            ]));
            lines.push(Line::raw(book.path.clone()));
            lines.push(Line::raw(""));

            let preview = self.engine.render_preview_for(book, &self.ctx.settings);
            for line in preview.lines().take(20) {
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

        let mode = Line::from(vec![
            Span::styled("Preview mode: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                self.preview_panel.draft.preview_mode.to_string(),
                Style::default().fg(Color::Cyan),
            ),
        ]);

        let depth = Line::from(vec![
            Span::styled("Preview depth: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                self.preview_panel.draft.preview_depth.to_string(),
                Style::default().fg(Color::Cyan),
            ),
        ]);

        let scope = Line::from(vec![
            Span::styled("Scan scope: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                self.preview_panel.draft.scan_scope.to_string(),
                Style::default().fg(Color::Cyan),
            ),
        ]);

        let help = vec![
            Line::raw(""),
            Line::from(vec![
                Span::styled("Tab/m", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cycle preview mode"),
            ]),
            Line::from(vec![
                Span::styled("s", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" toggle scan scope"),
            ]),
            Line::from(vec![
                Span::styled("+/- or ↑/↓", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" adjust depth"),
            ]),
            Line::from(vec![
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" apply  "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cancel"),
            ]),
        ];

        let text = Text::from([vec![mode, depth, scope], help].concat());

        let panel = Paragraph::new(text)
            .block(block)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true });

        frame.render_widget(panel, popup_area);
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

        let mut lines = Vec::new();
        lines.push(Line::raw("Enter scan paths (separate with ';' or ','):"));
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.scan_panel.input.clone()),
        ]));
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("cwd: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(self.ctx.cwd.clone()),
        ]));
        lines.push(Line::raw(""));

        if let Some(err) = &self.scan_panel.error {
            lines.push(Line::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
        }

        lines.push(Line::from(vec![
            Span::styled("o/Enter", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" apply + rescan  "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel  "),
            Span::styled("Backspace", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" delete  "),
            Span::styled("c", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" clear"),
        ]));

        let panel = Paragraph::new(Text::from(lines))
            .block(block)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false });
        frame.render_widget(panel, popup_area);
    }
}

#[derive(Debug, Clone)]
struct PreviewPanel {
    open: bool,
    draft: Settings,
}

impl PreviewPanel {
    fn new(settings: Settings) -> Self {
        Self {
            open: false,
            draft: settings,
        }
    }
}

#[derive(Debug, Clone)]
struct ScanPathPanel {
    open: bool,
    input: String,
    error: Option<String>,
}

impl ScanPathPanel {
    fn new(input: String) -> Self {
        Self {
            open: false,
            input,
            error: None,
        }
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
