//! Monochrome terminal output for the nightshift CLI.
//!
//! Uses bold and dim ANSI attributes only so output stays readable in any
//! terminal theme. Section headers use horizontal rules only.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BORDER_WIDTH: usize = 62;
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

fn use_styles() -> bool {
    std::io::stdout().is_terminal()
}

fn bold(text: &str) -> String {
    if use_styles() {
        format!("{BOLD}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn dim(text: &str) -> String {
    if use_styles() {
        format!("{DIM}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn horizontal_rule() -> String {
    "-".repeat(BORDER_WIDTH)
}

fn banner_title(content: &str) -> String {
    let max_len = BORDER_WIDTH;
    if content.len() > max_len {
        format!("{}...", &content[..max_len.saturating_sub(3)])
    } else {
        content.to_string()
    }
}

fn print_banner(out: &mut impl Write, title: &str) -> std::io::Result<()> {
    writeln!(out, "{}", horizontal_rule())?;
    writeln!(out, "{}", banner_title(title))?;
    writeln!(out, "{}", horizontal_rule())
}

/// Formats a duration for issue-run footers.
///
/// # Examples
///
/// ```
/// # use std::time::Duration;
/// # use nightshift::console::format_elapsed;
/// assert_eq!(format_elapsed(Duration::from_secs(90)), "1m 30s");
/// assert_eq!(format_elapsed(Duration::from_secs(3_661)), "1h 1m 1s");
/// ```
pub fn format_elapsed(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    match (hours, minutes, seconds) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, s) => format!("{h}h {m}m {s}s"),
    }
}

fn format_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix {secs}")
}

/// Prints the session header when a PRD loop starts.
pub fn session_start(prd: u32) {
    let mut out = std::io::stdout().lock();
    let _ = print_banner(&mut out, &bold(&format!("Nightshift  PRD #{prd}")));
    let _ = writeln!(out, "{}", dim(&format!("started {}", format_timestamp())));
}

/// Opens a bordered block when work begins on an issue.
pub struct IssueRun {
    number: u32,
    started: Instant,
}

impl IssueRun {
    /// Prints the issue header and returns a timer for the run footer.
    pub fn begin(number: u32, title: &str) -> Self {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out);
        let _ = print_banner(&mut out, &bold(&format!("Issue #{number}  {title}")));
        Self {
            number,
            started: Instant::now(),
        }
    }

    /// Prints dim metadata lines below the issue header.
    pub fn meta(&self, line: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", dim(line));
    }

    /// Prints the completion block with elapsed duration.
    pub fn complete(self) {
        let elapsed = format_elapsed(self.started.elapsed());
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out);
        let _ = print_banner(
            &mut out,
            &bold(&format!("Completed  issue #{}", self.number)),
        );
        let _ = writeln!(out, "{}", dim(&format!("elapsed {elapsed}")));
    }
}

/// Prints the simulated solve order for a dry run, then the agent command preview.
pub fn dry_run_planned_order(planned: &[(u32, &str)], blocked: &[(u32, &str)], agent_cmd: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = print_banner(&mut out, &bold("Dry run  planned issue order"));

    if planned.is_empty() {
        let _ = writeln!(out, "{}", dim("(no issues would be solved)"));
    } else {
        for (step, (number, title)) in planned.iter().enumerate() {
            let _ = writeln!(
                out,
                "{}",
                dim(&format!("{}. issue #{number}  {title}", step + 1))
            );
        }
    }

    if !blocked.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", dim("blocked (not in plan)"));
        for (number, title) in blocked {
            let _ = writeln!(out, "{}", dim(&format!("- issue #{number}  {title}")));
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "{}", dim(&format!("would invoke {agent_cmd}")));
}

/// Prints a dry-run prompt preview below the dry-run block.
pub fn dry_run_prompt(prompt: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", dim("prompt preview"));
    let _ = writeln!(out, "{prompt}");
}

/// Prints a loop-completion message.
pub fn loop_complete(message: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = print_banner(&mut out, &bold(message));
}

/// Prints git hygiene status in dim text.
pub fn git_hygiene(repo: &str, base_branch: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "{}",
        dim(&format!("git hygiene  {repo}  branch {base_branch}"))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_elapsed_seconds_only() {
        assert_eq!(format_elapsed(Duration::from_secs(45)), "45s");
    }

    #[test]
    fn format_elapsed_minutes_and_seconds() {
        assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 5s");
    }

    #[test]
    fn format_elapsed_hours() {
        assert_eq!(format_elapsed(Duration::from_secs(3_661)), "1h 1m 1s");
    }

    #[test]
    fn banner_title_clips_long_content() {
        let title = banner_title(&"x".repeat(120));
        assert!(!title.contains('|'));
        assert!(title.len() <= BORDER_WIDTH);
        assert!(title.ends_with("..."));
    }
}
