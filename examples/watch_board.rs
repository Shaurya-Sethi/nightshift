//! Offline Watch Board preview.
//!
//! Drives [`nightshift::tui::render`] with labeled sample state covering
//! running, completed, blocked, queued, and failed rows. Does not call GitHub
//! or start an agent.
//!
//! ```text
//! cargo run --example watch_board
//! ```
//!
//! Keys: `j`/`k` or arrows move, `?` help, `q` or Ctrl-C leave. The terminal
//! is restored on exit, error, and panic.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use nightshift::tui::{BoardCommand, BoardState, TerminalGuard, Theme, render};

fn main() {
    if let Err(err) = run_preview() {
        eprintln!("watch_board preview: {err}");
        std::process::exit(1);
    }
}

fn run_preview() -> io::Result<()> {
    let theme = Theme::from_env();
    let mut state = BoardState::offline_preview();
    let started = Instant::now()
        .checked_sub(state.elapsed)
        .unwrap_or_else(Instant::now);
    let (_guard, mut terminal) = TerminalGuard::enter()?;
    loop {
        state.elapsed = started.elapsed();
        terminal.draw(|frame| render(frame, &state, &theme))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if state.handle_key(key) == BoardCommand::Quit {
                    return Ok(());
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}
