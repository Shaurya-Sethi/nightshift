//! Coding-agent selection and execution.
//!
//! Nightshift renders a full issue prompt and sends it to a configured agent
//! process through stdin. [`crate::agent::Agent`] owns the command names and
//! argument lists, while [`crate::agent::AgentRunner`] lets the orchestrator run
//! a real process in production and a fake runner in tests.

use clap::ValueEnum;
use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};

use crate::invocation_profile::InvocationProfile;

/// Coding-agent CLI variants supported by nightshift.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    /// Anthropic Claude Code, invoked as `claude`.
    Claude,
    /// OpenAI Codex CLI, invoked as `codex exec -`.
    Codex,
    /// Google Antigravity CLI, invoked as `agy`.
    Antigravity,
    /// Cursor agent CLI, invoked as `agent`.
    Cursor,
    /// Pi Coding Agent, invoked as `pi`.
    ///
    /// Pi has no sub-agent support; built-in directives omit that step.
    Pi,
}

impl Agent {
    /// Returns every Nightshift-compatible coding agent in picker order.
    ///
    /// This list describes supported invocation capability, not installed
    /// binaries. Preflight deliberately does not inspect `PATH`.
    pub const fn all() -> &'static [Self] {
        &[
            Self::Claude,
            Self::Codex,
            Self::Antigravity,
            Self::Cursor,
            Self::Pi,
        ]
    }

    /// Returns the CLI program and flags for this agent.
    ///
    /// The compiled issue prompt is written to the child process stdin after
    /// spawn, which keeps large prompts off argv. Only [`Agent::Codex`] uses `-`
    /// as a documented stdin marker. Other agents are invoked with their
    /// non-interactive flags and no literal prompt argument.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nightshift::agent::Agent;
    ///
    /// let (program, args) = Agent::Codex.get_command();
    /// assert_eq!(program, "codex");
    /// assert!(args.contains(&"-"));
    /// ```
    pub fn get_command(self) -> (&'static str, Vec<&'static str>) {
        match self {
            // claude -p reads piped stdin when no positional prompt is given
            Self::Claude => ("claude", vec!["-p", "--dangerously-skip-permissions"]),
            // documented CLI name is `agent`; -p with no positional prompt accepts stdin
            Self::Cursor => ("agent", vec!["-p", "--force", "--trust"]),
            // pi -p merges piped stdin into the initial prompt
            Self::Pi => ("pi", vec!["-p"]),
            // codex exec documents `-` as "read instructions from stdin"
            Self::Codex => ("codex", vec!["exec", "-", "--ephemeral"]),
            // antigravity-cli is invoked as `agy`
            Self::Antigravity => ("agy", vec!["-p", "--dangerously-skip-permissions"]),
        }
    }

    /// Returns this agent's documented static reasoning-effort values, if known.
    ///
    /// The values are capability-level validation only. Agents remain
    /// responsible for model-specific effort restrictions.
    pub fn supported_reasoning_efforts(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Pi => Some(&["off", "minimal", "low", "medium", "high", "xhigh", "max"]),
            Self::Claude => Some(&["low", "medium", "high", "max"]),
            Self::Codex => Some(&["minimal", "low", "medium", "high", "xhigh"]),
            Self::Antigravity | Self::Cursor => None,
        }
    }

    /// Returns the CLI program and flags for a resolved invocation profile.
    ///
    /// Model strings are passed through without catalog validation. Reasoning
    /// effort is validated only against the selected agent's native enum.
    ///
    /// # Errors
    ///
    /// Returns an error when a requested model or reasoning effort is not
    /// supported by this agent, or when effort is outside its native enum.
    pub fn get_command_with_profile(
        self,
        profile: InvocationProfile<'_>,
    ) -> Result<(&'static str, Vec<String>), String> {
        if profile.model.is_some() {
            self.ensure_model_supported()?;
        }
        if let Some(effort) = profile.reasoning_effort {
            self.validate_reasoning_effort(effort)?;
        }

        let (program, base_args) = self.get_command();
        let mut args: Vec<String> = base_args.into_iter().map(str::to_string).collect();

        let mut extra = Vec::new();
        if let Some(model) = profile.model {
            extra.push("--model".into());
            extra.push(model.into());
        }
        if let Some(effort) = profile.reasoning_effort {
            self.append_reasoning_effort_args(&mut extra, effort);
        }

        // GitHub Codex is `exec - --ephemeral`; the stdin marker is not last.
        let idx = if self == Self::Codex {
            args.iter()
                .position(|arg| arg == "-")
                .expect("codex base command always contains stdin marker")
        } else {
            args.len()
        };
        args.splice(idx..idx, extra);

        Ok((program, args))
    }

    /// Returns the CLI program and flags for this agent, including `--model`
    /// when an explicit model is requested.
    ///
    /// This is a model-only wrapper around [`Self::get_command_with_profile`].
    ///
    /// # Errors
    ///
    /// Returns an error when `model` is provided for an agent whose CLI does
    /// not expose a documented non-interactive model flag.
    pub fn get_command_with_model(
        self,
        model: Option<&str>,
    ) -> Result<(&'static str, Vec<String>), String> {
        self.get_command_with_profile(InvocationProfile {
            agent: self,
            model,
            reasoning_effort: None,
        })
    }

    /// Rejects agents that have no documented non-interactive `--model` flag.
    ///
    /// # Errors
    ///
    /// Returns an error when this agent cannot accept an explicit model.
    pub(crate) fn ensure_model_supported(self) -> Result<(), String> {
        if self == Self::Antigravity {
            return Err(
                "nightshift: agent antigravity does not support --model; retry without --model to use agy's persisted default model"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn validate_reasoning_effort(self, effort: &str) -> Result<(), String> {
        let Some(supported) = self.supported_reasoning_efforts() else {
            let hint = match self {
                Self::Cursor => "; choose a --model slug that encodes the desired effort",
                Self::Antigravity => "; retry without --reasoning-effort",
                _ => unreachable!("all effort-capable agents have an enum"),
            };
            return Err(format!(
                "nightshift: agent {} does not support --reasoning-effort{hint}",
                self.name()
            ));
        };
        if supported.contains(&effort) {
            return Ok(());
        }
        Err(format!(
            "nightshift: agent {} does not support --reasoning-effort {effort}; supported values: {}",
            self.name(),
            supported.join(", ")
        ))
    }

    fn append_reasoning_effort_args(self, args: &mut Vec<String>, effort: &str) {
        match self {
            Self::Pi => args.extend(["--thinking".into(), effort.into()]),
            Self::Claude => args.extend(["--effort".into(), effort.into()]),
            Self::Codex => args.extend(["-c".into(), format!("model_reasoning_effort={effort}")]),
            Self::Antigravity | Self::Cursor => {
                unreachable!("unsupported effort is rejected before argv construction")
            }
        }
    }

    /// Returns the clap value name for this agent.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::Cursor => "cursor",
            Self::Pi => "pi",
        }
    }
}

fn append_profile_hints(message: &mut String, profile: InvocationProfile<'_>) {
    if let Some(model) = profile.model {
        message.push(' ');
        message.push_str(&format!(
            "The agent may have rejected --model {model}; retry without --model or use a model accepted by that CLI."
        ));
    }
    if let Some(effort) = profile.reasoning_effort {
        message.push(' ');
        message.push_str(&format!(
            "The agent may have rejected --reasoning-effort {effort}; retry without --reasoning-effort or use a level accepted by that CLI."
        ));
    }
}

fn agent_exit_error(status: ExitStatus, profile: InvocationProfile<'_>) -> String {
    let mut message = format!("nightshift: agent command exited with status {status}.");
    append_profile_hints(&mut message, profile);
    message
}

fn stdin_write_error(write_err: std::io::Error, profile: InvocationProfile<'_>) -> String {
    let mut message =
        format!("nightshift: failed to write prompt to agent's stdin: {write_err}. Exiting.");
    append_profile_hints(&mut message, profile);
    message
}

/// Runs a rendered issue prompt with a selected [`Agent`].
///
/// Use this trait in orchestrator tests to observe whether an agent would have
/// been invoked without spawning a process.
///
/// # Examples
///
/// ```rust
/// # use nightshift::agent::{Agent, AgentRunner};
/// # use nightshift::invocation_profile::InvocationProfile;
/// # struct Recorder;
/// # impl AgentRunner for Recorder {
/// #     fn run(
/// #         &self,
/// #         _agent: Agent,
/// #         _profile: InvocationProfile<'_>,
/// #         _prompt: &str,
/// #     ) -> Result<(), Box<dyn std::error::Error>> {
/// #         Ok(())
/// #     }
/// # }
/// let runner = Recorder;
/// runner.run(
///     Agent::Cursor,
///     InvocationProfile {
///         agent: Agent::Cursor,
///         model: Some("gpt-5.2"),
///         reasoning_effort: None,
///     },
///     "Solve issue #7",
/// )?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait AgentRunner {
    /// Sends `prompt` to `agent` and returns when the agent process completes.
    fn run(
        &self,
        agent: Agent,
        profile: InvocationProfile<'_>,
        prompt: &str,
    ) -> Result<(), Box<dyn std::error::Error>>;
}

/// [`AgentRunner`] that spawns the configured agent command.
pub struct ProcessAgentRunner;

impl AgentRunner for ProcessAgentRunner {
    fn run(
        &self,
        agent: Agent,
        profile: InvocationProfile<'_>,
        prompt: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (cmd_name, cmd_args) = agent.get_command_with_profile(profile)?;

        let mut child = Command::new(cmd_name)
            .args(&cmd_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                format!(
                    "nightshift: failed to spawn agent command: '{}': {}. Exiting.",
                    cmd_name, e
                )
            })?;

        let write_result = if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes())
        } else {
            Ok(())
        };

        let status = child.wait().map_err(|e| {
            format!(
                "nightshift: failed to wait on agent process: {}. Exiting.",
                e
            )
        })?;

        if let Err(write_err) = write_result {
            if !status.success() {
                return Err(agent_exit_error(status, profile).into());
            }
            return Err(stdin_write_error(write_err, profile).into());
        }

        if !status.success() {
            return Err(agent_exit_error(status, profile).into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_effort_is_wired_for_supported_agents() {
        let profile = InvocationProfile {
            agent: Agent::Pi,
            model: Some("configured-model"),
            reasoning_effort: Some("high"),
        };

        assert_eq!(
            Agent::Pi
                .get_command_with_profile(profile)
                .expect("pi supports high effort"),
            (
                "pi",
                vec![
                    "-p".into(),
                    "--model".into(),
                    "configured-model".into(),
                    "--thinking".into(),
                    "high".into(),
                ],
            )
        );
        assert_eq!(
            Agent::Claude
                .get_command_with_profile(profile)
                .expect("claude supports high effort"),
            (
                "claude",
                vec![
                    "-p".into(),
                    "--dangerously-skip-permissions".into(),
                    "--model".into(),
                    "configured-model".into(),
                    "--effort".into(),
                    "high".into(),
                ],
            )
        );
        assert_eq!(
            Agent::Codex
                .get_command_with_profile(profile)
                .expect("codex supports high effort"),
            (
                "codex",
                vec![
                    "exec".into(),
                    "--model".into(),
                    "configured-model".into(),
                    "-c".into(),
                    "model_reasoning_effort=high".into(),
                    "-".into(),
                    "--ephemeral".into(),
                ],
            )
        );
    }

    #[test]
    fn claude_only_exposes_documented_reasoning_effort_values() {
        assert_eq!(
            Agent::Claude.supported_reasoning_efforts(),
            Some(&["low", "medium", "high", "max"][..])
        );
        assert!(
            Agent::Claude
                .get_command_with_profile(InvocationProfile {
                    agent: Agent::Claude,
                    model: None,
                    reasoning_effort: Some("xhigh"),
                })
                .is_err()
        );
    }

    #[test]
    fn unsupported_or_invalid_reasoning_effort_is_rejected() {
        let unsupported = Agent::Cursor
            .get_command_with_profile(InvocationProfile {
                agent: Agent::Cursor,
                model: None,
                reasoning_effort: Some("high"),
            })
            .expect_err("cursor uses model-encoded effort");
        assert_eq!(
            unsupported,
            "nightshift: agent cursor does not support --reasoning-effort; choose a --model slug that encodes the desired effort"
        );

        let antigravity = Agent::Antigravity
            .get_command_with_profile(InvocationProfile {
                agent: Agent::Antigravity,
                model: None,
                reasoning_effort: Some("high"),
            })
            .expect_err("antigravity has no reasoning-effort control");
        assert_eq!(
            antigravity,
            "nightshift: agent antigravity does not support --reasoning-effort; retry without --reasoning-effort"
        );

        let claude = Agent::Claude
            .get_command_with_profile(InvocationProfile {
                agent: Agent::Claude,
                model: None,
                reasoning_effort: Some("ultracode"),
            })
            .expect_err("local claude CLI does not expose ultracode");
        assert!(claude.contains("claude does not support --reasoning-effort ultracode"));

        let invalid = Agent::Codex
            .get_command_with_profile(InvocationProfile {
                agent: Agent::Codex,
                model: None,
                reasoning_effort: Some("max"),
            })
            .expect_err("codex does not support max effort");
        assert!(invalid.contains("codex does not support --reasoning-effort max"));
        assert!(invalid.contains("minimal, low, medium, high, xhigh"));
    }

    #[test]
    fn codex_splices_model_and_effort_immediately_before_stdin_marker() {
        let (program, args) = Agent::Codex
            .get_command_with_profile(InvocationProfile {
                agent: Agent::Codex,
                model: None,
                reasoning_effort: None,
            })
            .expect("codex default profile");
        assert_eq!(program, "codex");
        assert_eq!(args, vec!["exec", "-", "--ephemeral"]);

        let (program, args) = Agent::Codex
            .get_command_with_profile(InvocationProfile {
                agent: Agent::Codex,
                model: Some("gpt-5.4"),
                reasoning_effort: None,
            })
            .expect("codex model-only profile");
        assert_eq!(program, "codex");
        assert_eq!(args, vec!["exec", "--model", "gpt-5.4", "-", "--ephemeral"]);

        let (program, args) = Agent::Codex
            .get_command_with_profile(InvocationProfile {
                agent: Agent::Codex,
                model: None,
                reasoning_effort: Some("high"),
            })
            .expect("codex effort-only profile");
        assert_eq!(program, "codex");
        assert_eq!(
            args,
            vec![
                "exec",
                "-c",
                "model_reasoning_effort=high",
                "-",
                "--ephemeral"
            ]
        );

        let (program, args) = Agent::Codex
            .get_command_with_profile(InvocationProfile {
                agent: Agent::Codex,
                model: Some("gpt-5.4"),
                reasoning_effort: Some("high"),
            })
            .expect("codex model and effort profile");
        assert_eq!(program, "codex");
        assert_eq!(
            args,
            vec![
                "exec",
                "--model",
                "gpt-5.4",
                "-c",
                "model_reasoning_effort=high",
                "-",
                "--ephemeral"
            ]
        );
    }

    #[test]
    fn model_flag_is_added_for_agents_that_support_it() {
        let (program, args) = Agent::Codex
            .get_command_with_model(Some("gpt-5.4"))
            .expect("codex supports --model");
        assert_eq!(program, "codex");
        assert_eq!(args, vec!["exec", "--model", "gpt-5.4", "-", "--ephemeral"]);

        let (program, args) = Agent::Cursor
            .get_command_with_model(Some("gpt-5.2"))
            .expect("cursor supports --model");
        assert_eq!(program, "agent");
        assert_eq!(args, vec!["-p", "--force", "--trust", "--model", "gpt-5.2"]);

        let (program, args) = Agent::Pi
            .get_command_with_model(Some("openai/gpt-4o"))
            .expect("pi supports --model");
        assert_eq!(program, "pi");
        assert_eq!(args, vec!["-p", "--model", "openai/gpt-4o"]);
    }

    #[test]
    fn antigravity_rejects_explicit_model() {
        let err = Agent::Antigravity
            .get_command_with_model(Some("gemini-3.1-pro"))
            .expect_err("antigravity has no documented --model flag");
        assert!(err.contains("does not support --model"));
    }

    #[test]
    fn omitted_model_keeps_existing_agent_commands() {
        let (program, args) = Agent::Antigravity
            .get_command_with_model(None)
            .expect("antigravity can use its persisted model");
        assert_eq!(program, "agy");
        assert_eq!(args, vec!["-p", "--dangerously-skip-permissions"]);
    }
}
