use clap::ValueEnum;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum Agent {
    Claude,
    Codex,
    Antigravity,
    Cursor,
    Pi,
    Copilot,
}

impl Agent {
    /// Returns the CLI program and flags for the agent.
    ///
    /// The compiled issue prompt is written to the child process stdin after spawn
    /// (keeps large prompts off argv). Only Codex uses `-` as a documented stdin marker;
    /// other agents must not receive `-` as a literal prompt argument.
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

pub trait AgentRunner {
    fn run(&self, agent: Agent, prompt: &str) -> Result<(), Box<dyn std::error::Error>>;
}

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
