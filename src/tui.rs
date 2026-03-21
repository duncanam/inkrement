use color_eyre::eyre::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use itertools::Itertools;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Gauge, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState,
    },
};

// === Layout constants ===

/// Top-level vertical layout: version bar, tabs, main content, keybinds, progress.
const MAIN_LAYOUT: [Constraint; 5] = [
    Constraint::Length(1), // version bar
    Constraint::Length(2), // tabs
    Constraint::Min(5),    // main content
    Constraint::Length(3), // keybinds
    Constraint::Length(2), // progress bar
];

/// Get tab vertical layout: title, legend, table.
const GET_TAB_LAYOUT: [Constraint; 3] = [
    Constraint::Length(2), // title
    Constraint::Length(1), // legend
    Constraint::Min(3),    // table
];

/// PR table column widths.
const PR_TABLE_COLUMNS: [Constraint; 6] = [
    Constraint::Length(5),  // checkbox
    Constraint::Length(6),  // number
    Constraint::Min(20),    // repo
    Constraint::Length(10), // sha
    Constraint::Length(8),  // added
    Constraint::Length(8),  // removed
];

/// Progress bar layout: status line + gauge.
const PROGRESS_LAYOUT: [Constraint; 2] = [Constraint::Length(1), Constraint::Length(1)];

// === Tab configuration ===

/// Tab display names, in order.
const TAB_NAMES: [&str; 2] = ["Get PRs", "Publish Reviews"];

/// Accent colors for each tab, matching TAB_NAMES order.
const TAB_COLORS: [Color; 2] = [Color::Green, Color::Blue];

/// PR table column headers.
const PR_TABLE_HEADERS: [&str; 6] = ["", "#", "Repository", "SHA", "+", "-"];

/// Version string shown in the top-right corner.
const VERSION_LABEL: &str = concat!("inkrement v", env!("CARGO_PKG_VERSION"));

/// Section title for the Get tab.
const GET_TAB_TITLE: &str = "Pull Requests Awaiting Review";

/// The active tab in the TUI.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// Fetch PRs from GitHub and upload review PDFs to the reMarkable.
    GetPrs,
    /// Download annotated PDFs from the reMarkable and publish reviews to GitHub.
    PublishReviews,
}

/// The upload status of a PR in the Get tab.
///
/// Determines how the row is displayed and whether it can be toggled.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PrStatus {
    /// Not yet selected — can be toggled with space.
    Available,
    /// Marked for upload this session — will be processed on enter.
    Queued,
    /// Already present on the reMarkable — cannot be toggled.
    Uploaded,
}

/// A single row in the PR selection table.
///
/// Represents a pull request awaiting review, along with its upload status.
struct PrRow {
    /// GitHub PR number.
    number: u32,
    /// Repository in `owner/name` format.
    repo: String,
    /// First 8 characters of the head commit SHA.
    short_sha: String,
    /// Total lines added in the diff.
    added: usize,
    /// Total lines removed in the diff.
    removed: usize,
    /// Whether this PR is available, queued, or already uploaded.
    status: PrStatus,
}

impl PrRow {
    /// Render this PR as a table row with colored checkbox and diff stats.
    fn to_row(&self) -> Row<'_> {
        let checkbox = match self.status {
            PrStatus::Uploaded => Cell::from("[✓]").style(Style::default().fg(Color::Green)),
            PrStatus::Queued => Cell::from("[x]").style(Style::default().fg(Color::Yellow)),
            PrStatus::Available => Cell::from("[ ]"),
        };

        let base_style = if self.status == PrStatus::Uploaded {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };

        Row::new(vec![
            checkbox,
            Cell::from(format!("#{}", self.number)),
            Cell::from(self.repo.as_str()),
            Cell::from(self.short_sha.as_str()),
            Cell::from(format!("+{}", self.added)).style(Style::default().fg(Color::Green)),
            Cell::from(format!("-{}", self.removed)).style(Style::default().fg(Color::Red)),
        ])
        .style(base_style)
    }
}

/// Tracks the state of an ongoing background operation (render + upload).
///
/// Displayed as a status message and progress bar at the bottom of the TUI.
struct Progress {
    /// Human-readable description of the current step (e.g. "Rendering #72 crab-rave...").
    message: String,
    /// Number of sub-steps completed so far.
    current: usize,
    /// Total number of sub-steps across all queued PRs.
    total: usize,
}

/// Root application state for the inkrement TUI.
///
/// Owns all data needed to render the interface and handle user input.
/// Constructed once at startup and mutated in response to key events
/// and background worker progress updates.
pub(crate) struct App {
    /// Which tab is currently displayed.
    tab: Tab,
    /// The list of PRs shown in the Get tab.
    prs: Vec<PrRow>,
    /// Ratatui table selection state (tracks cursor position).
    table_state: TableState,
    /// Active progress indicator, if a background operation is running.
    progress: Option<Progress>,
    /// Set to true when the user presses 'q' to exit.
    should_quit: bool,
}

impl App {
    /// Create a new app with mock data for development.
    ///
    /// TODO: Replace with real data from GitHub + reMarkable on startup.
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

    /// Run the TUI event loop.
    ///
    /// Alternates between rendering a frame and blocking on user input.
    /// Returns when the user presses 'q' or an error occurs.
    pub(crate) fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal
                .draw(|frame| self.render(frame))
                .wrap_err("failed to draw frame")?;
            self.handle_events()
                .wrap_err("while drawing TUI, failed to handle events")?;
        }
        Ok(())
    }

    // === Event handling ===

    /// Read and dispatch a single terminal event.
    fn handle_events(&mut self) -> Result<()> {
        let Event::Key(key) = event::read().wrap_err("while drawing TUI, failed to read event")?
        else {
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

    /// Move the table cursor up (negative) or down (positive), clamping to bounds.
    fn move_cursor(&mut self, delta: i32) {
        if self.prs.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, self.prs.len() as i32 - 1) as usize;
        self.table_state.select(Some(next));
    }

    /// Toggle the selected PR between Available and Queued.
    /// Does nothing if the PR is already Uploaded.
    fn toggle_selected(&mut self) {
        let Some(idx) = self.table_state.selected() else {
            return;
        };

        let pr = &mut self.prs[idx];
        if pr.status == PrStatus::Uploaded {
            return;
        }
        pr.status = match pr.status {
            PrStatus::Available => PrStatus::Queued,
            PrStatus::Queued => PrStatus::Available,
            PrStatus::Uploaded => PrStatus::Uploaded,
        };
    }

    /// Queue all available (non-uploaded) PRs for upload.
    fn select_all(&mut self) {
        for pr in &mut self.prs {
            if pr.status == PrStatus::Available {
                pr.status = PrStatus::Queued;
            }
        }
    }

    /// Cycle to the next tab.
    fn next_tab(&mut self) {
        self.tab = match self.tab {
            Tab::GetPrs => Tab::PublishReviews,
            Tab::PublishReviews => Tab::GetPrs,
        };
    }

    /// Begin processing all queued PRs (render PDFs + upload to reMarkable).
    fn execute(&mut self) {
        // TODO: spawn background worker for queued uploads
    }

    // === Rendering ===

    /// Top-level render function. Splits the terminal into vertical sections
    /// and delegates each to a specialized render method.
    fn render(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(MAIN_LAYOUT)
            .split(frame.area());

        self.render_header(frame, chunks[0]);
        self.render_tabs(frame, chunks[1]);
        self.render_main(frame, chunks[2]);
        self.render_footer(frame, chunks[3]);
        self.render_progress(frame, chunks[4]);
    }

    /// Render the version string right-aligned at the top of the screen.
    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let header = Paragraph::new(Span::styled(
            VERSION_LABEL,
            Style::default().fg(Color::DarkGray),
        ))
        .alignment(Alignment::Right);
        frame.render_widget(header, area);
    }

    /// Returns the accent color for the currently active tab.
    fn tab_color(&self) -> Color {
        match self.tab {
            Tab::GetPrs => Color::Green,
            Tab::PublishReviews => Color::Blue,
        }
    }

    /// Render the tab bar with colored active tab and matching separator line.
    fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let selected = match self.tab {
            Tab::GetPrs => 0,
            Tab::PublishReviews => 1,
        };

        let spans = TAB_NAMES
            .iter()
            .enumerate()
            .flat_map(|(i, name)| {
                let style = if i == selected {
                    Style::default().fg(Color::Black).bg(TAB_COLORS[i]).bold()
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let label = format!(" {name} ");
                let mut items = vec![Span::styled(label, style)];
                if i < TAB_NAMES.len() - 1 {
                    items.push(Span::raw("  "));
                }
                items
            })
            .collect_vec();

        let tabs = Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(self.tab_color())),
        );
        frame.render_widget(tabs, area);
    }

    /// Dispatch to the appropriate tab renderer.
    fn render_main(&mut self, frame: &mut Frame, area: Rect) {
        match self.tab {
            Tab::GetPrs => self.render_get_tab(frame, area),
            Tab::PublishReviews => self.render_publish_tab(frame, area),
        }
    }

    /// Render the "Get PRs" tab: section title, legend, PR selection table, and scrollbar.
    fn render_get_tab(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(GET_TAB_LAYOUT)
            .split(area);

        let title = Paragraph::new(GET_TAB_TITLE)
            .style(Style::default().bold())
            .centered();
        frame.render_widget(title, chunks[0]);

        let legend = Paragraph::new(Line::from(vec![
            Span::styled(" [✓]", Style::default().fg(Color::Green)),
            Span::raw(" on reMarkable  "),
            Span::styled("[x]", Style::default().fg(Color::Yellow)),
            Span::raw(" queued for upload"),
        ]));
        frame.render_widget(legend, chunks[1]);

        let header = Row::new(PR_TABLE_HEADERS.to_vec())
            .style(Style::default().fg(Color::Gray))
            .bottom_margin(1);

        let rows = self.prs.iter().map(PrRow::to_row).collect_vec();

        let table = Table::new(rows, PR_TABLE_COLUMNS)
            .header(header)
            .block(Block::default().borders(Borders::ALL))
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(table, chunks[2], &mut self.table_state);

        // Scrollbar on the right edge of the table
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state =
            ScrollbarState::new(self.prs.len()).position(self.table_state.selected().unwrap_or(0));
        frame.render_stateful_widget(
            scrollbar,
            chunks[2].inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }

    /// Render the "Publish Reviews" tab (placeholder for now).
    fn render_publish_tab(&self, frame: &mut Frame, area: Rect) {
        let placeholder = Paragraph::new("  Coming soon...")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Publish Reviews "),
            );
        frame.render_widget(placeholder, area);
    }

    /// Render the keybind help bar at the bottom of the screen.
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

    /// Render the progress bar and status message at the very bottom.
    ///
    /// Only visible when a background operation is in progress.
    /// Shows a human-readable status line and a gauge bar with step count.
    fn render_progress(&self, frame: &mut Frame, area: Rect) {
        match &self.progress {
            Some(progress) => {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(PROGRESS_LAYOUT)
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
                    .label(format!("{}/{} steps", progress.current, progress.total));
                frame.render_widget(gauge, chunks[1]);
            }
            None => {
                let empty = Paragraph::new("");
                frame.render_widget(empty, area);
            }
        }
    }
}
