use std::{
    collections::HashSet,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use color_eyre::eyre::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use itertools::Itertools;

use crate::{
    pdf::Pdf,
    pull_changes::{PullRequest, PullRequests},
    remarkable::RemarkableClient,
    review_data::ReviewData,
};
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

impl Tab {
    /// The accent color for this tab.
    fn color(self) -> Color {
        match self {
            Self::GetPrs => Color::Green,
            Self::PublishReviews => Color::Blue,
        }
    }

    /// The index of this tab in TAB_NAMES / TAB_COLORS.
    fn index(self) -> usize {
        match self {
            Self::GetPrs => 0,
            Self::PublishReviews => 1,
        }
    }

    /// Cycle to the next tab.
    fn next(self) -> Self {
        match self {
            Self::GetPrs => Self::PublishReviews,
            Self::PublishReviews => Self::GetPrs,
        }
    }
}

/// Whether the reMarkable tablet is reachable via USB.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemarkableStatus {
    Connected,
    Disconnected,
}

impl RemarkableStatus {
    /// Render as a styled span for the header bar.
    fn to_span(self) -> Span<'static> {
        match self {
            Self::Connected => {
                Span::styled("● reMarkable connected", Style::default().fg(Color::Green))
            }
            Self::Disconnected => Span::styled(
                "○ reMarkable disconnected",
                Style::default().fg(Color::DarkGray),
            ),
        }
    }
}

impl From<bool> for RemarkableStatus {
    fn from(connected: bool) -> Self {
        if connected {
            Self::Connected
        } else {
            Self::Disconnected
        }
    }
}

/// The upload status of a PR in the Get tab.
///
/// Determines how the row is displayed and whether it can be toggled.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PrStatus {
    /// Not yet selected -can be toggled with space.
    Available,
    /// Marked for upload this session -will be processed on enter.
    Queued,
    /// Already present on the reMarkable -cannot be toggled.
    Uploaded,
}

/// A lightweight view of a PR in the selection table.
///
/// Holds an index into the `PullRequests.pull_requests` slice for access
/// to the full data when needed (e.g. rendering PDFs), plus the upload status.
struct PrRow {
    /// Index into the PullRequests.pull_requests slice.
    pr_index: usize,
    /// Whether this PR is available, queued, or already uploaded.
    status: PrStatus,
}

impl PrRow {
    /// Render this PR as a table row, pulling display data from the full PullRequest.
    fn to_row<'a>(&self, pr: &'a PullRequest) -> Row<'a> {
        let checkbox = match self.status {
            PrStatus::Uploaded => Cell::from("[✓]").style(Style::default().fg(Color::Green)),
            PrStatus::Queued => Cell::from("[x]").style(Style::default().fg(Color::Yellow)),
            PrStatus::Available => Cell::from("[ ]"),
        };

        let base_style = match self.status {
            PrStatus::Uploaded => Style::default().fg(Color::DarkGray),
            _ => Style::default(),
        };

        let short_sha = &pr.head_ref_oid[..8.min(pr.head_ref_oid.len())];

        Row::new(vec![
            checkbox,
            Cell::from(format!("#{}", pr.number)),
            Cell::from(pr.repo_name.as_str()),
            Cell::from(short_sha),
            Cell::from(format!("+{}", pr.additions)).style(Style::default().fg(Color::Green)),
            Cell::from(format!("-{}", pr.deletions)).style(Style::default().fg(Color::Red)),
        ])
        .style(base_style)
    }
}

/// Messages sent from background threads to the TUI event loop.
enum BackgroundMessage {
    /// A status update to display while loading (e.g. "Fetching PRs from GitHub...").
    LoadingStatus(String),
    /// GitHub PR data has been fetched (or failed).
    PrsLoaded(Result<PullRequests>),
    /// reMarkable connection + document listing result.
    RemarkableLoaded(Result<(RemarkableStatus, Vec<String>)>),
    /// Upload progress update: (current_step, total_steps, message).
    UploadProgress(usize, usize, String),
    /// A single PR upload completed (index into prs vec).
    UploadedPr(usize),
    /// All uploads finished.
    UploadComplete,
    /// Upload failed with error.
    UploadError(String),
}

/// Tracks the state of an ongoing background operation (render + upload).
///
/// Displayed as a status line and progress bar at the bottom of the TUI.
/// Each PR has 4 sub-steps: fetch diff, fetch sources, render PDF, upload.
struct Progress {
    /// Human-readable description of the current step.
    message: String,
    /// Current sub-step across all PRs (0-based).
    current_step: usize,
    /// Total sub-steps across all PRs (prs * 4).
    total_steps: usize,
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
    /// The full PR data from GitHub (needed for rendering PDFs).
    pull_requests: Option<PullRequests>,
    /// Ratatui table selection state (tracks cursor position).
    table_state: TableState,
    /// Whether the reMarkable is reachable via USB.
    remarkable_status: RemarkableStatus,
    /// Filenames of inkrement docs already on the reMarkable.
    existing_filenames: HashSet<String>,
    /// Sender for background messages (cloned into worker threads).
    bg_tx: mpsc::Sender<BackgroundMessage>,
    /// Receiver for messages from background threads.
    bg_rx: Receiver<BackgroundMessage>,
    /// Status message shown during loading.
    loading_message: Option<String>,
    /// Number of loading tasks still outstanding.
    loading_pending: usize,
    /// Active progress indicator, if a background operation is running.
    progress: Option<Progress>,
    /// Set to true when the user presses 'q' to exit.
    should_quit: bool,
}

impl App {
    /// Create a new app and spawn background threads to load data.
    ///
    /// The TUI launches immediately with a loading spinner. GitHub PRs and
    /// reMarkable connection status are fetched in background threads and
    /// arrive via channel messages.
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let tx_gh = tx.clone();
        let tx_rm = tx.clone();

        // Spawn GitHub PR fetch
        thread::spawn(move || {
            let _ = tx_gh.send(BackgroundMessage::LoadingStatus(
                "Fetching PRs from GitHub...".to_string(),
            ));
            let _ = tx_gh.send(BackgroundMessage::PrsLoaded(PullRequests::fetch()));
        });

        // Spawn reMarkable connection check
        thread::spawn(move || {
            let _ = tx_rm.send(BackgroundMessage::LoadingStatus(
                "Connecting to reMarkable...".to_string(),
            ));
            let result = RemarkableClient::connect().and_then(|rm| {
                let docs = rm.list_inkrement_documents()?;
                let names = docs.into_iter().map(|d| d.visible_name).collect();
                Ok((RemarkableStatus::Connected, names))
            });
            let _ = tx_rm.send(BackgroundMessage::RemarkableLoaded(
                result.or_else(|_| Ok((RemarkableStatus::Disconnected, Vec::new()))),
            ));
        });

        Self {
            tab: Tab::GetPrs,
            prs: Vec::new(),
            pull_requests: None,
            table_state: TableState::default(),
            remarkable_status: RemarkableStatus::Disconnected,
            existing_filenames: HashSet::new(),
            bg_tx: tx,
            bg_rx: rx,
            loading_message: Some("Starting up...".to_string()),
            loading_pending: 2, // GitHub + reMarkable
            progress: None,
            should_quit: false,
        }
    }

    /// Run the TUI event loop.
    ///
    /// Uses polling to handle both keyboard input and background thread messages.
    /// Renders at ~30fps when idle, immediately on any event.
    pub(crate) fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal
                .draw(|frame| self.render(frame))
                .wrap_err("failed to draw frame")?;
            self.process_background_messages();
            self.handle_events()
                .wrap_err("while drawing TUI, failed to handle events")?;
        }
        Ok(())
    }

    // === Background message processing ===

    /// Drain all pending messages from background threads.
    fn process_background_messages(&mut self) {
        while let Ok(msg) = self.bg_rx.try_recv() {
            match msg {
                BackgroundMessage::LoadingStatus(message) => {
                    self.loading_message = Some(message);
                }
                BackgroundMessage::PrsLoaded(result) => {
                    match result {
                        Ok(pull_requests) => {
                            self.prs = pull_requests
                                .pull_requests
                                .iter()
                                .enumerate()
                                .map(|(i, pr)| PrRow {
                                    pr_index: i,
                                    status: if self.existing_filenames.contains(&pr.pdf_filename())
                                    {
                                        PrStatus::Uploaded
                                    } else {
                                        PrStatus::Available
                                    },
                                })
                                .collect();
                            if !self.prs.is_empty() {
                                self.table_state.select(Some(0));
                            }
                            self.pull_requests = Some(pull_requests);
                        }
                        Err(e) => {
                            self.loading_message = Some(format!("Failed to load PRs: {e}"));
                        }
                    }
                    self.check_loading_complete();
                }
                BackgroundMessage::RemarkableLoaded(result) => {
                    match result {
                        Ok((status, filenames)) => {
                            self.remarkable_status = status;
                            self.existing_filenames = filenames.into_iter().collect();
                            // Re-check uploaded status for any already-loaded PRs
                            if let Some(ref pull_requests) = self.pull_requests {
                                for row in &mut self.prs {
                                    let pr = &pull_requests.pull_requests[row.pr_index];
                                    if self.existing_filenames.contains(&pr.pdf_filename()) {
                                        row.status = PrStatus::Uploaded;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            self.loading_message =
                                Some(format!("reMarkable connection failed: {e}"));
                        }
                    }
                    self.check_loading_complete();
                }
                BackgroundMessage::UploadProgress(current_step, total_steps, message) => {
                    self.progress = Some(Progress {
                        message,
                        current_step,
                        total_steps,
                    });
                }
                BackgroundMessage::UploadedPr(pr_index) => {
                    if let Some(row) = self.prs.iter_mut().find(|r| r.pr_index == pr_index) {
                        row.status = PrStatus::Uploaded;
                    }
                }
                BackgroundMessage::UploadComplete => {
                    self.progress = None;
                }
                BackgroundMessage::UploadError(message) => {
                    self.progress = None;
                    self.loading_message = Some(format!("Upload failed: {message}"));
                }
            }
        }
    }

    /// Decrement the loading counter and clear the message when all tasks are done.
    fn check_loading_complete(&mut self) {
        self.loading_pending = self.loading_pending.saturating_sub(1);
        if self.loading_pending == 0 {
            self.loading_message = None;
        }
    }

    // === Event handling ===

    /// Poll for a terminal event with a short timeout to keep the UI responsive.
    fn handle_events(&mut self) -> Result<()> {
        // Short poll timeout so we can process loading messages frequently
        if !event::poll(Duration::from_millis(33)).wrap_err("failed to poll events")? {
            return Ok(());
        }

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
        self.tab = self.tab.next();
    }

    /// Begin processing all queued PRs (render PDFs + upload to reMarkable).
    fn execute(&mut self) {
        if self.remarkable_status != RemarkableStatus::Connected {
            self.loading_message = Some("reMarkable not connected".to_string());
            return;
        }

        let Some(ref pull_requests) = self.pull_requests else {
            return;
        };

        // Collect the queued PR indices and corresponding data
        let queued: Vec<(usize, String)> = self
            .prs
            .iter()
            .filter(|row| row.status == PrStatus::Queued)
            .map(|row| {
                let pr = &pull_requests.pull_requests[row.pr_index];
                (row.pr_index, pr.pdf_filename())
            })
            .collect();

        if queued.is_empty() {
            return;
        }

        let reviewer = pull_requests.reviewer.clone();
        let total_steps = queued.len() * 4; // 4 steps per PR: diff, sources, render, upload
        let tx = self.bg_tx.clone();

        // TODO: Clone is only here to send PR data to the upload worker thread. Fix this.
        let pr_data: Vec<(usize, PullRequest)> = queued
            .iter()
            .map(|(idx, _)| {
                let pr = &pull_requests.pull_requests[*idx];
                (*idx, pr.clone())
            })
            .collect();

        thread::spawn(move || {
            let rm = match RemarkableClient::connect() {
                Ok(rm) => rm,
                Err(e) => {
                    let _ = tx.send(BackgroundMessage::UploadError(format!(
                        "Failed to connect to reMarkable: {e}"
                    )));
                    return;
                }
            };

            let mut step = 0;

            for (pr_index, pr) in &pr_data {
                let name = format!("{}#{}", pr.repo_name, pr.number);

                // Step 1: fetch diff
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Fetching diff for {name}..."),
                ));
                let patch = match pr.parse_diff() {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!("{name}: {e}")));
                        return;
                    }
                };
                step += 1;

                // Step 2: fetch source files
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Fetching source files for {name}..."),
                ));
                let source_files = match pr.fetch_source_files(&patch) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!("{name}: {e}")));
                        return;
                    }
                };
                step += 1;

                // Step 3: render PDF
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Rendering PDF for {name}..."),
                ));
                let review_data = ReviewData::build(pr, &reviewer, &patch, &source_files);
                let pdf = match Pdf::render(&review_data) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!("{name}: {e}")));
                        return;
                    }
                };
                step += 1;

                // Step 4: upload to reMarkable
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Uploading {name} to reMarkable..."),
                ));
                let filename = pr.pdf_filename();
                match rm.upload(&filename, &pdf) {
                    Ok(()) => {
                        let _ = tx.send(BackgroundMessage::UploadedPr(*pr_index));
                    }
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!("{name}: {e}")));
                        return;
                    }
                }
                step += 1;
            }

            let _ = tx.send(BackgroundMessage::UploadComplete);
        });
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

    /// Render the version string and reMarkable connection status.
    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let header = Line::from(vec![
            self.remarkable_status.to_span(),
            Span::raw("  "),
            Span::styled(VERSION_LABEL, Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(header).alignment(Alignment::Right), area);
    }

    /// Render the tab bar with colored active tab and matching separator line.
    fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let selected = self.tab.index();

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
                .border_style(Style::default().fg(self.tab.color())),
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

        let empty_prs: Box<[PullRequest]> = Box::new([]);
        let pr_data = self
            .pull_requests
            .as_ref()
            .map(|prs| &prs.pull_requests)
            .unwrap_or(&empty_prs);

        let table = Table::new(
            self.prs
                .iter()
                .map(|row| row.to_row(&pr_data[row.pr_index])),
            PR_TABLE_COLUMNS,
        )
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

    /// Render the progress/loading area at the very bottom.
    ///
    /// Shows either:
    /// - A loading spinner message during startup
    /// - A progress bar during upload operations
    /// - Nothing when idle
    fn render_progress(&self, frame: &mut Frame, area: Rect) {
        // Loading message takes priority (startup)
        if let Some(message) = &self.loading_message {
            let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let tick = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() / 80)
                .unwrap_or(0)
                % spinner.len() as u128) as usize;

            let line = Line::from(vec![
                Span::styled(spinner[tick], Style::default().fg(Color::Cyan)),
                Span::raw(format!(" {message}")),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }

        // Progress bar during operations
        if let Some(progress) = &self.progress {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(PROGRESS_LAYOUT)
                .split(area);

            let status = Paragraph::new(format!("  {}", progress.message))
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(status, chunks[0]);

            let ratio = if progress.total_steps > 0 {
                progress.current_step as f64 / progress.total_steps as f64
            } else {
                0.0
            };
            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Cyan))
                .ratio(ratio);
            frame.render_widget(gauge, chunks[1]);
        }
    }
}
