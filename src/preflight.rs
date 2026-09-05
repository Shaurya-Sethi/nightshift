//! Interactive Invocation Profile Preflight collection.
//!
//! This module owns terminal I/O for batch per-issue choices. It returns a
//! Run-Ephemeral Profile Map and never persists selections. Presentation is
//! line-mode only: grouped multi-line fields, bold/dim labels, and a final
//! proceed/abort confirm.

use std::io::{BufRead, Write};

use crate::agent::Agent;
use crate::github::GithubIssue;
use crate::invocation_profile::{
    PerIssueInvocationOverride, PreflightDimensions, RunEphemeralProfileMap,
    WholeRunInvocationDefaults,
};

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const FIELD_LABEL_WIDTH: usize = 6;

/// Injectable input/output handles for Invocation Profile Preflight.
pub struct Io<'a> {
    terminal: bool,
    input: &'a mut dyn BufRead,
    output: &'a mut dyn Write,
}

impl<'a> Io<'a> {
    /// Builds preflight I/O from terminal status and caller-owned handles.
    pub fn new(terminal: bool, input: &'a mut dyn BufRead, output: &'a mut dyn Write) -> Self {
        Self {
            terminal,
            input,
            output,
        }
    }

    /// Reports whether both supplied handles belong to an interactive terminal.
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Rejects hidden or redirected input before interactive work begins.
    ///
    /// # Errors
    ///
    /// Returns an actionable error directing unattended callers to whole-run
    /// `--agent`, `--model`, and `--reasoning-effort` flags when either handle
    /// is not a terminal.
    pub fn ensure_terminal(&self) -> std::io::Result<()> {
        if self.is_terminal() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "nightshift: --pick-agents, --pick-efforts, and --pick-models require an interactive TTY; use --agent, --model, and --reasoning-effort for unattended runs",
            ))
        }
    }
}

fn bold(io: &Io<'_>, text: &str) -> String {
    if io.is_terminal() {
        format!("{BOLD}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn dim(io: &Io<'_>, text: &str) -> String {
    if io.is_terminal() {
        format!("{DIM}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn abort_error(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, message)
}

/// Reads one preflight line. Empty string is a blank choice; `q` aborts.
fn read_line(io: &mut Io<'_>) -> std::io::Result<String> {
    let mut line = String::new();
    if io.input.read_line(&mut line)? == 0 {
        return Err(abort_error(
            "Invocation Profile Preflight aborted: input closed",
        ));
    }
    let line = line.trim().to_string();
    if line.eq_ignore_ascii_case("q") || line == "\u{3}" {
        return Err(abort_error("Invocation Profile Preflight aborted"));
    }
    Ok(line)
}

fn write_agent_legend(io: &mut Io<'_>) -> std::io::Result<()> {
    writeln!(io.output, "{}", bold(io, "Agent choices"))?;
    for (index, agent) in Agent::all().iter().enumerate() {
        writeln!(
            io.output,
            "{}",
            dim(io, &format!("  {} {}", index + 1, agent.name()))
        )?;
    }
    Ok(())
}

fn write_effort_legend(io: &mut Io<'_>, agent_name: &str, efforts: &[&str]) -> std::io::Result<()> {
    writeln!(
        io.output,
        "{}",
        bold(io, &format!("Effort choices for {agent_name}"))
    )?;
    for (index, effort) in efforts.iter().enumerate() {
        writeln!(
            io.output,
            "{}",
            dim(io, &format!("  {} {effort}", index + 1))
        )?;
    }
    Ok(())
}

fn write_model_hint(io: &mut Io<'_>) -> std::io::Result<()> {
    writeln!(
        io.output,
        "{}",
        dim(io, "model = free string; blank keeps default")
    )
}

fn write_issue_header(io: &mut Io<'_>, issue: &GithubIssue) -> std::io::Result<()> {
    writeln!(io.output)?;
    writeln!(
        io.output,
        "{}",
        bold(io, &format!("#{}  {}", issue.number, issue.title))
    )
}

fn write_field_prompt(io: &mut Io<'_>, label: &str, default: &str) -> std::io::Result<()> {
    let label = format!("{label:<FIELD_LABEL_WIDTH$}");
    write!(
        io.output,
        "  {} {}: ",
        bold(io, &label),
        dim(io, &format!("[Enter = {default}]"))
    )?;
    io.output.flush()
}

fn parse_legend_index(selection: &str) -> Option<usize> {
    (selection.len() == 1)
        .then(|| selection.parse::<usize>().ok())?
        .and_then(|key| key.checked_sub(1))
}

fn parse_agent_selection(selection: &str) -> std::io::Result<Option<Agent>> {
    if selection.is_empty() {
        return Ok(None);
    }
    parse_legend_index(selection)
        .and_then(|index| Agent::all().get(index).copied())
        .map(Some)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "preflight selection is not in the agent legend",
            )
        })
}

fn parse_effort_selection(selection: &str, efforts: &[&str]) -> std::io::Result<Option<String>> {
    if selection.is_empty() {
        return Ok(None);
    }
    match parse_legend_index(selection).and_then(|index| efforts.get(index).copied()) {
        Some(effort) => Ok(Some(effort.to_string())),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "preflight selection is not in the effort legend",
        )),
    }
}

fn confirm_proceed(io: &mut Io<'_>, dry_run: bool) -> std::io::Result<()> {
    writeln!(io.output)?;
    let prompt = if dry_run {
        "Continue dry-run? [Enter = yes, q = abort]: "
    } else {
        "Start run with these profiles? [Enter = yes, q = abort]: "
    };
    write!(io.output, "{}", dim(io, prompt))?;
    io.output.flush()?;

    let selection = read_line(io)?;
    if selection.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "preflight confirm accepts Enter to proceed or q to abort",
        ))
    }
}

fn write_skip_reason(io: &mut Io<'_>, field: &str, reason: &str) -> std::io::Result<()> {
    writeln!(
        io.output,
        "{}",
        dim(io, &format!("  {field} skipped: {reason}"))
    )
}

fn effort_skip_reason(agent: Agent) -> &'static str {
    match agent {
        Agent::Cursor => "model-encoded effort",
        Agent::Antigravity => "no separate effort control",
        _ => "agent does not support separate effort control",
    }
}

/// Collects enabled Invocation Profile Preflight dimensions for every planned child issue.
///
/// Fields are collected in agent, model, effort order. Each row resolves its
/// agent before capability checks, so unsupported model or effort fields are
/// skipped without blocking other rows. Blank fields remain absent for
/// Same-Agent Defaults Inheritance during invocation resolution.
///
/// # Errors
///
/// Returns an error when I/O fails, input is not a TTY, a selection is not in
/// a displayed legend, or the user aborts preflight.
pub fn pick_profiles(
    planned: &[GithubIssue],
    defaults: WholeRunInvocationDefaults<'_>,
    dimensions: PreflightDimensions,
    dry_run: bool,
    io: &mut Io<'_>,
) -> std::io::Result<RunEphemeralProfileMap> {
    io.ensure_terminal()?;
    if dimensions.agents {
        write_agent_legend(io)?;
    }
    if dimensions.models {
        write_model_hint(io)?;
    }

    let mut profiles = RunEphemeralProfileMap::new();
    let mut effort_legend_agent = None;
    for issue in planned {
        write_issue_header(io, issue)?;
        let agent = if dimensions.agents {
            write_field_prompt(io, "agent", defaults.agent.name())?;
            parse_agent_selection(&read_line(io)?)?
        } else {
            None
        };
        let resolved_agent = agent.unwrap_or(defaults.agent);
        let inherits_defaults = resolved_agent == defaults.agent;

        let model = if dimensions.models {
            if resolved_agent.ensure_model_supported().is_ok() {
                write_field_prompt(
                    io,
                    "model",
                    if inherits_defaults {
                        defaults.model.unwrap_or("agent default")
                    } else {
                        "agent default"
                    },
                )?;
                let model = read_line(io)?;
                (!model.is_empty()).then_some(model)
            } else {
                write_skip_reason(io, "model", "agent does not support --model")?;
                None
            }
        } else {
            None
        };

        let reasoning_effort = if dimensions.efforts || dimensions.models {
            if let Some(efforts) = resolved_agent.supported_reasoning_efforts() {
                if effort_legend_agent != Some(resolved_agent) {
                    write_effort_legend(io, resolved_agent.name(), efforts)?;
                    effort_legend_agent = Some(resolved_agent);
                }
                write_field_prompt(
                    io,
                    "effort",
                    if inherits_defaults {
                        defaults.reasoning_effort.unwrap_or("agent default")
                    } else {
                        "agent default"
                    },
                )?;
                parse_effort_selection(&read_line(io)?, efforts)?
            } else {
                write_skip_reason(io, "effort", effort_skip_reason(resolved_agent))?;
                None
            }
        } else {
            None
        };

        profiles.insert(
            issue.number,
            PerIssueInvocationOverride {
                agent,
                model,
                reasoning_effort,
            },
        );
    }

    confirm_proceed(io, dry_run)?;
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::{Io, pick_profiles};
    use crate::agent::Agent;
    use crate::github::GithubIssue;
    use crate::invocation_profile::{PreflightDimensions, WholeRunInvocationDefaults};
    use std::io::Cursor;

    fn issue(number: u32) -> GithubIssue {
        GithubIssue {
            number,
            title: format!("Child {number}"),
            body: String::new(),
        }
    }

    fn agents() -> PreflightDimensions {
        PreflightDimensions {
            agents: true,
            ..PreflightDimensions::default()
        }
    }

    fn efforts() -> PreflightDimensions {
        PreflightDimensions {
            efforts: true,
            ..PreflightDimensions::default()
        }
    }

    fn models() -> PreflightDimensions {
        PreflightDimensions {
            models: true,
            ..PreflightDimensions::default()
        }
    }

    #[test]
    fn pick_agents_records_selection_and_blank_default() {
        let planned = [issue(10), issue(11)];
        let mut input = Cursor::new(b"2\n\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            agents(),
            false,
            &mut io,
        )
        .expect("agent selections should build a profile map");

        assert_eq!(profiles[&10].agent, Some(Agent::Codex));
        assert_eq!(profiles[&11].agent, None);
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("Agent choices"));
        assert!(output.contains("2 codex"));
        assert!(output.contains("[Enter = pi]"));
        assert!(output.contains("Start run with these profiles?"));
    }

    #[test]
    fn pick_profiles_stacks_agents_and_efforts_with_row_native_legends() {
        let planned = [issue(10), issue(11)];
        let mut input = Cursor::new(b"2\n1\n\n5\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            PreflightDimensions {
                agents: true,
                efforts: true,
                models: false,
            },
            false,
            &mut io,
        )
        .expect("stacked selections should build a profile map");

        assert_eq!(profiles[&10].agent, Some(Agent::Codex));
        assert_eq!(profiles[&10].reasoning_effort.as_deref(), Some("minimal"));
        assert_eq!(profiles[&11].agent, None);
        assert_eq!(profiles[&11].reasoning_effort.as_deref(), Some("high"));
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("Agent choices"));
        assert!(output.contains("Effort choices for codex"));
        assert!(output.contains("Effort choices for pi"));
    }

    #[test]
    fn pick_profiles_reuses_effort_legend_when_resolved_agent_does_not_change() {
        let planned = [issue(10), issue(11)];
        let mut input = Cursor::new(b"5\n\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            PreflightDimensions {
                agents: false,
                efforts: true,
                models: false,
            },
            false,
            &mut io,
        )
        .expect("unchanged agent rows should reuse their legend");

        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert_eq!(output.matches("Effort choices for pi").count(), 1);
    }

    #[test]
    fn pick_profiles_stacks_agents_models_and_native_efforts() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"2\nissue-model\n4\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: Some("run-model"),
                reasoning_effort: Some("medium"),
            },
            PreflightDimensions {
                agents: true,
                efforts: false,
                models: true,
            },
            false,
            &mut io,
        )
        .expect("stacked selections should collect every capable column");

        assert_eq!(profiles[&10].agent, Some(Agent::Codex));
        assert_eq!(profiles[&10].model.as_deref(), Some("issue-model"));
        assert_eq!(profiles[&10].reasoning_effort.as_deref(), Some("high"));
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("Effort choices for codex"));
        assert!(output.contains("model"));
        assert!(output.contains("effort"));
    }

    #[test]
    fn pick_profiles_skips_incapable_columns_for_agent_model_rows() {
        let planned = [issue(10), issue(11)];
        let mut input = Cursor::new(b"4\ncursor-thinking-high\n3\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: Some("run-model"),
                reasoning_effort: Some("medium"),
            },
            PreflightDimensions {
                agents: true,
                efforts: false,
                models: true,
            },
            false,
            &mut io,
        )
        .expect("row-capable columns should collect supported fields only");

        assert_eq!(profiles[&10].agent, Some(Agent::Cursor));
        assert_eq!(profiles[&10].model.as_deref(), Some("cursor-thinking-high"));
        assert_eq!(profiles[&10].reasoning_effort, None);
        assert_eq!(profiles[&11].agent, Some(Agent::Antigravity));
        assert_eq!(profiles[&11].model, None);
        assert_eq!(profiles[&11].reasoning_effort, None);
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("effort skipped: model-encoded effort"));
        assert!(output.contains("model skipped: agent does not support --model"));
        assert!(output.contains("effort skipped: no separate effort control"));
    }

    #[test]
    fn pick_efforts_aborts_without_returning_a_partial_map() {
        let planned = [issue(10), issue(11)];
        let mut input = Cursor::new(b"1\nq\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let error = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            efforts(),
            false,
            &mut io,
        )
        .expect_err("q must abort the complete preflight");

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(error.to_string().contains("Preflight aborted"));
    }

    #[test]
    fn pick_efforts_treats_end_of_input_as_abort() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let error = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            efforts(),
            false,
            &mut io,
        )
        .expect_err("closed terminal input must not create a partial profile map");

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    }

    #[test]
    fn pick_efforts_rejects_multi_key_selection() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"03\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let error = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            efforts(),
            false,
            &mut io,
        )
        .expect_err("effort selection must use one legend key");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn pick_efforts_rejects_non_terminal_input_before_reading() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(false, &mut input, &mut output);

        let error = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            efforts(),
            false,
            &mut io,
        )
        .expect_err("preflight must not prompt on a non-terminal stream");

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("--reasoning-effort"));
    }

    #[test]
    fn pick_efforts_records_single_key_choice_and_blank_override() {
        let planned = [issue(10), issue(11)];
        let mut input = Cursor::new(b"5\n\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: Some("fixed-model"),
                reasoning_effort: Some("medium"),
            },
            efforts(),
            false,
            &mut io,
        )
        .expect("scripted choices should build a profile map");

        assert_eq!(profiles[&10].reasoning_effort.as_deref(), Some("high"));
        assert_eq!(profiles[&10].model, None);
        assert_eq!(profiles[&11].reasoning_effort, None);
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("Effort choices for pi"));
        assert!(output.contains("#10  Child 10"));
        assert!(output.contains("effort"));
        assert!(output.contains("[Enter = medium]"));
        assert!(output.contains("Start run with these profiles?"));
        assert!(!output.contains("Issue #10"));
    }

    #[test]
    fn pick_efforts_dry_run_uses_continue_confirm_wording() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"1\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            efforts(),
            true,
            &mut io,
        )
        .expect("dry-run confirm Enter should proceed");

        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("Continue dry-run?"));
        assert!(!output.contains("Start run with these profiles?"));
    }

    #[test]
    fn pick_models_records_model_and_effort_overrides_independently() {
        let planned = [issue(10), issue(11), issue(12)];
        let mut input = Cursor::new(b"issue-model\n5\n\n4\nfallback-model\n\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: Some("run-model"),
                reasoning_effort: Some("medium"),
            },
            models(),
            false,
            &mut io,
        )
        .expect("scripted choices should build a full profile map");

        assert_eq!(profiles[&10].model.as_deref(), Some("issue-model"));
        assert_eq!(profiles[&10].reasoning_effort.as_deref(), Some("high"));
        assert_eq!(profiles[&11].model, None);
        assert_eq!(profiles[&11].reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(profiles[&12].model.as_deref(), Some("fallback-model"));
        assert_eq!(profiles[&12].reasoning_effort, None);
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(!output.contains("Model choices for pi"));
        assert!(output.contains("model = free string; blank keeps default"));
        assert!(output.contains("Effort choices for pi"));
        assert!(output.contains("#10  Child 10"));
        assert!(output.contains("model"));
        assert!(output.contains("effort"));
        assert!(output.contains("[Enter = run-model]"));
        assert!(output.contains("Start run with these profiles?"));
    }

    #[test]
    fn pick_models_omits_effort_controls_for_model_encoded_effort_agents() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"cursor-thinking-high\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Cursor,
                model: None,
                reasoning_effort: None,
            },
            models(),
            false,
            &mut io,
        )
        .expect("cursor full preflight should collect only model slugs");

        assert_eq!(profiles[&10].model.as_deref(), Some("cursor-thinking-high"));
        assert_eq!(profiles[&10].reasoning_effort, None);
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(!output.contains("Model choices for cursor"));
        assert!(output.contains("model = free string; blank keeps default"));
        assert!(!output.contains("Effort choices"));
        assert!(output.contains("effort skipped: model-encoded effort"));
        assert!(output.contains("model"));
        assert!(output.contains("#10  Child 10"));
    }

    #[test]
    fn pick_models_groups_model_and_effort_under_one_issue_header() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"m\n2\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Pi,
                model: None,
                reasoning_effort: None,
            },
            models(),
            false,
            &mut io,
        )
        .expect("grouped prompts should accept scripted input");

        let output = String::from_utf8(output).expect("preflight output is utf-8");
        let header_count = output.matches("#10  Child 10").count();
        assert_eq!(header_count, 1, "issue header must print once per issue");
        assert!(
            !output.contains("Issue #10 Child 10 model"),
            "must not use the old single-line model prompt"
        );
        assert!(
            !output.contains("Issue #10 Child 10 effort"),
            "must not use the old single-line effort prompt"
        );
    }
}
