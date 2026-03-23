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
    annotate, diff_parse,
    pdf::Pdf,
    pdf_strip,
    post_review::{self, ReviewTarget},
    pull_changes::{PullRequest, PullRequests},
    remarkable::{DocumentId, RemarkableClient},
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

/// The review status of a document in the Publish tab.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReviewStatus {
    /// Available for selection.
    Available,
    /// Selected for download + review.
    Queued,
}

/// A row in the Publish tab's document table.
///
/// Represents an inkrement document on the reMarkable that may contain
/// handwritten review annotations ready to be published.
struct DocRow {
    /// The document's ID on the reMarkable (needed for download).
    doc_id: DocumentId,
    /// PR number parsed from the filename.
    number: String,
    /// Repository name parsed from the filename.
    repo: String,
    /// Short SHA parsed from the filename.
    short_sha: String,
    /// Number of OCR-relevant pages (before source reference section).
    ocr_pages: usize,
    /// Whether this doc is selected for processing.
    status: ReviewStatus,
}

impl DocRow {
    /// Parse an inkrement filename into a DocRow.
    /// Expected format: "#72 Atomic-Industries-crab-rave [747e7fe8] p42 inkrement.pdf"
    fn from_remarkable(doc_id: DocumentId, name: &str) -> Self {
        let (number, repo, short_sha, ocr_pages) = Self::parse_filename(name)
            .unwrap_or_else(|| ("?".to_string(), name.to_string(), "?".to_string(), 0));

        Self {
            doc_id,
            number,
            repo,
            short_sha,
            ocr_pages,
            status: ReviewStatus::Available,
        }
    }

    /// Try to extract PR number, repo, SHA, and OCR page count from an inkrement filename.
    fn parse_filename(name: &str) -> Option<(String, String, String, usize)> {
        // "#72 Atomic-Industries-crab-rave [747e7fe8] p42 inkrement.pdf"
        let rest = name.strip_prefix('#')?;
        let (number, rest) = rest.split_once(' ')?;
        let (repo, rest) = rest.split_once(" [")?;
        let (sha, rest) = rest.split_once("] ")?;
        let (pages_str, _) = rest.split_once(' ')?;
        let ocr_pages = pages_str.strip_prefix('p')?.parse().ok()?;
        Some((
            format!("#{number}"),
            repo.to_string(),
            sha.to_string(),
            ocr_pages,
        ))
    }

    /// Render this document as a table row.
    fn to_row(&self) -> Row<'_> {
        let checkbox = match self.status {
            ReviewStatus::Queued => Cell::from("[x]").style(Style::default().fg(Color::Yellow)),
            ReviewStatus::Available => Cell::from("[ ]"),
        };

        Row::new(vec![
            checkbox,
            Cell::from(self.number.as_str()),
            Cell::from(self.repo.as_str()),
            Cell::from(self.short_sha.as_str()),
        ])
    }
}

/// Messages sent from background threads to the TUI event loop.
enum BackgroundMessage {
    /// GitHub PR data has been fetched (or failed).
    PrsLoaded(Result<PullRequests>),
    /// reMarkable connection + document listing result.
    /// Contains (status, vec of (doc_id, visible_name)).
    RemarkableLoaded(Result<(RemarkableStatus, Vec<(DocumentId, String)>)>),
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
    /// Ratatui table selection state for Get tab.
    get_table_state: TableState,
    /// The list of reMarkable docs shown in the Publish tab.
    docs: Vec<DocRow>,
    /// Ratatui table selection state for Publish tab.
    publish_table_state: TableState,
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
    /// Error message shown as a red popup overlay, dismissed with Escape.
    error_message: Option<String>,
    /// Set to true when the user presses 'q' to exit.
    should_quit: bool,
}

impl App {
    /// Create a new app and begin loading data in the background.
    ///
    /// The TUI launches immediately with a loading spinner. GitHub PRs and
    /// reMarkable connection status are fetched in background threads and
    /// arrive via channel messages.
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();

        let mut app = Self {
            tab: Tab::GetPrs,
            prs: Vec::new(),
            pull_requests: None,
            get_table_state: TableState::default(),
            docs: Vec::new(),
            publish_table_state: TableState::default(),
            remarkable_status: RemarkableStatus::Disconnected,
            existing_filenames: HashSet::new(),
            bg_tx: tx,
            bg_rx: rx,
            loading_message: Some("Starting up...".to_string()),
            loading_pending: 0,
            progress: None,
            error_message: None,
            should_quit: false,
        };

        app.refresh();
        app
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
                BackgroundMessage::PrsLoaded(result) => {
                    match result {
                        Ok(pull_requests) => {
                            self.prs = pull_requests
                                .pull_requests
                                .iter()
                                .enumerate()
                                .map(|(i, pr)| PrRow {
                                    pr_index: i,
                                    status: if self
                                        .existing_filenames
                                        .iter()
                                        .any(|f| f.starts_with(&pr.filename_prefix()))
                                    {
                                        PrStatus::Uploaded
                                    } else {
                                        PrStatus::Available
                                    },
                                })
                                .collect();
                            if !self.prs.is_empty() {
                                self.get_table_state.select(Some(0));
                            }
                            self.pull_requests = Some(pull_requests);
                        }
                        Err(e) => {
                            self.error_message = Some(format!("Failed to load PRs: {e}"));
                        }
                    }
                    self.check_loading_complete();
                }
                BackgroundMessage::RemarkableLoaded(result) => {
                    match result {
                        Ok((status, doc_pairs)) => {
                            self.remarkable_status = status;
                            self.existing_filenames =
                                doc_pairs.iter().map(|(_, name)| name.clone()).collect();

                            // Populate the Publish tab with docs from reMarkable
                            self.docs = doc_pairs
                                .into_iter()
                                .map(|(id, name)| DocRow::from_remarkable(id, &name))
                                .collect();
                            if !self.docs.is_empty() {
                                self.publish_table_state.select(Some(0));
                            }

                            // Re-check uploaded status for any already-loaded PRs
                            if let Some(ref pull_requests) = self.pull_requests {
                                for row in &mut self.prs {
                                    let pr = &pull_requests.pull_requests[row.pr_index];
                                    if self
                                        .existing_filenames
                                        .iter()
                                        .any(|f| f.starts_with(&pr.filename_prefix()))
                                    {
                                        row.status = PrStatus::Uploaded;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            self.error_message =
                                Some(format!("reMarkable connection failed: {e}"));
                        }
                    }
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
                    self.error_message = Some(message);
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

        // Dismiss error popup on any key
        if self.error_message.is_some() {
            self.error_message = None;
            return Ok(());
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char(' ') => self.toggle_selected(),
            KeyCode::Char('a') => self.select_all(),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Tab => self.next_tab(),
            KeyCode::Enter => self.execute(),
            _ => {}
        }

        Ok(())
    }

    /// Move the active tab's table cursor up (negative) or down (positive).
    fn move_cursor(&mut self, delta: i32) {
        let (len, state) = match self.tab {
            Tab::GetPrs => (self.prs.len(), &mut self.get_table_state),
            Tab::PublishReviews => (self.docs.len(), &mut self.publish_table_state),
        };
        if len == 0 {
            return;
        }
        let current = state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, len as i32 - 1) as usize;
        state.select(Some(next));
    }

    /// Toggle the selected item in the active tab.
    fn toggle_selected(&mut self) {
        match self.tab {
            Tab::GetPrs => {
                let Some(idx) = self.get_table_state.selected() else {
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
            Tab::PublishReviews => {
                let Some(idx) = self.publish_table_state.selected() else {
                    return;
                };
                let doc = &mut self.docs[idx];
                doc.status = match doc.status {
                    ReviewStatus::Available => ReviewStatus::Queued,
                    ReviewStatus::Queued => ReviewStatus::Available,
                };
            }
        }
    }

    /// Select all items in the active tab.
    fn select_all(&mut self) {
        match self.tab {
            Tab::GetPrs => {
                for pr in &mut self.prs {
                    if pr.status == PrStatus::Available {
                        pr.status = PrStatus::Queued;
                    }
                }
            }
            Tab::PublishReviews => {
                for doc in &mut self.docs {
                    doc.status = ReviewStatus::Queued;
                }
            }
        }
    }

    /// Spawn background threads to fetch data from GitHub and reMarkable.
    /// Used on startup and when the user presses 'r'.
    fn refresh(&mut self) {
        if self.loading_pending > 0 {
            return;
        }

        self.loading_message = Some("Fetching PRs from GitHub...".to_string());
        self.loading_pending = 1;

        let tx_gh = self.bg_tx.clone();
        thread::spawn(move || {
            let _ = tx_gh.send(BackgroundMessage::PrsLoaded(PullRequests::fetch()));
        });

        let tx_rm = self.bg_tx.clone();
        thread::spawn(move || {
            let result = RemarkableClient::connect().and_then(|rm| {
                let docs = rm.list_inkrement_documents()?;
                let pairs = docs.into_iter().map(|d| (d.id, d.visible_name)).collect();
                Ok((RemarkableStatus::Connected, pairs))
            });
            let _ = tx_rm.send(BackgroundMessage::RemarkableLoaded(
                result.or_else(|_| Ok((RemarkableStatus::Disconnected, Vec::new()))),
            ));
        });
    }

    /// Cycle to the next tab.
    fn next_tab(&mut self) {
        self.tab = self.tab.next();
    }

    /// Execute the action for the current tab.
    fn execute(&mut self) {
        match self.tab {
            Tab::GetPrs => self.execute_get(),
            Tab::PublishReviews => self.execute_publish(),
        }
    }

    /// Begin processing all queued PRs (render PDFs + upload to reMarkable).
    fn execute_get(&mut self) {
        if self.remarkable_status != RemarkableStatus::Connected {
            self.error_message = Some("reMarkable not connected".to_string());
            return;
        }

        let Some(ref pull_requests) = self.pull_requests else {
            return;
        };

        // Collect the queued PR indices and corresponding data
        let queued_indices: Vec<usize> = self
            .prs
            .iter()
            .filter(|row| row.status == PrStatus::Queued)
            .map(|row| row.pr_index)
            .collect();

        if queued_indices.is_empty() {
            return;
        }

        let reviewer = pull_requests.reviewer.clone();
        let total_steps = queued_indices.len() * 4; // 4 steps per PR: diff, sources, render, upload
        let tx = self.bg_tx.clone();

        // TODO: Clone is only here to send PR data to the upload worker thread. Fix this.
        let pr_data: Vec<(usize, PullRequest)> = queued_indices
            .iter()
            .map(|idx| {
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

                // Step 1: resolve diff (incremental if prior review exists)
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Resolving diff for {name}..."),
                ));
                let resolved = match pr.resolve_diff(&reviewer) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!("{name}: {e}")));
                        return;
                    }
                };
                let patch = match diff_parse::parse_diff(&resolved.diff) {
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
                let source_files = match pr.fetch_source_files(&patch, &resolved.diff_base) {
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
                let review_data =
                    ReviewData::build(pr, &reviewer, &patch, &source_files, &resolved);
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
                let filename = pr.pdf_filename(pdf.ocr_page_count);
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

    /// Download, interpret, and post reviews for selected documents.
    fn execute_publish(&mut self) {
        if self.remarkable_status != RemarkableStatus::Connected {
            self.error_message = Some("reMarkable not connected".to_string());
            return;
        }

        // Collect queued docs with their filename info for repo resolution
        let queued: Vec<(DocumentId, String, String, usize)> = self
            .docs
            .iter()
            .filter(|doc| doc.status == ReviewStatus::Queued)
            .map(|doc| {
                let display = format!("{} {}", doc.number, doc.repo);
                let full_name = format!(
                    "{} {} [{}] p{} inkrement",
                    doc.number, doc.repo, doc.short_sha, doc.ocr_pages,
                );
                (doc.doc_id.clone(), display, full_name, doc.ocr_pages)
            })
            .collect();

        if queued.is_empty() {
            return;
        }

        // 4 steps per doc: download, strip, interpret, post
        let total_steps = queued.len() * 4;
        let tx = self.bg_tx.clone();

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

            for (doc_id, name, full_name, ocr_pages) in &queued {
                // Step 1: Download
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Downloading {name}..."),
                ));
                let pdf_bytes = match rm.download(doc_id) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!(
                            "Failed to download {name}: {e}"
                        )));
                        return;
                    }
                };
                step += 1;

                // Step 2: Strip source pages
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Stripping source pages from {name} (this may take ~1min)..."),
                ));
                let stripped = match pdf_strip::strip_source_pages(&pdf_bytes, *ocr_pages) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!(
                            "Failed to strip {name}: {e}"
                        )));
                        return;
                    }
                };

                let tmp_dir = match tempfile::tempdir() {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!(
                            "Failed to create temp dir: {e}"
                        )));
                        return;
                    }
                };
                let pdf_path = tmp_dir.path().join(format!("{name}.pdf"));
                if let Err(e) = std::fs::write(&pdf_path, &stripped) {
                    let _ = tx.send(BackgroundMessage::UploadError(format!(
                        "Failed to save {}: {e}",
                        pdf_path.display()
                    )));
                    return;
                }
                step += 1;

                // Step 3: Interpret via Claude
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Interpreting {name} via Claude..."),
                ));
                let review = match annotate::interpret_pdf(&pdf_path) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!(
                            "Failed to interpret {name}: {e}"
                        )));
                        return;
                    }
                };

                if review.is_empty() {
                    let _ = tx.send(BackgroundMessage::UploadProgress(
                        step,
                        total_steps,
                        format!("Skipping {name} (no annotations)"),
                    ));
                    step += 2; // skip interpret + post steps
                    continue;
                }
                step += 1;

                // Step 4: Post to GitHub
                let _ = tx.send(BackgroundMessage::UploadProgress(
                    step,
                    total_steps,
                    format!("Posting review for {name} to GitHub..."),
                ));
                let target = match ReviewTarget::from_filename(full_name) {
                    Ok(t) => t,
                    Err(e) => {
                        let _ = tx.send(BackgroundMessage::UploadError(format!(
                            "Failed to parse PR info for {name}: {e}"
                        )));
                        return;
                    }
                };

                if let Err(e) = post_review::post_review(&target, &review) {
                    let _ = tx.send(BackgroundMessage::UploadError(format!(
                        "Failed to post review for {name}: {e}"
                    )));
                    return;
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

        if let Some(error) = &self.error_message {
            self.render_error_popup(frame, error.clone());
        }
    }

    /// Render a centered red error popup overlay.
    fn render_error_popup(&self, frame: &mut Frame, message: String) {
        let area = frame.area();

        // Size the popup: up to 60% width, height based on wrapped text + padding
        let popup_width = (area.width * 3 / 5).max(40).min(area.width.saturating_sub(4));
        // Rough line count: wrap message to inner width (popup - borders - padding)
        let inner_width = popup_width.saturating_sub(6) as usize;
        let line_count = if inner_width > 0 {
            message
                .as_bytes()
                .chunks(inner_width)
                .count()
                .max(1)
        } else {
            1
        };
        let popup_height = (line_count as u16 + 4).min(area.height.saturating_sub(4));

        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(ratatui::widgets::Clear, popup_area);

        let block = Block::default()
            .title(" Error ")
            .title_style(Style::default().fg(Color::White).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .style(Style::default().bg(Color::Black));

        let text = Paragraph::new(message)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .style(Style::default().fg(Color::Red))
            .block(block);

        frame.render_widget(text, popup_area);
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

        frame.render_stateful_widget(table, chunks[2], &mut self.get_table_state);

        // Scrollbar on the right edge of the table
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state = ScrollbarState::new(self.prs.len())
            .position(self.get_table_state.selected().unwrap_or(0));
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
    fn render_publish_tab(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(1), // legend
                Constraint::Min(3),    // table
            ])
            .split(area);

        let title = Paragraph::new("Reviews on reMarkable")
            .style(Style::default().bold())
            .centered();
        frame.render_widget(title, chunks[0]);

        let legend = Paragraph::new(Line::from(vec![
            Span::styled(" [x]", Style::default().fg(Color::Yellow)),
            Span::raw(" queued for review"),
        ]));
        frame.render_widget(legend, chunks[1]);

        let header = Row::new(vec!["", "#", "Repository", "SHA"])
            .style(Style::default().fg(Color::Gray))
            .bottom_margin(1);

        let table = Table::new(
            self.docs.iter().map(DocRow::to_row),
            [
                Constraint::Length(5),  // checkbox
                Constraint::Length(6),  // number
                Constraint::Min(20),    // repo
                Constraint::Length(10), // sha
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(table, chunks[2], &mut self.publish_table_state);

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state = ScrollbarState::new(self.docs.len())
            .position(self.publish_table_state.selected().unwrap_or(0));
        frame.render_stateful_widget(
            scrollbar,
            chunks[2].inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
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
