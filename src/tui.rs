//! Watch Board renderer, live session, and offline preview state.
//!
//! The renderer is shared by `cargo run --example watch_board` and opt-in
//! `--tui`. Live wiring keeps orchestration on the calling thread and runs the
//! dashboard on a scoped UI thread behind a narrow event/cancel seam.
//!
//! Preview the same renderer with labeled sample state:
//!
//! ```text
//! cargo run --example watch_board
//! ```

use std::io::{self, stdout};
#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

/// Semantic Watch Board colors. Native background, ANSI accents, DIM chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    bg: Color,
    fg: Color,
    running: Color,
    completed: Color,
    blocked: Color,
    failed: Color,
}

impl Theme {
    /// Terminal-native palette: `Reset` canvas plus cyan/green/yellow/red.
    pub fn native() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Reset,
            running: Color::Cyan,
            completed: Color::Green,
            blocked: Color::Yellow,
            failed: Color::Red,
        }
    }

    /// Colorless palette for `NO_COLOR` and tests.
    pub fn no_color() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Reset,
            running: Color::Reset,
            completed: Color::Reset,
            blocked: Color::Reset,
            failed: Color::Reset,
        }
    }

    /// Native palette unless `NO_COLOR` is set and non-empty.
    pub fn from_env() -> Self {
        Self::from_no_color_var(std::env::var_os("NO_COLOR").as_deref())
    }

    fn from_no_color_var(no_color: Option<&std::ffi::OsStr>) -> Self {
        if no_color.is_none_or(|value| value.is_empty()) {
            Self::native()
        } else {
            Self::no_color()
        }
    }

    fn body(self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }

    fn muted(self) -> Style {
        self.body().add_modifier(Modifier::DIM)
    }

    fn bold(self) -> Style {
        self.body().add_modifier(Modifier::BOLD)
    }

    fn live(self) -> Style {
        self.body().fg(self.running)
    }

    fn status(self, status: IssueStatus) -> Style {
        let color = match status {
            IssueStatus::Completed => self.completed,
            IssueStatus::Running => self.running,
            IssueStatus::Queued | IssueStatus::Blocked => self.blocked,
            IssueStatus::Failed => self.failed,
        };
        self.body().fg(color)
    }

    fn phase(self, phase: &Phase) -> Style {
        match phase {
            Phase::Running { .. } | Phase::Dispatch { .. } | Phase::Verify { .. } => self.live(),
            Phase::Done => self.status(IssueStatus::Completed),
            Phase::Failed { .. } => self.status(IssueStatus::Failed),
            Phase::Idle | Phase::Hygiene => self.muted(),
        }
    }
}

/// Textual issue status. Color is optional decoration, never the only signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueStatus {
    /// GitHub confirmed the issue closed after an agent run.
    Completed,
    /// Agent is running on this issue now.
    Running,
    /// In the plan and not yet started.
    Queued,
    /// Waiting on a blocker.
    Blocked,
    /// Agent or verify step failed; the row stays visible.
    Failed,
}

impl IssueStatus {
    /// Stable roster/count label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Running => "running",
            Self::Queued => "queued",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

/// One roster row: identity, status, and resolved invocation profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRow {
    /// GitHub issue number.
    pub number: u32,
    /// Issue title, which may contain Unicode.
    pub title: String,
    /// Current truthful status.
    pub status: IssueStatus,
    /// Resolved agent name for this issue.
    pub agent: String,
    /// Resolved model, or `None` for the agent default.
    pub model: Option<String>,
    /// Resolved effort, or `None` for the agent default.
    pub effort: Option<String>,
}

/// Planned or blocked roster identity plus resolved invocation profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RosterIssue {
    pub number: u32,
    pub title: String,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl RosterIssue {
    fn into_row(self, status: IssueStatus) -> IssueRow {
        IssueRow {
            number: self.number,
            title: self.title,
            status,
            agent: self.agent,
            model: self.model,
            effort: self.effort,
        }
    }
}

/// Orchestration facts for the Watch Board. Never includes agent logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WatchEvent {
    /// Header facts once known.
    Session {
        prd: u32,
        prd_title: String,
        repo: String,
        branch: String,
    },
    /// Latest plan. Completed/failed history is kept by [`BoardState::apply`].
    Roster {
        planned: Vec<RosterIssue>,
        blocked: Vec<RosterIssue>,
    },
    /// Git hygiene / sync on the base branch.
    Hygiene,
    /// About to invoke the agent.
    Dispatch { issue: u32 },
    /// Agent process is running.
    Running { issue: u32 },
    /// Checking that GitHub closed the issue.
    Verify { issue: u32 },
    /// GitHub confirmed the issue closed.
    Completed { issue: u32 },
    /// Run stopped with a readable error.
    Failed { issue: u32, message: String },
    /// Planned work finished or stop was honored.
    Done,
}

/// Narrow observer used by the shared orchestration loop.
pub(crate) trait Watch {
    fn emit(&self, event: WatchEvent);
    fn stop_requested(&self) -> bool;
}

pub(crate) struct NullWatch;

impl Watch for NullWatch {
    fn emit(&self, _event: WatchEvent) {}
    fn stop_requested(&self) -> bool {
        false
    }
}

/// Test observer that records events and a cancel flag.
#[cfg(test)]
pub(crate) struct WatchLog {
    events: Mutex<Vec<WatchEvent>>,
    stop: AtomicBool,
}

#[cfg(test)]
impl WatchLog {
    pub(crate) fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        }
    }

    pub(crate) fn events(&self) -> Vec<WatchEvent> {
        self.events.lock().expect("watch log").clone()
    }

    pub(crate) fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
impl Watch for WatchLog {
    fn emit(&self, event: WatchEvent) {
        self.events.lock().expect("watch log").push(event);
    }

    fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Live orchestration phase shown in the profile pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Board is idle before work starts.
    Idle,
    /// Git hygiene on the base branch.
    Hygiene,
    /// About to invoke the agent for `issue`.
    Dispatch {
        /// Issue that will run next.
        issue: u32,
    },
    /// Agent process is running for `issue`.
    Running {
        /// Issue the agent is working on.
        issue: u32,
    },
    /// Checking that GitHub closed `issue`.
    Verify {
        /// Issue being confirmed closed.
        issue: u32,
    },
    /// Planned work finished.
    Done,
    /// Run stopped with a visible error.
    Failed {
        /// Issue that failed, when known.
        issue: u32,
        /// Readable error detail, not agent logs.
        message: String,
    },
}

impl Phase {
    fn label(&self) -> String {
        match self {
            Self::Idle => "idle".to_string(),
            Self::Hygiene => "git hygiene".to_string(),
            Self::Dispatch { issue } => format!("dispatch #{issue}"),
            Self::Running { issue } => format!("agent #{issue}"),
            Self::Verify { issue } => format!("verify #{issue}"),
            Self::Done => "done".to_string(),
            Self::Failed { issue, .. } => format!("failed #{issue}"),
        }
    }
}

/// Truthful tallies derived from roster rows, never stored separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusCounts {
    /// Closed issues.
    pub completed: usize,
    /// The in-flight issue.
    pub running: usize,
    /// Not started.
    pub queued: usize,
    /// Waiting on blockers.
    pub blocked: usize,
    /// Failed rows still on the board.
    pub failed: usize,
}

/// What a key press asked the board to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardCommand {
    /// Keep drawing.
    Continue,
    /// Leave the preview, or dismiss an idle live board.
    Quit,
    /// Request stop after the current issue on a live board.
    Stop,
}

/// Frame snapshot for the Watch Board renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardState {
    /// PRD issue number.
    pub prd: u32,
    /// PRD title shown in the header when space allows.
    pub prd_title: String,
    /// Repository slug `owner/name`.
    pub repo: String,
    /// Base or working branch name.
    pub branch: String,
    /// Optional banner such as `offline preview`.
    pub notice: Option<String>,
    /// Roster in display order.
    pub issues: Vec<IssueRow>,
    /// Selected roster index.
    pub selected: usize,
    /// Current orchestration phase.
    pub phase: Phase,
    /// Elapsed time for the current phase or run.
    pub elapsed: Duration,
    /// Whether the `?` help overlay is open.
    pub help_open: bool,
    /// Live orchestration board, not the offline preview.
    pub live: bool,
    /// `q` / Ctrl-C asked to stop after the current issue.
    pub stop_pending: bool,
}

impl BoardState {
    /// Labeled sample board covering running, completed, blocked, queued, and failed.
    ///
    /// This is fixture data for the offline example and tests. It is not a live run.
    pub fn offline_preview() -> Self {
        Self {
            prd: 15,
            prd_title: "Add opt-in Watch Board TUI".to_string(),
            repo: "offline/preview".to_string(),
            branch: "sample".to_string(),
            notice: Some("offline preview".to_string()),
            issues: vec![
                IssueRow {
                    number: 11,
                    title: "Parse parent links".to_string(),
                    status: IssueStatus::Completed,
                    agent: "pi".to_string(),
                    model: Some("pi-default".to_string()),
                    effort: Some("low".to_string()),
                },
                IssueRow {
                    number: 12,
                    title: "子 issue — Watch Board".to_string(),
                    status: IssueStatus::Running,
                    agent: "pi".to_string(),
                    model: Some("composer".to_string()),
                    effort: Some("high".to_string()),
                },
                IssueRow {
                    number: 13,
                    title: "Blocked by an open child".to_string(),
                    status: IssueStatus::Blocked,
                    agent: "claude".to_string(),
                    model: None,
                    effort: None,
                },
                IssueRow {
                    number: 14,
                    title: "Queued after blockers".to_string(),
                    status: IssueStatus::Queued,
                    agent: "pi".to_string(),
                    model: None,
                    effort: Some("medium".to_string()),
                },
                IssueRow {
                    number: 16,
                    title: "Agent exited nonzero".to_string(),
                    status: IssueStatus::Failed,
                    agent: "codex".to_string(),
                    model: Some("gpt".to_string()),
                    effort: Some("high".to_string()),
                },
            ],
            selected: 1,
            phase: Phase::Running { issue: 12 },
            elapsed: Duration::from_secs(75),
            help_open: false,
            live: false,
            stop_pending: false,
        }
    }

    /// Empty live board for a real `--tui` run.
    pub fn live_run(prd: u32, repo: String, branch: String) -> Self {
        Self {
            prd,
            prd_title: String::new(),
            repo,
            branch,
            notice: None,
            issues: Vec::new(),
            selected: 0,
            phase: Phase::Idle,
            elapsed: Duration::ZERO,
            help_open: false,
            live: true,
            stop_pending: false,
        }
    }

    /// Counts each [`IssueStatus`] currently on the roster.
    pub fn counts(&self) -> StatusCounts {
        let mut counts = StatusCounts {
            completed: 0,
            running: 0,
            queued: 0,
            blocked: 0,
            failed: 0,
        };
        for issue in &self.issues {
            match issue.status {
                IssueStatus::Completed => counts.completed += 1,
                IssueStatus::Running => counts.running += 1,
                IssueStatus::Queued => counts.queued += 1,
                IssueStatus::Blocked => counts.blocked += 1,
                IssueStatus::Failed => counts.failed += 1,
            }
        }
        counts
    }

    /// Selected roster row, if any.
    pub fn selected_issue(&self) -> Option<&IssueRow> {
        if self.issues.is_empty() {
            None
        } else {
            self.issues.get(self.selected.min(self.issues.len() - 1))
        }
    }

    /// Applies a key. Navigation and help stay on the board.
    ///
    /// Preview: `q` / Ctrl-C quit. Live active: those keys request stop after
    /// the current issue. Live idle: `q` / Ctrl-C / Enter dismiss.
    pub fn handle_key(&mut self, key: KeyEvent) -> BoardCommand {
        if key.kind != KeyEventKind::Press {
            return BoardCommand::Continue;
        }
        let quit_key = (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
            || matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'));
        if quit_key {
            return self.quit_or_stop();
        }
        match key.code {
            KeyCode::Enter if self.live && self.can_dismiss() => BoardCommand::Quit,
            KeyCode::Char('?') => {
                self.help_open = !self.help_open;
                BoardCommand::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                BoardCommand::Continue
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_prev();
                BoardCommand::Continue
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.select_first();
                BoardCommand::Continue
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.select_last();
                BoardCommand::Continue
            }
            _ => BoardCommand::Continue,
        }
    }

    fn select_next(&mut self) {
        if self.issues.is_empty() {
            self.selected = 0;
            return;
        }
        if self.selected < self.issues.len() - 1 {
            self.selected += 1;
        }
    }

    fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn select_first(&mut self) {
        self.selected = 0;
    }

    fn select_last(&mut self) {
        self.selected = self.issues.len().saturating_sub(1);
    }

    fn can_dismiss(&self) -> bool {
        matches!(self.phase, Phase::Done | Phase::Failed { .. })
    }

    fn quit_or_stop(&mut self) -> BoardCommand {
        if self.live && !self.can_dismiss() {
            self.stop_pending = true;
            self.notice = Some("stop after current issue".to_string());
            BoardCommand::Stop
        } else {
            BoardCommand::Quit
        }
    }

    fn select_number(&mut self, number: u32) {
        if let Some(index) = self.issues.iter().position(|issue| issue.number == number) {
            self.selected = index;
        }
    }

    /// Applies one orchestration event. Completed rows are only created from
    /// [`WatchEvent::Completed`], never from a missing plan row.
    pub(crate) fn apply(&mut self, event: WatchEvent) {
        match event {
            WatchEvent::Session {
                prd,
                prd_title,
                repo,
                branch,
            } => {
                self.prd = prd;
                self.prd_title = prd_title;
                self.repo = repo;
                self.branch = branch;
            }
            WatchEvent::Roster { planned, blocked } => self.merge_roster(planned, blocked),
            WatchEvent::Hygiene => self.phase = Phase::Hygiene,
            WatchEvent::Dispatch { issue } => {
                self.phase = Phase::Dispatch { issue };
                self.mark_running(issue);
            }
            WatchEvent::Running { issue } => {
                self.phase = Phase::Running { issue };
                self.mark_running(issue);
            }
            WatchEvent::Verify { issue } => {
                self.phase = Phase::Verify { issue };
                self.mark_running(issue);
            }
            WatchEvent::Completed { issue } => {
                self.mark_status(issue, IssueStatus::Completed);
            }
            WatchEvent::Failed { issue, message } => {
                self.mark_status(issue, IssueStatus::Failed);
                self.phase = Phase::Failed { issue, message };
            }
            WatchEvent::Done => self.phase = Phase::Done,
        }
    }

    fn merge_roster(&mut self, planned: Vec<RosterIssue>, blocked: Vec<RosterIssue>) {
        let mut issues: Vec<IssueRow> = self
            .issues
            .iter()
            .filter(|row| matches!(row.status, IssueStatus::Completed | IssueStatus::Failed))
            .cloned()
            .collect();
        for item in planned {
            if issues.iter().any(|row| row.number == item.number) {
                continue;
            }
            issues.push(item.into_row(IssueStatus::Queued));
        }
        for item in blocked {
            if issues.iter().any(|row| row.number == item.number) {
                continue;
            }
            issues.push(item.into_row(IssueStatus::Blocked));
        }
        issues.sort_by_key(|row| row.number);
        self.issues = issues;
        match self.phase {
            Phase::Dispatch { issue } | Phase::Running { issue } | Phase::Verify { issue } => {
                self.mark_running(issue);
            }
            _ => {}
        }
    }

    fn mark_running(&mut self, issue: u32) {
        if let Some(row) = self.issues.iter_mut().find(|row| row.number == issue)
            && !matches!(row.status, IssueStatus::Completed | IssueStatus::Failed)
        {
            row.status = IssueStatus::Running;
        }
        self.select_number(issue);
    }

    fn mark_status(&mut self, issue: u32, status: IssueStatus) {
        if let Some(row) = self.issues.iter_mut().find(|row| row.number == issue) {
            row.status = status;
        }
        self.select_number(issue);
    }
}

/// Restores cooked mode, cursor, and the main screen.
///
/// Entering the alternate screen is tracked so a failed setup still unwinds
/// only the steps that succeeded. Drop, explicit restore, and the panic hook
/// all call [`restore_terminal`].
pub struct TerminalGuard {
    raw: bool,
    alt: bool,
    cursor_hidden: bool,
}

impl TerminalGuard {
    /// Enables raw mode, the alternate screen, and a hidden cursor.
    ///
    /// # Errors
    ///
    /// Returns the first terminal setup error. Drop restores partial success.
    pub fn enter() -> io::Result<(Self, Terminal<CrosstermBackend<io::Stdout>>)> {
        install_panic_hook();
        let mut guard = Self {
            raw: false,
            alt: false,
            cursor_hidden: false,
        };
        enable_raw_mode()?;
        guard.raw = true;
        execute!(stdout(), EnterAlternateScreen)?;
        guard.alt = true;
        execute!(stdout(), Hide)?;
        guard.cursor_hidden = true;
        let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        Ok((guard, terminal))
    }

    /// Restores the terminal if this guard still owns setup steps.
    pub fn restore(&mut self) {
        if self.cursor_hidden {
            let _ = execute!(stdout(), Show);
            self.cursor_hidden = false;
        }
        if self.alt {
            let _ = execute!(stdout(), LeaveAlternateScreen);
            self.alt = false;
        }
        if self.raw {
            let _ = disable_raw_mode();
            self.raw = false;
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Best-effort restore used by [`TerminalGuard`] and the panic hook.
pub fn restore_terminal() {
    let _ = execute!(stdout(), Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

/// Header facts used to seed the live Watch Board before the first event.
pub(crate) struct LiveHeader {
    pub prd: u32,
    pub repo: String,
    pub branch: String,
}

struct LiveWatch {
    tx: mpsc::Sender<WatchEvent>,
    stop: Arc<AtomicBool>,
}

impl Watch for LiveWatch {
    fn emit(&self, event: WatchEvent) {
        let _ = self.tx.send(event);
    }

    fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Runs `work` on the calling thread with a scoped UI thread for the board.
///
/// The UI thread owns raw mode and the alternate screen. `q` / Ctrl-C while
/// work is active sets the cancel flag; the orchestrator must honor it at safe
/// boundaries. This waits for `work`, then for dismiss, then joins the UI.
///
/// # Errors
///
/// Returns terminal setup errors, `work` errors (preferred), or a later UI
/// draw/input error after current work finishes.
pub(crate) fn with_live_board<T>(
    header: LiveHeader,
    work: impl FnOnce(&dyn Watch) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    thread::scope(|scope| {
        let (event_tx, event_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let ui_stop = stop.clone();
        let handle = scope.spawn(move || match TerminalGuard::enter() {
            Ok((_guard, mut terminal)) => {
                let _ = ready_tx.send(Ok(()));
                ui_loop(&mut terminal, event_rx, ui_stop, header)
            }
            Err(err) => {
                let message = err.to_string();
                let _ = ready_tx.send(Err(message.clone()));
                Err(io::Error::other(message))
            }
        });
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(message)) => return Err(message.into()),
            Err(_) => {
                return Err("nightshift: Watch Board UI thread exited during setup".into());
            }
        }
        let watch = LiveWatch {
            tx: event_tx,
            stop: stop.clone(),
        };
        let work_result = work(&watch);
        drop(watch);
        let ui_result = handle
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("Watch Board UI thread panicked")));
        match (work_result, ui_result) {
            (Err(work), _) => Err(work),
            (Ok(_value), Err(ui)) => Err(ui.into()),
            (Ok(value), Ok(())) => Ok(value),
        }
    })
}

fn ui_loop(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    rx: mpsc::Receiver<WatchEvent>,
    stop: Arc<AtomicBool>,
    header: LiveHeader,
) -> io::Result<()> {
    let result = ui_loop_inner(terminal, rx, &stop, header);
    if result.is_err() {
        stop.store(true, Ordering::SeqCst);
        restore_terminal();
    }
    result
}

fn ui_loop_inner(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    rx: mpsc::Receiver<WatchEvent>,
    stop: &AtomicBool,
    header: LiveHeader,
) -> io::Result<()> {
    let theme = Theme::from_env();
    let mut state = BoardState::live_run(header.prd, header.repo, header.branch);
    let mut phase_started = Instant::now();
    loop {
        let mut phase_changed = false;
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    let before = std::mem::discriminant(&state.phase);
                    state.apply(event);
                    if std::mem::discriminant(&state.phase) != before {
                        phase_changed = true;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        if phase_changed {
            phase_started = Instant::now();
        }
        state.elapsed = phase_started.elapsed();
        terminal.draw(|frame| render(frame, &state, &theme))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match state.handle_key(key) {
                BoardCommand::Stop => stop.store(true, Ordering::SeqCst),
                BoardCommand::Quit => return Ok(()),
                BoardCommand::Continue => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
    });
}

#[derive(Clone, Copy)]
enum LayoutMode {
    Wide,
    Compact,
    Tiny,
}

fn layout_mode(area: Rect) -> LayoutMode {
    if area.width < 40 || area.height < 8 {
        LayoutMode::Tiny
    } else if area.width < 80 {
        LayoutMode::Compact
    } else {
        LayoutMode::Wide
    }
}

/// Draws the Watch Board into `frame`. Empty areas are skipped; this never panics
/// on tiny sizes.
pub fn render(frame: &mut Frame, state: &BoardState, theme: &Theme) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = *theme;
    frame.render_widget(Block::default().style(theme.body()), area);
    match layout_mode(area) {
        LayoutMode::Wide => render_wide(frame, state, theme, area),
        LayoutMode::Compact => render_compact(frame, state, theme, area),
        LayoutMode::Tiny => render_tiny(frame, state, theme, area),
    }
    if state.help_open {
        render_help(frame, state, theme, area);
    }
}

fn render_wide(frame: &mut Frame, state: &BoardState, theme: Theme, area: Rect) {
    let header_h = header_height(area);
    let footer_h = u16::from(area.height > header_h);
    let chunks = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(0),
        Constraint::Length(footer_h),
    ])
    .split(area);
    render_header(frame, state, theme, chunks[0], LayoutMode::Wide);
    if !is_empty(chunks[1]) {
        let cols = Layout::horizontal([Constraint::Min(24), Constraint::Min(28)]).split(chunks[1]);
        render_roster(frame, state, theme, cols[0], true);
        render_details(frame, state, theme, cols[1], true);
    }
    if !is_empty(chunks[2]) {
        render_footer(frame, state, theme, chunks[2]);
    }
}

fn render_compact(frame: &mut Frame, state: &BoardState, theme: Theme, area: Rect) {
    let header_h = header_height(area);
    let footer_h = u16::from(area.height > header_h + 1);
    let rest = area.height.saturating_sub(header_h + footer_h);
    let roster_h = if rest <= 1 { rest } else { rest / 2 };
    let chunks = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Length(roster_h),
        Constraint::Min(0),
        Constraint::Length(footer_h),
    ])
    .split(area);
    render_header(frame, state, theme, chunks[0], LayoutMode::Compact);
    render_roster(frame, state, theme, chunks[1], true);
    render_details(frame, state, theme, chunks[2], true);
    if !is_empty(chunks[3]) {
        render_footer(frame, state, theme, chunks[3]);
    }
}

fn render_tiny(frame: &mut Frame, state: &BoardState, theme: Theme, area: Rect) {
    let footer_h = u16::from(area.height >= 3);
    let header_h = 1.min(area.height.saturating_sub(footer_h));
    let chunks = Layout::vertical([
        Constraint::Length(header_h.min(area.height)),
        Constraint::Min(0),
        Constraint::Length(footer_h),
    ])
    .split(area);
    render_header(frame, state, theme, chunks[0], LayoutMode::Tiny);
    render_roster(frame, state, theme, chunks[1], false);
    if !is_empty(chunks[2]) {
        render_footer(frame, state, theme, chunks[2]);
    }
}

fn header_height(area: Rect) -> u16 {
    if area.height < 6 { 1 } else { 2 }.min(area.height)
}

fn render_header(
    frame: &mut Frame,
    state: &BoardState,
    theme: Theme,
    area: Rect,
    mode: LayoutMode,
) {
    if is_empty(area) {
        return;
    }
    let mut lines = Vec::new();
    match mode {
        LayoutMode::Tiny => {
            lines.push(Line::from(vec![
                Span::styled("nightshift", theme.bold()),
                Span::raw(format!("  PRD #{}", state.prd)),
            ]));
        }
        LayoutMode::Compact | LayoutMode::Wide => {
            lines.push(Line::from(vec![
                Span::styled("nightshift", theme.bold()),
                Span::raw("  "),
                Span::styled(format!("PRD #{}", state.prd), theme.live()),
                Span::styled(format!("  {}  {}", state.repo, state.branch), theme.muted()),
                Span::raw("  "),
                Span::raw(state.prd_title.as_str()),
            ]));
            if area.height >= 2 {
                lines.push(counts_line(state, theme));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).style(theme.body()), area);
}

fn counts_line<'a>(state: &'a BoardState, theme: Theme) -> Line<'a> {
    let counts = state.counts();
    let mut spans = Vec::new();
    if let Some(notice) = &state.notice {
        spans.push(Span::styled(notice.as_str(), theme.muted()));
    }
    push_count(&mut spans, counts.completed, IssueStatus::Completed, theme);
    push_count(&mut spans, counts.running, IssueStatus::Running, theme);
    push_count(&mut spans, counts.queued, IssueStatus::Queued, theme);
    push_count(&mut spans, counts.blocked, IssueStatus::Blocked, theme);
    if counts.failed > 0 {
        push_count(&mut spans, counts.failed, IssueStatus::Failed, theme);
    }
    Line::from(spans)
}

fn push_count(spans: &mut Vec<Span<'_>>, n: usize, status: IssueStatus, theme: Theme) {
    if !spans.is_empty() {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        format!("{n} {}", status.as_str()),
        theme.status(status),
    ));
}

fn render_roster(frame: &mut Frame, state: &BoardState, theme: Theme, area: Rect, bordered: bool) {
    if is_empty(area) {
        return;
    }
    let inner_h = if bordered {
        area.height.saturating_sub(2)
    } else {
        area.height
    };
    let lines = roster_lines(state, theme, inner_h);
    let mut paragraph = Paragraph::new(lines).style(theme.body());
    if bordered {
        paragraph = paragraph.block(pane_block("issues", theme));
    }
    frame.render_widget(paragraph, area);
}

fn roster_lines(state: &BoardState, theme: Theme, height: u16) -> Vec<Line<'_>> {
    if state.issues.is_empty() || height == 0 {
        return Vec::new();
    }
    let selected = state.selected.min(state.issues.len() - 1);
    let start = visible_start(state.issues.len(), selected, height as usize);
    let end = (start + height as usize).min(state.issues.len());
    state.issues[start..end]
        .iter()
        .enumerate()
        .map(|(offset, issue)| roster_line(issue, start + offset == selected, theme))
        .collect()
}

fn visible_start(len: usize, selected: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        return 0;
    }
    let max_start = len - height;
    selected.saturating_sub(height / 2).min(max_start)
}

fn roster_line<'a>(issue: &'a IssueRow, selected: bool, theme: Theme) -> Line<'a> {
    let marker = if selected { ">" } else { " " };
    let status = issue.status.as_str();
    if selected {
        Line::from(Span::styled(
            format!("{marker} #{} {status:<9} {}", issue.number, issue.title),
            theme.body().add_modifier(Modifier::REVERSED),
        ))
    } else {
        Line::from(vec![
            Span::raw(format!("{marker} #{} ", issue.number)),
            Span::styled(format!("{status:<9}"), theme.status(issue.status)),
            Span::raw(" "),
            Span::raw(issue.title.as_str()),
        ])
    }
}

fn render_details(frame: &mut Frame, state: &BoardState, theme: Theme, area: Rect, bordered: bool) {
    if is_empty(area) {
        return;
    }
    let title = match state.selected_issue() {
        Some(issue) => format!("issue #{}", issue.number),
        None => "issue".to_string(),
    };
    let mut paragraph = Paragraph::new(detail_lines(state, theme))
        .style(theme.body())
        .wrap(Wrap { trim: true });
    if bordered {
        paragraph = paragraph.block(pane_block(&title, theme));
    }
    frame.render_widget(paragraph, area);
}

fn detail_lines<'a>(state: &'a BoardState, theme: Theme) -> Vec<Line<'a>> {
    let Some(issue) = state.selected_issue() else {
        return vec![Line::from("no issues")];
    };
    let model = issue.model.as_deref().unwrap_or("agent default");
    let effort = issue.effort.as_deref().unwrap_or("agent default");
    let mut lines = vec![
        Line::from(format!("agent   {}", issue.agent)),
        Line::from(format!("model   {model}")),
        Line::from(format!("effort  {effort}")),
        Line::from(vec![
            Span::raw("phase   "),
            Span::styled(state.phase.label(), theme.phase(&state.phase)),
        ]),
        Line::from(format!(
            "elapsed {}",
            crate::console::format_elapsed(state.elapsed)
        )),
        Line::from(vec![
            Span::raw("status  "),
            Span::styled(issue.status.as_str(), theme.status(issue.status)),
        ]),
        Line::from(Span::styled(issue.title.as_str(), theme.bold())),
    ];
    if let Phase::Failed { message, .. } = &state.phase {
        for line in message.lines() {
            lines.push(Line::from(Span::styled(
                line.to_string(),
                theme.status(IssueStatus::Failed),
            )));
        }
    }
    lines
}

fn render_footer(frame: &mut Frame, state: &BoardState, theme: Theme, area: Rect) {
    if is_empty(area) {
        return;
    }
    let text = if state.stop_pending {
        if area.width < 28 {
            "stopping after current"
        } else {
            "stop after current issue   waiting"
        }
    } else if state.live {
        if state.can_dismiss() {
            if area.width < 24 {
                "q/Enter"
            } else {
                "q / Enter dismiss"
            }
        } else if area.width < 28 {
            "q stop   ?  j/k"
        } else {
            "j/k select   ? help   q stop after current"
        }
    } else if area.width < 24 {
        "j/k  ?  q"
    } else {
        "j/k select   ? help   q quit"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, theme.muted()))),
        area,
    );
}

fn render_help(frame: &mut Frame, state: &BoardState, theme: Theme, area: Rect) {
    if is_empty(area) {
        return;
    }
    let width = 48.min(area.width);
    let height = 13.min(area.height);
    let help_area = centered(area, width, height);
    frame.render_widget(Clear, help_area);
    let quit_line = if state.live {
        "q / Ctrl-C stop after current issue"
    } else {
        "q / Ctrl-C leave preview"
    };
    let extra = if state.live {
        "Enter / q  dismiss when idle. Never kills an agent."
    } else {
        "Offline sample. No GitHub. No agent."
    };
    let lines = vec![
        Line::from(Span::styled("Watch Board help", theme.bold())),
        Line::from(""),
        Line::from("j / ↓      next issue"),
        Line::from("k / ↑      previous issue"),
        Line::from("g / Home   first issue"),
        Line::from("G / End    last issue"),
        Line::from("?          close help"),
        Line::from(quit_line),
        Line::from(""),
        Line::from(Span::styled(extra, theme.muted())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(pane_block("help", theme))
            .style(theme.body()),
        help_area,
    );
}

fn pane_block(title: &str, theme: Theme) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(theme.muted())
        .style(theme.body())
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    )
}

fn is_empty(area: Rect) -> bool {
    area.width == 0 || area.height == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn draw(state: &BoardState, theme: &Theme, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render(frame, state, theme))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn plain(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn failed_preview() -> BoardState {
        let mut state = BoardState::offline_preview();
        state.phase = Phase::Failed {
            issue: 16,
            message: "agent exited 1".to_string(),
        };
        state.selected = 4;
        state
    }

    #[test]
    fn tiny_board_does_not_panic() {
        let state = BoardState::offline_preview();
        let theme = Theme::native();
        for (w, h) in [(1, 1), (8, 3), (10, 4), (20, 6), (39, 7)] {
            let buf = draw(&state, &theme, w, h);
            assert_eq!(buf.area.width, w);
            assert_eq!(buf.area.height, h);
        }
    }

    #[test]
    fn narrow_board_keeps_unicode_title_readable() {
        let state = BoardState::offline_preview();
        let text = plain(&draw(&state, &Theme::native(), 40, 16));
        assert!(
            text.contains('子') && text.contains("Watch Board"),
            "{text}"
        );
    }

    #[test]
    fn compact_board_keeps_repo_phase_profile_and_elapsed_visible() {
        let state = BoardState::offline_preview();
        for (width, height) in [(40, 16), (60, 16)] {
            let text = plain(&draw(&state, &Theme::native(), width, height));
            assert!(
                text.contains("offline/preview"),
                "repo missing at {width}x{height}: {text}"
            );
            assert!(
                text.contains("agent #12"),
                "phase missing at {width}x{height}: {text}"
            );
            assert!(
                text.contains("1m 15s"),
                "elapsed missing at {width}x{height}: {text}"
            );
            assert!(
                text.contains("agent   pi"),
                "agent missing at {width}x{height}: {text}"
            );
            assert!(
                text.contains("model   composer"),
                "model missing at {width}x{height}: {text}"
            );
            assert!(
                text.contains("effort  high"),
                "effort missing at {width}x{height}: {text}"
            );
        }
    }

    #[test]
    fn roster_labels_status_in_text() {
        let state = BoardState::offline_preview();
        let text = plain(&draw(&state, &Theme::native(), 100, 24));
        for label in ["completed", "running", "queued", "blocked", "failed"] {
            assert!(text.contains(label), "missing {label} in {text}");
        }
        assert!(text.contains("1 completed"));
        assert!(text.contains("1 running"));
        assert!(text.contains("1 queued"));
        assert!(text.contains("1 blocked"));
        assert!(text.contains("1 failed"));
        assert!(text.contains("offline preview"));
        assert!(text.contains("PRD #15"));
        assert!(text.contains("offline/preview"));
    }

    #[test]
    fn wide_layout_shows_profile_and_phase() {
        let state = BoardState::offline_preview();
        let text = plain(&draw(&state, &Theme::native(), 100, 24));
        assert!(text.contains("agent   pi"), "{text}");
        assert!(text.contains("model   composer"), "{text}");
        assert!(text.contains("effort  high"), "{text}");
        assert!(text.contains("phase   agent #12"), "{text}");
        assert!(text.contains("elapsed 1m 15s"), "{text}");
        assert!(text.contains("j/k select"));
    }

    #[test]
    fn failed_phase_keeps_error_text_visible() {
        let state = failed_preview();
        let text = plain(&draw(&state, &Theme::native(), 80, 24));
        assert!(text.contains("failed #16"), "{text}");
        assert!(text.contains("agent exited 1"), "{text}");
        assert!(text.contains("status  failed"), "{text}");
    }

    #[test]
    fn no_color_theme_uses_no_ansi_colors() {
        let state = BoardState::offline_preview();
        let buf = draw(&state, &Theme::no_color(), 80, 20);
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                assert_eq!(cell.fg, Color::Reset, "fg at {x},{y}");
                assert_eq!(cell.bg, Color::Reset, "bg at {x},{y}");
            }
        }
    }

    #[test]
    fn from_env_selects_native_unless_no_color_is_nonempty() {
        assert_eq!(Theme::from_no_color_var(None), Theme::native());
        assert_eq!(
            Theme::from_no_color_var(Some(std::ffi::OsStr::new(""))),
            Theme::native()
        );
        assert_eq!(
            Theme::from_no_color_var(Some(std::ffi::OsStr::new("1"))),
            Theme::no_color()
        );
        assert_eq!(
            Theme::from_no_color_var(Some(std::ffi::OsStr::new("0"))),
            Theme::no_color()
        );
    }

    #[test]
    fn native_theme_paints_status_accents() {
        let state = BoardState::offline_preview();
        let buf = draw(&state, &Theme::native(), 100, 24);
        let mut colors = Vec::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                colors.push(buf[(x, y)].fg);
            }
        }
        assert!(colors.contains(&Color::Cyan));
        assert!(colors.contains(&Color::Green));
        assert!(colors.contains(&Color::Yellow));
        assert!(colors.contains(&Color::Red));
    }

    #[test]
    fn help_overlay_lists_keys() {
        let mut state = BoardState::offline_preview();
        state.help_open = true;
        let text = plain(&draw(&state, &Theme::native(), 80, 24));
        assert!(text.contains("Watch Board help"), "{text}");
        assert!(text.contains("next issue"), "{text}");
        assert!(text.contains("leave preview"), "{text}");
        assert!(text.contains("No GitHub"), "{text}");
    }

    #[test]
    fn arrows_and_vim_keys_move_selection() {
        let mut state = BoardState::offline_preview();
        assert_eq!(state.selected_issue().unwrap().number, 12);
        assert_eq!(state.handle_key(key(KeyCode::Down)), BoardCommand::Continue);
        assert_eq!(state.selected_issue().unwrap().number, 13);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('j'))),
            BoardCommand::Continue
        );
        assert_eq!(state.selected_issue().unwrap().number, 14);
        assert_eq!(state.handle_key(key(KeyCode::Up)), BoardCommand::Continue);
        assert_eq!(state.selected_issue().unwrap().number, 13);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('k'))),
            BoardCommand::Continue
        );
        assert_eq!(state.selected_issue().unwrap().number, 12);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('g'))),
            BoardCommand::Continue
        );
        assert_eq!(state.selected_issue().unwrap().number, 11);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('G'))),
            BoardCommand::Continue
        );
        assert_eq!(state.selected_issue().unwrap().number, 16);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('?'))),
            BoardCommand::Continue
        );
        assert!(state.help_open);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('q'))),
            BoardCommand::Quit
        );
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(state.handle_key(ctrl_c), BoardCommand::Quit);
    }

    #[test]
    fn empty_roster_stays_drawable() {
        let mut state = BoardState::offline_preview();
        state.issues.clear();
        state.selected = 9;
        let text = plain(&draw(&state, &Theme::native(), 60, 16));
        assert!(text.contains("no issues"), "{text}");
        assert!(state.selected_issue().is_none());
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected, 0);
    }

    fn roster(number: u32, title: &str) -> RosterIssue {
        RosterIssue {
            number,
            title: title.to_string(),
            agent: "pi".to_string(),
            model: Some("gpt".to_string()),
            effort: Some("high".to_string()),
        }
    }

    #[test]
    fn live_q_requests_stop_while_running_and_dismisses_when_done() {
        let mut state = BoardState::live_run(42, "owner/repo".into(), "main".into());
        state.phase = Phase::Running { issue: 10 };
        assert_eq!(
            state.handle_key(key(KeyCode::Char('q'))),
            BoardCommand::Stop
        );
        assert!(state.stop_pending);
        assert_eq!(state.notice.as_deref(), Some("stop after current issue"));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(state.handle_key(ctrl_c), BoardCommand::Stop);
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            BoardCommand::Continue
        );
        state.phase = Phase::Done;
        assert_eq!(state.handle_key(key(KeyCode::Enter)), BoardCommand::Quit);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('q'))),
            BoardCommand::Quit
        );
    }

    #[test]
    fn preview_enter_does_not_quit() {
        let mut state = BoardState::offline_preview();
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            BoardCommand::Continue
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Char('q'))),
            BoardCommand::Quit
        );
    }

    #[test]
    fn apply_preserves_completed_rows_across_roster_refresh() {
        let mut state = BoardState::live_run(42, "owner/repo".into(), "main".into());
        state.apply(WatchEvent::Roster {
            planned: vec![roster(10, "First"), roster(11, "Second")],
            blocked: vec![roster(12, "Blocked")],
        });
        state.apply(WatchEvent::Running { issue: 10 });
        state.apply(WatchEvent::Completed { issue: 10 });
        state.apply(WatchEvent::Roster {
            planned: vec![roster(11, "Second")],
            blocked: vec![roster(12, "Blocked")],
        });
        assert_eq!(state.issues[0].number, 10);
        assert_eq!(state.issues[0].status, IssueStatus::Completed);
        assert_eq!(state.issues[1].number, 11);
        assert_eq!(state.issues[1].status, IssueStatus::Queued);
        assert_eq!(state.issues[2].number, 12);
        assert_eq!(state.issues[2].status, IssueStatus::Blocked);
        assert_eq!(state.issues[1].agent, "pi");
        assert_eq!(state.issues[1].model.as_deref(), Some("gpt"));
        assert_eq!(state.issues[1].effort.as_deref(), Some("high"));
    }

    #[test]
    fn apply_does_not_infer_completed_when_a_row_vanishes() {
        let mut state = BoardState::live_run(42, "owner/repo".into(), "main".into());
        state.apply(WatchEvent::Roster {
            planned: vec![roster(10, "First")],
            blocked: vec![],
        });
        state.apply(WatchEvent::Running { issue: 10 });
        state.apply(WatchEvent::Roster {
            planned: vec![],
            blocked: vec![],
        });
        assert!(
            state
                .issues
                .iter()
                .all(|row| row.status != IssueStatus::Completed)
        );
    }

    #[test]
    fn apply_failed_keeps_readable_error_and_failed_row() {
        let mut state = BoardState::live_run(42, "owner/repo".into(), "main".into());
        state.apply(WatchEvent::Roster {
            planned: vec![roster(16, "Broken")],
            blocked: vec![],
        });
        state.apply(WatchEvent::Failed {
            issue: 16,
            message: "agent exited 1\ncheckout failed: fatal: not a git repository".to_string(),
        });
        let text = plain(&draw(&state, &Theme::native(), 80, 24));
        assert!(text.contains("failed #16"), "{text}");
        assert!(text.contains("agent exited 1"), "{text}");
        assert!(text.contains("fatal:"), "{text}");
        assert!(text.contains("not a git"), "{text}");
        assert_eq!(state.issues[0].status, IssueStatus::Failed);
    }

    #[test]
    fn live_footer_shows_pending_stop() {
        let mut state = BoardState::live_run(42, "owner/repo".into(), "main".into());
        state.phase = Phase::Running { issue: 10 };
        state.handle_key(key(KeyCode::Char('q')));
        let text = plain(&draw(&state, &Theme::native(), 80, 16));
        assert!(text.contains("stop after current issue"), "{text}");
    }
}
