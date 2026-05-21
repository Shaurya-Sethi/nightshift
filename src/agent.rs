//! Coding-agent selection and execution.
//!
//! Nightshift renders a full issue prompt and sends it to a configured agent
//! process through stdin. [`crate::agent::Agent`] owns the command names and
//! argument lists, while [`crate::agent::AgentRunner`] lets the orchestrator run
//! a real process in production and a fake runner in tests.

use clap::ValueEnum;
use std::io::Write;
use std::process::{Command, Stdio};

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
    /// Pi CLI, invoked as `pi`.
    Pi,
    /// GitHub Copilot CLI, invoked as `copilot`.
    Copilot,
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
            // copilot: -p/--prompt takes argv text; piped stdin is used without -p
            Self::Copilot => ("copilot", vec!["--allow-all", "--no-ask-user", "-s"]),
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
/// #     fn run(&self, _agent: Agent, _prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
/// #         Ok(())
/// #     }
/// # }
/// let runner = Recorder;
/// runner.run(Agent::Cursor, "Solve issue #7")?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait AgentRunner {
    /// Sends `prompt` to `agent` and returns when the agent process completes.
    fn run(&self, agent: Agent, prompt: &str) -> Result<(), Box<dyn std::error::Error>>;
}

/// [`AgentRunner`] that spawns the configured agent command.
pub struct ProcessAgentRunner;

impl AgentRunner for ProcessAgentRunner {
    fn run(&self, agent: Agent, prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
        let (cmd_name, cmd_args) = agent.get_command();

        let mut child = Command::new(cmd_name)
            .args(&cmd_args)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!(
                    "nightshift: failed to spawn agent command: '{}': {}. Exiting.",
                    cmd_name, e
                )
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).map_err(|e| {
                format!(
                    "nightshift: failed to write prompt to agent's stdin: {}. Exiting.",
                    e
                )
            })?;
        }

        let status = child.wait().map_err(|e| {
            format!(
                "nightshift: failed to wait on agent process: {}. Exiting.",
                e
            )
        })?;

        if !status.success() {
            return Err(format!(
                "nightshift: command failed: {} {}",
                cmd_name,
                cmd_args.join(" ")
            )
            .into());
        }

        Ok(())
    }
}
