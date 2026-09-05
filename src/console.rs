//! Cooked (non-`--tui`) monochrome terminal output for the nightshift CLI.
//!
//! Uses bold and dim ANSI attributes only so output stays readable in any
//! terminal theme. Section headers use horizontal rules only. The Watch Board
//! lives in [`crate::tui`].

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::invocation_profile::InvocationProfile;

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

fn profile_fields(profile: InvocationProfile<'_>) -> String {
    format!(
        "agent {}  model {}  reasoning effort {}",
        profile.agent.name(),
        profile.model.unwrap_or("agent default"),
        profile.reasoning_effort.unwrap_or("agent default"),
    )
}

fn invocation_profile_line(profile: InvocationProfile<'_>) -> String {
    format!("profile  {}", profile_fields(profile))
}

fn dry_run_assignment_line(
    step: usize,
    number: u32,
    title: &str,
    profile: InvocationProfile<'_>,
) -> String {
    format!(
        "{step}. issue #{number}  {title}  {}",
        profile_fields(profile)
    )
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

    /// Prints the resolved agent, model, and reasoning effort for this invocation.
    pub fn invocation_profile(&self, profile: InvocationProfile<'_>) {
        self.meta(&invocation_profile_line(profile));
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

/// Prints the simulated solve order, resolved profile per issue, and command preview for a dry run.
pub fn dry_run_planned_order(
    planned: &[(u32, &str, InvocationProfile<'_>)],
    blocked: &[(u32, &str)],
    agent_cmd: &str,
) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = print_banner(&mut out, &bold("Dry run  planned issue order"));

    if planned.is_empty() {
        let _ = writeln!(out, "{}", dim("(no issues would be solved)"));
    } else {
        for (step, (number, title, profile)) in planned.iter().enumerate() {
            let _ = writeln!(
                out,
                "{}",
                dim(&dry_run_assignment_line(step + 1, *number, title, *profile))
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
    fn invocation_profile_line_labels_agent_defaults() {
        assert_eq!(
            invocation_profile_line(crate::invocation_profile::InvocationProfile {
                agent: crate::agent::Agent::Pi,
                model: None,
                reasoning_effort: None,
            }),
            "profile  agent pi  model agent default  reasoning effort agent default"
        );
        assert_eq!(
            invocation_profile_line(crate::invocation_profile::InvocationProfile {
                agent: crate::agent::Agent::Pi,
                model: Some("gpt-5.4"),
                reasoning_effort: Some("high"),
            }),
            "profile  agent pi  model gpt-5.4  reasoning effort high"
        );
    }

    #[test]
    fn dry_run_assignment_line_includes_each_resolved_profile() {
        assert_eq!(
            dry_run_assignment_line(
                1,
                10,
                "Child 10",
                crate::invocation_profile::InvocationProfile {
                    agent: crate::agent::Agent::Pi,
                    model: Some("issue-model"),
                    reasoning_effort: Some("high"),
                },
            ),
            "1. issue #10  Child 10  agent pi  model issue-model  reasoning effort high"
        );
        assert_eq!(
            dry_run_assignment_line(
                2,
                11,
                "Child 11",
                crate::invocation_profile::InvocationProfile {
                    agent: crate::agent::Agent::Pi,
                    model: None,
                    reasoning_effort: None,
                },
            ),
            "2. issue #11  Child 11  agent pi  model agent default  reasoning effort agent default"
        );
    }

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
