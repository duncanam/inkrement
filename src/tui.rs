use color_eyre::eyre::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Gauge, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState,
    },
};

/// Which tab is active
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    GetPrs,
    PublishReviews,
}

/// State of a PR row in the Get tab
#[derive(Clone, Copy, PartialEq, Eq)]
enum PrStatus {
    /// Available to select for upload
    Available,
    /// Selected for upload this session
    Queued,
    /// Already on the reMarkable
    Uploaded,
}

/// A row in the PR table
struct PrRow {
    number: u32,
    repo: String,
    short_sha: String,
    added: usize,
    removed: usize,
    status: PrStatus,
}

/// Progress state for the bottom bar
struct Progress {
    message: String,
    current: usize,
    total: usize,
}

/// Main application state
pub(crate) struct App {
    tab: Tab,
    prs: Vec<PrRow>,
    table_state: TableState,
    progress: Option<Progress>,
    should_quit: bool,
}

impl App {
    /// Create a new app with mock data for now
    pub(crate) fn new() -> Self {
        let prs = vec![
            PrRow {
                number: 72,
                repo: "Atomic-Industries/crab-rave".to_string(),
                short_sha: "747e7fe8".to_string(),
                added: 3525,
                removed: 634,
                status: PrStatus::Uploaded,
            },
            PrRow {
                number: 1,
                repo: "Atomic-Industries/angstrom".to_string(),
                short_sha: "651164df".to_string(),
                added: 120,
                removed: 0,
                status: PrStatus::Available,
            },
            PrRow {
                number: 15,
                repo: "Atomic-Industries/core-lib".to_string(),
                short_sha: "a3f2b1c9".to_string(),
                added: 45,
                removed: 12,
                status: PrStatus::Queued,
            },
        ];

        let mut table_state = TableState::default();
        table_state.select(Some(0));

        Self {
            tab: Tab::GetPrs,
            prs,
            table_state,
            progress: None,
            should_quit: false,
        }
    }

    /// Run the TUI event loop
    pub(crate) fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal
                .draw(|frame| self.render(frame))
                .wrap_err("failed to draw frame")?;
            self.handle_events().wrap_err("failed to handle events")?;
        }
        Ok(())
    }

    fn handle_events(&mut self) -> Result<()> {
        let Event::Key(key) = event::read().wrap_err("failed to read event")? else {
            return Ok(());
        };

        // Only handle key press events, not release
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char(' ') => self.toggle_selected(),
            KeyCode::Char('a') => self.select_all(),
            KeyCode::Tab => self.next_tab(),
            KeyCode::Enter => self.execute(),
            _ => {}
        }

        Ok(())
    }

    fn move_cursor(&mut self, delta: i32) {
        if self.prs.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, self.prs.len() as i32 - 1) as usize;
        self.table_state.select(Some(next));
    }

    fn toggle_selected(&mut self) {
        let Some(idx) = self.table_state.selected() else {
            return;
        };
        let pr = &mut self.prs[idx];
        if pr.status == PrStatus::Uploaded {
            return; // Can't toggle uploaded PRs
        }
        pr.status = match pr.status {
            PrStatus::Available => PrStatus::Queued,
            PrStatus::Queued => PrStatus::Available,
            PrStatus::Uploaded => PrStatus::Uploaded,
        };
    }

    fn select_all(&mut self) {
        for pr in &mut self.prs {
            if pr.status == PrStatus::Available {
                pr.status = PrStatus::Queued;
            }
        }
    }

    fn next_tab(&mut self) {
        self.tab = match self.tab {
            Tab::GetPrs => Tab::PublishReviews,
            Tab::PublishReviews => Tab::GetPrs,
        };
    }

    fn execute(&mut self) {
        // TODO: spawn background worker for queued uploads
    }

    // === Rendering ===

    fn render(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // version bar
                Constraint::Length(2), // tabs
                Constraint::Min(5),   // main content
                Constraint::Length(3), // legend + keybinds
                Constraint::Length(2), // progress bar
            ])
            .split(frame.area());

        self.render_header(frame, chunks[0]);
        self.render_tabs(frame, chunks[1]);
        self.render_main(frame, chunks[2]);
        self.render_footer(frame, chunks[3]);
        self.render_progress(frame, chunks[4]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                "inkrement v0.1",
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .alignment(ratatui::layout::Alignment::Right);
        frame.render_widget(header, area);
    }

    fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let tab_names = ["Get PRs", "Publish Reviews"];
        let selected = match self.tab {
            Tab::GetPrs => 0,
            Tab::PublishReviews => 1,
        };

        let spans: Vec<Span> = tab_names
            .iter()
            .enumerate()
            .flat_map(|(i, name)| {
                let style = if i == selected {
                    Style::default().fg(Color::White).bold().add_modifier(Modifier::UNDERLINED)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let mut items = vec![Span::styled(*name, style)];
                if i < tab_names.len() - 1 {
                    items.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                }
                items
            })
            .collect();

        let tabs = Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(tabs, area);
    }

    fn render_main(&mut self, frame: &mut Frame, area: Rect) {
        match self.tab {
            Tab::GetPrs => self.render_get_tab(frame, area),
            Tab::PublishReviews => self.render_publish_tab(frame, area),
        }
    }

    fn render_get_tab(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(1), // legend
                Constraint::Min(3),   // table
            ])
            .split(area);

        // Section title
        let title = Paragraph::new("Pull Requests Awaiting Review")
            .style(Style::default().bold())
            .centered();
        frame.render_widget(title, chunks[0]);

        // Legend
        let legend = Paragraph::new(Line::from(vec![
            Span::styled(" [✓]", Style::default().fg(Color::Green)),
            Span::raw(" on reMarkable  "),
            Span::styled("[x]", Style::default().fg(Color::Yellow)),
            Span::raw(" queued for upload"),
        ]));
        frame.render_widget(legend, chunks[1]);

        // Table
        let header = Row::new(vec!["", "#", "Repository", "SHA", "+", "-"])
            .style(Style::default().fg(Color::Gray))
            .bottom_margin(1);

        let rows: Vec<Row> = self
            .prs
            .iter()
            .map(|pr| {
                let checkbox = match pr.status {
                    PrStatus::Uploaded => Cell::from("[✓]").style(Style::default().fg(Color::Green)),
                    PrStatus::Queued => Cell::from("[x]").style(Style::default().fg(Color::Yellow)),
                    PrStatus::Available => Cell::from("[ ]"),
                };

                let style = if pr.status == PrStatus::Uploaded {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    checkbox,
                    Cell::from(format!("#{}", pr.number)),
                    Cell::from(pr.repo.as_str()),
                    Cell::from(pr.short_sha.as_str()),
                    Cell::from(format!("+{}", pr.added))
                        .style(Style::default().fg(Color::Green)),
                    Cell::from(format!("-{}", pr.removed))
                        .style(Style::default().fg(Color::Red)),
                ])
                .style(style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(5),  // checkbox
                Constraint::Length(6),  // number
                Constraint::Min(20),   // repo
                Constraint::Length(10), // sha
                Constraint::Length(8),  // added
                Constraint::Length(8),  // removed
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(table, chunks[2], &mut self.table_state);

        // Scrollbar
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state = ScrollbarState::new(self.prs.len())
            .position(self.table_state.selected().unwrap_or(0));
        frame.render_stateful_widget(
            scrollbar,
            chunks[2].inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }

    fn render_publish_tab(&self, frame: &mut Frame, area: Rect) {
        let placeholder = Paragraph::new("  Coming soon...")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL).title(" Publish Reviews "));
        frame.render_widget(placeholder, area);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let legend = Line::from(vec![
            Span::styled("[space]", Style::default().bold()),
            Span::raw(" toggle  "),
            Span::styled("[a]", Style::default().bold()),
            Span::raw(" all  "),
            Span::styled("[enter]", Style::default().bold()),
            Span::raw(" upload  "),
            Span::styled("[r]", Style::default().bold()),
            Span::raw(" refresh  "),
            Span::styled("[tab]", Style::default().bold()),
            Span::raw(" switch  "),
            Span::styled("[q]", Style::default().bold()),
            Span::raw(" quit"),
        ]);
        let footer = Paragraph::new(legend).centered();
        frame.render_widget(footer, area);
    }

    fn render_progress(&self, frame: &mut Frame, area: Rect) {
        match &self.progress {
            Some(progress) => {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(1), Constraint::Length(1)])
                    .split(area);

                let status = Paragraph::new(format!("  {}", progress.message))
                    .style(Style::default().fg(Color::Cyan));
                frame.render_widget(status, chunks[0]);

                let ratio = if progress.total > 0 {
                    progress.current as f64 / progress.total as f64
                } else {
                    0.0
                };
                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(Color::Cyan))
                    .ratio(ratio)
                    .label(format!(
                        "{}/{} steps",
                        progress.current, progress.total
                    ));
                frame.render_widget(gauge, chunks[1]);
            }
            None => {
                let empty = Paragraph::new("");
                frame.render_widget(empty, area);
            }
        }
    }
}
