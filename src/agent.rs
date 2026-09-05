//! Coding-agent selection and execution.
//!
//! Nightshift renders a full issue prompt and sends it to a configured agent
//! process through stdin. [`crate::agent::Agent`] owns the command names and
//! argument lists, while [`crate::agent::AgentRunner`] lets the orchestrator run
//! a real process in production and a fake runner in tests.

use clap::ValueEnum;
use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};

/// Coding-agent CLI variants supported by nightshift.
#[derive(ValueEnum, Debug, Clone, Copy)]
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

    /// Returns the CLI program and flags for this agent, including `--model`
    /// when an explicit model is requested.
    ///
    /// # Errors
    ///
    /// Returns an error when `model` is provided for an agent whose CLI does
    /// not expose a documented non-interactive model flag.
    pub fn get_command_with_model(
        self,
        model: Option<&str>,
    ) -> Result<(&'static str, Vec<String>), String> {
        match (self, model) {
            (Self::Antigravity, Some(_)) => Err(
                "nightshift: agent antigravity does not support --model; retry without --model to use agy's persisted default model"
                    .to_string(),
            ),
            (Self::Codex, Some(model)) => Ok((
                "codex",
                vec![
                    "exec".into(),
                    "--model".into(),
                    model.into(),
                    "-".into(),
                    "--ephemeral".into(),
                ],
            )),
            (Self::Claude, Some(model)) => Ok((
                "claude",
                vec![
                    "-p".into(),
                    "--dangerously-skip-permissions".into(),
                    "--model".into(),
                    model.into(),
                ],
            )),
            (Self::Cursor, Some(model)) => Ok((
                "agent",
                vec![
                    "-p".into(),
                    "--force".into(),
                    "--trust".into(),
                    "--model".into(),
                    model.into(),
                ],
            )),
            (Self::Pi, Some(model)) => Ok(("pi", vec!["-p".into(), "--model".into(), model.into()])),
            (_, None) => {
                let (program, args) = self.get_command();
                Ok((program, args.into_iter().map(str::to_string).collect()))
            }
        }
    }
}

fn append_model_hint(message: &mut String, model: Option<&str>) {
    if let Some(model) = model {
        message.push(' ');
        message.push_str(&format!(
            "The agent may have rejected --model {model}; retry without --model or use a model accepted by that CLI."
        ));
    }
}

fn agent_exit_error(status: ExitStatus, model: Option<&str>) -> String {
    let mut message = format!("nightshift: agent command exited with status {status}.");
    append_model_hint(&mut message, model);
    message
}

fn stdin_write_error(write_err: std::io::Error, model: Option<&str>) -> String {
    let mut message =
        format!("nightshift: failed to write prompt to agent's stdin: {write_err}. Exiting.");
    append_model_hint(&mut message, model);
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
/// # struct Recorder;
/// # impl AgentRunner for Recorder {
/// #     fn run(
/// #         &self,
/// #         _agent: Agent,
/// #         _model: Option<&str>,
/// #         _prompt: &str,
/// #     ) -> Result<(), Box<dyn std::error::Error>> {
/// #         Ok(())
/// #     }
/// # }
/// let runner = Recorder;
/// runner.run(Agent::Cursor, Some("gpt-5.2"), "Solve issue #7")?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait AgentRunner {
    /// Sends `prompt` to `agent` and returns when the agent process completes.
    fn run(
        &self,
        agent: Agent,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<(), Box<dyn std::error::Error>>;
}

/// [`AgentRunner`] that spawns the configured agent command.
pub struct ProcessAgentRunner;

impl AgentRunner for ProcessAgentRunner {
    fn run(
        &self,
        agent: Agent,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (cmd_name, cmd_args) = agent.get_command_with_model(model)?;

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
                return Err(agent_exit_error(status, model).into());
            }
            return Err(stdin_write_error(write_err, model).into());
        }

        if !status.success() {
            return Err(agent_exit_error(status, model).into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
