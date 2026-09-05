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
use crate::prompt::{PerIssuePrompt, PromptMode};

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
    /// `--agent`, `--model`, `--reasoning-effort`, `--prompt-file`, and
    /// `--append-prompt-file` flags when either handle is not a terminal.
    pub fn ensure_terminal(&self) -> std::io::Result<()> {
        if self.is_terminal() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "nightshift: --pick-agents, --pick-efforts, --pick-models, and --pick-prompts require an interactive TTY; use --agent, --model, --reasoning-effort, --prompt-file, and --append-prompt-file for unattended runs",
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
    if line.eq_ignore_ascii_case("q") {
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

fn write_prompt_mode_legend(io: &mut Io<'_>) -> std::io::Result<()> {
    writeln!(io.output, "{}", bold(io, "Prompt mode"))?;
    writeln!(io.output, "{}", dim(io, "  1 append"))?;
    writeln!(io.output, "{}", dim(io, "  2 replace"))?;
    writeln!(
        io.output,
        "{}",
        dim(io, "prompt = path; blank keeps run-wide")
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

fn parse_prompt_mode(selection: &str) -> std::io::Result<PromptMode> {
    if selection.is_empty() {
        return Ok(PromptMode::Append);
    }
    match parse_legend_index(selection) {
        Some(0) => Ok(PromptMode::Append),
        Some(1) => Ok(PromptMode::Replace),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "preflight selection is not in the prompt mode legend",
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
/// Fields are collected in agent, model, effort, prompt, mode order for enabled
/// columns. Each row resolves its agent before capability checks, so unsupported
/// model or effort fields are skipped without blocking other rows. Prompt and
/// mode are gated only on `dimensions.prompts`. Blank agent/model/effort fields
/// remain absent for Same-Agent Defaults Inheritance. A blank prompt path
/// inherits the run-wide directive policy; a supplied path is loaded after every
/// row and before confirm.
///
/// # Errors
///
/// Returns an error when I/O fails, input is not a TTY, a selection is not in
/// a displayed legend, a picked prompt file cannot be read, or the user aborts
/// preflight.
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
    if dimensions.prompts {
        write_prompt_mode_legend(io)?;
    }

    struct PendingPrompt {
        issue: u32,
        path: String,
        mode: PromptMode,
    }

    let mut profiles = RunEphemeralProfileMap::new();
    let mut pending: Vec<PendingPrompt> = Vec::new();
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

        if dimensions.prompts {
            write_field_prompt(io, "prompt", "run-wide")?;
            let path = read_line(io)?;
            if path.is_empty() {
                write_skip_reason(io, "mode", "inherit run-wide")?;
            } else {
                write_field_prompt(io, "mode", "append")?;
                let mode = parse_prompt_mode(&read_line(io)?)?;
                pending.push(PendingPrompt {
                    issue: issue.number,
                    path,
                    mode,
                });
            }
        }

        profiles.insert(
            issue.number,
            PerIssueInvocationOverride {
                agent,
                model,
                reasoning_effort,
                ..PerIssueInvocationOverride::default()
            },
        );
    }

    for item in &pending {
        let contents = crate::prompt::load_directives(std::path::Path::new(&item.path))
            .map_err(std::io::Error::other)?;
        profiles
            .get_mut(&item.issue)
            .expect("row inserted in the same pass")
            .prompt = Some(PerIssuePrompt {
            mode: item.mode,
            contents,
        });
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
    use crate::prompt::PromptMode;
    use std::io::Cursor;
    use std::path::PathBuf;

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

    fn prompts() -> PreflightDimensions {
        PreflightDimensions {
            prompts: true,
            ..PreflightDimensions::default()
        }
    }

    fn pi_defaults() -> WholeRunInvocationDefaults<'static> {
        WholeRunInvocationDefaults {
            agent: Agent::Pi,
            model: None,
            reasoning_effort: None,
        }
    }

    fn write_temp_prompt(test_name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "nightshift-preflight-prompt-{test_name}-{}",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("write temp prompt file");
        path
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
                ..PreflightDimensions::default()
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
                efforts: true,
                ..PreflightDimensions::default()
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
                models: true,
                ..PreflightDimensions::default()
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
                models: true,
                ..PreflightDimensions::default()
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

    #[test]
    fn pick_prompts_blank_path_stores_none_and_skips_mode() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
            .expect("blank path should inherit run-wide");

        assert_eq!(profiles[&10].prompt, None);
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("mode skipped: inherit run-wide"));
        assert!(output.contains("Start run with these profiles?"));
        assert!(output.contains("Prompt mode"));
        assert!(output.contains("prompt = path; blank keeps run-wide"));
    }

    #[test]
    fn pick_prompts_path_enter_mode_is_append() {
        let path = write_temp_prompt("enter-mode-append", "row extra\n");
        let stdin = format!("{}\n\n\n", path.display());
        let planned = [issue(10)];
        let mut input = Cursor::new(stdin.into_bytes());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
            .expect("enter on mode should append");

        let prompt = profiles[&10].prompt.as_ref().expect("path should snapshot");
        assert_eq!(prompt.mode, PromptMode::Append);
        assert_eq!(prompt.contents, "row extra");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pick_prompts_path_2_is_replace() {
        let path = write_temp_prompt("mode-2-replace", "replace only\n");
        let stdin = format!("{}\n2\n\n", path.display());
        let planned = [issue(10)];
        let mut input = Cursor::new(stdin.into_bytes());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
            .expect("mode 2 should replace");

        let prompt = profiles[&10].prompt.as_ref().expect("path should snapshot");
        assert_eq!(prompt.mode, PromptMode::Replace);
        assert_eq!(prompt.contents, "replace only");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pick_prompts_path_1_is_append() {
        let path = write_temp_prompt("mode-1-append", "appended extra\n");
        let stdin = format!("{}\n1\n\n", path.display());
        let planned = [issue(10)];
        let mut input = Cursor::new(stdin.into_bytes());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
            .expect("mode 1 should append");

        let prompt = profiles[&10].prompt.as_ref().expect("path should snapshot");
        assert_eq!(prompt.mode, PromptMode::Append);
        assert_eq!(prompt.contents, "appended extra");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pick_prompts_rejects_invalid_mode_keys() {
        for key in ["3", "append", "03"] {
            let planned = [issue(10)];
            let stdin = format!("/no/such/nightshift-prompt\n{key}\n");
            let mut input = Cursor::new(stdin.into_bytes());
            let mut output = Vec::new();
            let mut io = Io::new(true, &mut input, &mut output);

            let error = pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
                .expect_err("invalid mode key must fail");

            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(
                error
                    .to_string()
                    .contains("preflight selection is not in the prompt mode legend")
            );
        }
    }

    #[test]
    fn pick_prompts_q_on_path_aborts_without_confirm() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"q\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let error = pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
            .expect_err("q on the path line must abort");

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(!output.contains("Start run with these profiles?"));
    }

    #[test]
    fn pick_prompts_missing_file_fails_before_confirm() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"/no/such/nightshift-prompt-missing\n1\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let error = pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
            .expect_err("missing prompt file must fail the run");

        assert!(
            error
                .to_string()
                .contains("nightshift: failed to read prompt file:")
        );
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(!output.contains("Start run with these profiles?"));
    }

    #[test]
    fn pick_prompts_two_missing_files_fail_in_plan_order() {
        let planned = [issue(10), issue(11)];
        let mut input = Cursor::new(
            b"/no/such/nightshift-prompt-issue-10\n1\n/no/such/nightshift-prompt-issue-11\n1\n"
                .as_slice(),
        );
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let error = pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
            .expect_err("first unreadable pending path must fail");

        let message = error.to_string();
        assert!(message.contains("nightshift: failed to read prompt file:"));
        assert!(message.contains("/no/such/nightshift-prompt-issue-10"));
        assert!(!message.contains("/no/such/nightshift-prompt-issue-11"));
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(!output.contains("Start run with these profiles?"));
    }

    #[test]
    fn pick_prompts_stacks_after_agent_model_effort() {
        let path = write_temp_prompt("stacked-row", "stacked extra\n");
        let stdin = format!("2\nissue-model\n4\n{}\n1\n\n", path.display());
        let planned = [issue(10)];
        let mut input = Cursor::new(stdin.into_bytes());
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
                models: true,
                prompts: true,
                ..PreflightDimensions::default()
            },
            false,
            &mut io,
        )
        .expect("stacked prompt column should follow agent model effort");

        assert_eq!(profiles[&10].agent, Some(Agent::Codex));
        assert_eq!(profiles[&10].model.as_deref(), Some("issue-model"));
        assert_eq!(profiles[&10].reasoning_effort.as_deref(), Some("high"));
        let prompt = profiles[&10].prompt.as_ref().expect("path should snapshot");
        assert_eq!(prompt.mode, PromptMode::Append);
        assert_eq!(prompt.contents, "stacked extra");
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("Agent choices"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pick_prompts_follows_cursor_model_skip_without_extra_effort_stdin() {
        let path = write_temp_prompt("cursor-skip-then-prompt", "cursor extra\n");
        let stdin = format!("cursor-thinking-high\n{}\n2\n\n", path.display());
        let planned = [issue(10)];
        let mut input = Cursor::new(stdin.into_bytes());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        let profiles = pick_profiles(
            &planned,
            WholeRunInvocationDefaults {
                agent: Agent::Cursor,
                model: None,
                reasoning_effort: None,
            },
            PreflightDimensions {
                models: true,
                prompts: true,
                ..PreflightDimensions::default()
            },
            false,
            &mut io,
        )
        .expect("prompt should follow skipped effort with no extra stdin");

        assert_eq!(profiles[&10].model.as_deref(), Some("cursor-thinking-high"));
        assert_eq!(profiles[&10].reasoning_effort, None);
        let prompt = profiles[&10].prompt.as_ref().expect("path should snapshot");
        assert_eq!(prompt.mode, PromptMode::Replace);
        assert_eq!(prompt.contents, "cursor extra");
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("effort skipped: model-encoded effort"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pick_prompts_alone_omits_agent_and_effort_ui() {
        let planned = [issue(10)];
        let mut input = Cursor::new(b"\n\n".as_slice());
        let mut output = Vec::new();
        let mut io = Io::new(true, &mut input, &mut output);

        pick_profiles(&planned, pi_defaults(), prompts(), false, &mut io)
            .expect("prompts-only preflight should confirm after path");

        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("Prompt mode"));
        assert!(output.contains("#10  Child 10"));
        assert!(output.contains("Start run with these profiles?"));
        assert!(!output.contains("Agent choices"));
        assert!(!output.contains("Effort choices"));
        assert!(!output.contains("agent skipped"));
    }
}
