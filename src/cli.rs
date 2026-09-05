//! Command-line interface for configuring a nightshift run.
//!
//! The parsed arguments are translated into [`crate::orchestrator::WorkflowConfig`]
//! by the binary entrypoint. They identify the PRD, optional issue floor,
//! repository, Whole-Run Invocation Defaults, optional Preflight Dimensions,
//! directive source, base branch, and dry-run mode.

use clap::Parser;
use std::path::PathBuf;

use crate::agent::Agent;
use crate::invocation_profile::{
    PreflightDimensions, RunEphemeralProfileMap, WholeRunInvocationDefaults,
};
use crate::orchestrator::WorkflowConfig;
use crate::prompt::DirectivePolicy;

/// CLI arguments for one PRD child-issue loop.
#[derive(Parser)]
#[command(
    name = "nightshift",
    author = "Shaurya Sethi",
    version,
    about = "Autonomous Issue Completion Loop",
    help_template = "{about}\n\nAuthor: {author}\n\nUsage: {usage}\n\n{all-args}"
)]
pub struct Args {
    /// PRD issue number whose body provides shared context for child issues.
    #[arg(long)]
    pub prd: u32,
    /// Lowest child issue number to consider, useful when resuming partway through a PRD.
    #[arg(long, default_value_t = 0)]
    pub issue: u32,
    /// GitHub repository slug in `owner/name` form, or omitted to use `gh repo view`.
    #[arg(long)]
    pub repo: Option<String>,
    /// Whole-run default coding agent; --pick-agents rows may override it.
    #[arg(long)]
    pub agent: Agent,
    /// Explicit model for the selected agent; omitted means use the agent's persisted default.
    #[arg(long)]
    pub model: Option<String>,
    /// Whole-run agent-native reasoning-effort default. Preflight rows can override this default; omission uses the agent default. Cursor uses model-encoded effort; choose a model slug instead of --reasoning-effort. OpenCode whole-run --variant values pass through; --pick-efforts uses the documented legend.
    #[arg(long)]
    pub reasoning_effort: Option<String>,
    /// TTY-only preflight that assigns effort per simulated-solvable issue while keeping the model fixed; may combine with --pick-agents and is mutually exclusive with --pick-models.
    #[arg(long, conflicts_with = "pick_models")]
    pub pick_efforts: bool,
    /// TTY-only preflight that assigns model and, where supported, effort per simulated-solvable issue. May combine with --pick-agents and is mutually exclusive with --pick-efforts. Cursor gets a model-only picker because its effort is model-encoded.
    #[arg(long, conflicts_with = "pick_efforts")]
    pub pick_models: bool,
    /// TTY-only preflight that assigns a compatible coding agent per simulated-solvable issue. Blank rows keep --agent. May combine with either --pick-efforts or --pick-models. Unsupported columns are skipped for that row. Whole-run model and effort defaults apply only when the row agent equals --agent.
    #[arg(long)]
    pub pick_agents: bool,
    /// File that overrides built-in directives for every issue unless a --pick-prompts row supplies a file.
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,
    /// File appended to the resolved agent's built-in directives for every issue unless a --pick-prompts row supplies a file. Mutually exclusive with --prompt-file.
    #[arg(long, conflicts_with = "prompt_file")]
    pub append_prompt_file: Option<PathBuf>,
    /// TTY-only preflight that assigns an optional prompt file and append/replace mode per planned issue. Blank path keeps the run-wide prompt policy. Enter on mode defaults to append. May combine with --pick-agents, --pick-efforts, and --pick-models.
    #[arg(long)]
    pub pick_prompts: bool,
    /// Base branch checked out and pulled before each agent run.
    #[arg(long, default_value = "main")]
    pub base_branch: String,
    /// Simulate planned order and preview the first prompt and command without invoking an agent; requested preflight still runs.
    #[arg(long)]
    pub dry_run: bool,
}

impl Args {
    /// Converts parsed CLI inputs into the orchestrator config.
    ///
    /// `repo` is resolved in `main` after parse, so it is supplied here rather
    /// than read from [`Self::repo`].
    pub fn to_workflow_config<'a>(
        &'a self,
        repo: &'a str,
        directive_policy: DirectivePolicy<'a>,
    ) -> WorkflowConfig<'a> {
        WorkflowConfig {
            prd: self.prd,
            issue: self.issue,
            repo,
            base_branch: &self.base_branch,
            dry_run: self.dry_run,
            whole_run_defaults: WholeRunInvocationDefaults {
                agent: self.agent,
                model: self.model.as_deref(),
                reasoning_effort: self.reasoning_effort.as_deref(),
            },
            per_issue_profiles: RunEphemeralProfileMap::new(),
            preflight_dimensions: PreflightDimensions {
                agents: self.pick_agents,
                efforts: self.pick_efforts,
                models: self.pick_models,
                prompts: self.pick_prompts,
            },
            directive_policy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Args;
    use crate::agent::Agent;
    use crate::invocation_profile::PreflightDimensions;
    use crate::prompt::DirectivePolicy;
    use clap::{CommandFactory, Parser};

    #[test]
    fn opencode_agent_value_is_unhyphenated() {
        let args = Args::try_parse_from(["nightshift", "--prd", "1", "--agent", "opencode"])
            .expect("opencode is the clap value name");
        assert_eq!(args.agent, Agent::OpenCode);

        assert!(
            Args::try_parse_from(["nightshift", "--prd", "1", "--agent", "open-code"]).is_err(),
            "OpenCode must not kebab-case to open-code"
        );
    }

    #[test]
    fn help_explains_cursor_model_encoded_effort() {
        let mut command = Args::command();
        let help = command.render_long_help().to_string();

        assert!(help.contains(
            "Cursor uses model-encoded effort; choose a model slug instead of --reasoning-effort"
        ));
    }

    #[test]
    fn help_explains_effort_is_a_whole_run_default() {
        let mut command = Args::command();
        let help = command.render_long_help().to_string();

        assert!(help.contains("Preflight rows can override this default"));
        assert!(help.contains(
            "OpenCode whole-run --variant values pass through; --pick-efforts uses the documented legend"
        ));
    }

    #[test]
    fn help_explains_stacked_row_capabilities_and_default_inheritance() {
        let mut command = Args::command();
        let help = command.render_long_help().to_string();

        assert!(help.contains("May combine with either --pick-efforts or --pick-models"));
        assert!(help.contains("Unsupported columns are skipped for that row"));
        assert!(help.contains(
            "Whole-run model and effort defaults apply only when the row agent equals --agent"
        ));
    }

    #[test]
    fn pick_agents_enables_agent_preflight_dimension() {
        let args = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--pick-agents",
        ])
        .expect("agent picker flag should parse");

        assert_eq!(
            args.to_workflow_config("owner/repo", DirectivePolicy::BuiltIn)
                .preflight_dimensions,
            PreflightDimensions {
                agents: true,
                ..PreflightDimensions::default()
            }
        );
    }

    #[test]
    fn pick_efforts_enables_effort_preflight_dimension() {
        let args = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--pick-efforts",
        ])
        .expect("effort picker flag should parse");

        assert_eq!(
            args.to_workflow_config("owner/repo", DirectivePolicy::BuiltIn)
                .preflight_dimensions,
            PreflightDimensions {
                efforts: true,
                ..PreflightDimensions::default()
            }
        );
    }

    #[test]
    fn pick_models_enables_model_preflight_dimension() {
        let args = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--pick-models",
        ])
        .expect("full profile picker flag should parse");

        assert_eq!(
            args.to_workflow_config("owner/repo", DirectivePolicy::BuiltIn)
                .preflight_dimensions,
            PreflightDimensions {
                models: true,
                ..PreflightDimensions::default()
            }
        );
    }

    #[test]
    fn pick_agents_combines_with_efforts() {
        let args = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--pick-agents",
            "--pick-efforts",
        ])
        .expect("agent and effort dimensions should stack");

        assert_eq!(
            args.to_workflow_config("owner/repo", DirectivePolicy::BuiltIn)
                .preflight_dimensions,
            PreflightDimensions {
                agents: true,
                efforts: true,
                ..PreflightDimensions::default()
            }
        );
    }

    #[test]
    fn pick_agents_combines_with_models() {
        let args = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--pick-agents",
            "--pick-models",
        ])
        .expect("agent and model dimensions should stack");

        assert_eq!(
            args.to_workflow_config("owner/repo", DirectivePolicy::BuiltIn)
                .preflight_dimensions,
            PreflightDimensions {
                agents: true,
                models: true,
                ..PreflightDimensions::default()
            }
        );
    }

    #[test]
    fn pick_efforts_conflicts_with_pick_models() {
        let parsed = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--pick-efforts",
            "--pick-models",
        ]);

        assert!(parsed.is_err());
    }

    #[test]
    fn append_prompt_file_conflicts_with_prompt_file() {
        let parsed = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--prompt-file",
            "replace.md",
            "--append-prompt-file",
            "append.md",
        ]);

        assert!(parsed.is_err());
    }

    #[test]
    fn pick_prompts_enables_prompt_preflight_dimension() {
        let args = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--pick-prompts",
        ])
        .expect("prompt picker flag should parse");

        assert_eq!(
            args.to_workflow_config("owner/repo", DirectivePolicy::BuiltIn)
                .preflight_dimensions,
            PreflightDimensions {
                prompts: true,
                ..PreflightDimensions::default()
            }
        );
    }

    #[test]
    fn pick_prompts_combines_with_pick_agents() {
        let args = Args::try_parse_from([
            "nightshift",
            "--prd",
            "42",
            "--agent",
            "pi",
            "--pick-prompts",
            "--pick-agents",
        ])
        .expect("prompt and agent dimensions should stack");

        assert_eq!(
            args.to_workflow_config("owner/repo", DirectivePolicy::BuiltIn)
                .preflight_dimensions,
            PreflightDimensions {
                agents: true,
                prompts: true,
                ..PreflightDimensions::default()
            }
        );
    }

    #[test]
    fn help_explains_prompt_file_replace_append_and_pick_prompts() {
        let mut command = Args::command();
        let help = command.render_long_help().to_string();

        assert!(help.contains(
            "overrides built-in directives for every issue unless a --pick-prompts row supplies a file"
        ));
        assert!(help.contains("File appended to the resolved agent's built-in directives"));
        assert!(help.contains("Mutually exclusive with --prompt-file"));
        assert!(help.contains("Blank path keeps the run-wide prompt policy"));
        assert!(help.contains("Enter on mode defaults to append"));
        assert!(
            !help.contains("maintainer directives to append to each prompt"),
            "--prompt-file must not be described as append"
        );
    }
}
