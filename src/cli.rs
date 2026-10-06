//! Command-line interface for configuring a nightshift run.
//!
//! The parsed arguments are translated into [`crate::orchestrator::WorkflowConfig`]
//! by the binary entrypoint. They identify the parent issue, optional issue floor,
//! repository, Whole-Run Invocation Defaults, optional Preflight Dimensions,
//! directive source, base branch, dry-run mode, opt-in `--tui` Watch Board,
//! and optional `--recipe` / `--write-recipe` YAML paths.

use clap::Parser;
use std::path::PathBuf;

use crate::agent::Agent;
use crate::invocation_profile::{
    PreflightDimensions, RunEphemeralProfileMap, WholeRunInvocationDefaults,
};
use crate::orchestrator::WorkflowConfig;
use crate::prompt::DirectivePolicy;
use crate::recipe::default_write_path;

/// CLI arguments for one parent issue's child-issue loop.
#[derive(Parser)]
#[command(
    name = "nightshift",
    author = "Shaurya Sethi",
    version,
    about = "Autonomous Issue Completion Loop",
    help_template = "{about}\n\nAuthor: {author}\n\nUsage: {usage}\n\n{all-args}"
)]
pub struct Args {
    /// Parent issue number whose body provides shared context for child issues.
    #[arg(long, required_unless_present = "recipe")]
    pub parent: Option<u32>,
    /// Lowest child issue number to consider, useful when resuming partway through a parent issue.
    #[arg(long, default_value_t = 0)]
    pub issue: u32,
    /// GitHub repository slug in `owner/name` form, or omitted to use `gh repo view`.
    #[arg(long)]
    pub repo: Option<String>,
    /// Whole-run default coding agent; --pick-agents rows may override it.
    #[arg(long, required_unless_present = "recipe")]
    pub agent: Option<Agent>,
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
    /// Opt-in Watch Board, including recipe and dry runs. Requires stdin and stdout TTY and fails before recipe loading, GitHub, or git work. While work is active, q and Ctrl-C stop after the current issue without killing the agent. When idle, q, Ctrl-C, or Enter dismisses the board. Without this flag, cooked and non-TTY output stay unchanged.
    #[arg(long)]
    pub tui: bool,
    /// User-owned YAML run recipe. Exclusive with run flags except --dry-run and --tui. Replaces TTY pickers.
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = [
            "parent",
            "agent",
            "issue",
            "repo",
            "model",
            "reasoning_effort",
            "pick_agents",
            "pick_efforts",
            "pick_models",
            "pick_prompts",
            "prompt_file",
            "append_prompt_file",
            "base_branch",
            "write_recipe"
        ]
    )]
    pub recipe: Option<PathBuf>,
    /// Write a recipe YAML for the planned set and exit without starting a run. Default path is parent-<parent>-recipe.yaml in the current directory.
    #[arg(
        long,
        value_name = "PATH",
        num_args = 0..=1,
        conflicts_with_all = [
            "recipe",
            "pick_agents",
            "pick_efforts",
            "pick_models",
            "pick_prompts",
            "tui"
        ]
    )]
    pub write_recipe: Option<Option<PathBuf>>,
}

/// Rejects `--tui` unless both stdin and stdout are terminals.
///
/// Call this before any GitHub or git work, including repository resolution.
///
/// # Errors
///
/// Returns an actionable message when `--tui` is set and either handle is not a TTY.
pub fn ensure_tui_tty(tui: bool, stdin_tty: bool, stdout_tty: bool) -> Result<(), String> {
    if tui && !(stdin_tty && stdout_tty) {
        Err("nightshift: --tui requires an interactive TTY on stdin and stdout".to_string())
    } else {
        Ok(())
    }
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
            parent: self.parent.expect("clap requires --parent unless --recipe"),
            issue: self.issue,
            repo,
            base_branch: &self.base_branch,
            dry_run: self.dry_run,
            tui: self.tui,
            whole_run_defaults: WholeRunInvocationDefaults {
                agent: self.agent.expect("clap requires --agent unless --recipe"),
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
            recipe_lock: false,
        }
    }

    /// Destination for `--write-recipe`. `None` when the flag was not passed.
    ///
    /// Flag with no value becomes `parent-<parent>-recipe.yaml`.
    pub fn write_recipe_path(&self) -> Option<PathBuf> {
        match &self.write_recipe {
            None => None,
            Some(None) => Some(default_write_path(
                self.parent.expect("clap requires --parent unless --recipe"),
            )),
            Some(Some(path)) => Some(path.clone()),
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
    fn parent_is_the_only_parent_issue_flag() {
        let args = Args::try_parse_from(["nightshift", "--parent", "12", "--agent", "claude"])
            .expect("--parent should select the parent issue");
        assert!(args.agent.is_some());
        assert!(Args::try_parse_from(["nightshift", "--prd", "12", "--agent", "claude"]).is_err());
    }

    #[test]
    fn opencode_agent_value_is_unhyphenated() {
        let args = Args::try_parse_from(["nightshift", "--parent", "1", "--agent", "opencode"])
            .expect("opencode is the clap value name");
        assert_eq!(args.agent, Some(Agent::OpenCode));

        assert!(
            Args::try_parse_from(["nightshift", "--parent", "1", "--agent", "open-code"]).is_err(),
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
            "--parent",
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
            "--parent",
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
            "--parent",
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
            "--parent",
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
            "--parent",
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
            "--parent",
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
            "--parent",
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
            "--parent",
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
            "--parent",
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
    fn tui_flag_is_opt_in_and_help_explains_tty_and_stop() {
        let args = Args::try_parse_from(["nightshift", "--parent", "42", "--agent", "pi"])
            .expect("tui is optional");
        assert!(!args.tui);

        let args = Args::try_parse_from(["nightshift", "--parent", "42", "--agent", "pi", "--tui"])
            .expect("--tui should parse");
        assert!(args.tui);
        assert!(
            args.to_workflow_config("owner/repo", DirectivePolicy::BuiltIn)
                .tui
        );

        let mut command = Args::command();
        let help = command.render_long_help().to_string();
        assert!(help.contains("Requires stdin and stdout TTY"));
        assert!(help.contains("stop after the current issue"));
        assert!(help.contains("fails before recipe loading, GitHub, or git work"));
    }

    #[test]
    fn tui_without_tty_is_rejected_before_any_work() {
        super::ensure_tui_tty(false, false, false).expect("plain mode allows non-TTY");
        super::ensure_tui_tty(true, true, true).expect("tui allows a full TTY");
        for (stdin_tty, stdout_tty) in [(false, false), (true, false), (false, true)] {
            let error = super::ensure_tui_tty(true, stdin_tty, stdout_tty)
                .expect_err("--tui must require both TTYs");
            assert!(error.contains("--tui requires an interactive TTY"));
        }
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

    #[test]
    fn recipe_parses_without_parent_or_agent() {
        let args = Args::try_parse_from(["nightshift", "--recipe", "run.yaml"])
            .expect("--recipe should not require --parent or --agent");
        assert_eq!(
            args.recipe.as_deref(),
            Some(std::path::Path::new("run.yaml"))
        );
        assert!(args.parent.is_none());
        assert!(args.agent.is_none());
        assert!(args.write_recipe_path().is_none());
    }

    #[test]
    fn recipe_allows_dry_run() {
        let args = Args::try_parse_from(["nightshift", "--recipe", "run.yaml", "--dry-run"])
            .expect("--recipe --dry-run should parse");
        assert!(args.dry_run);
    }

    #[test]
    fn recipe_allows_tui_with_or_without_dry_run() {
        for extra in [Vec::new(), vec!["--dry-run"]] {
            let mut argv = vec!["nightshift", "--recipe", "run.yaml", "--tui"];
            argv.extend(extra.iter().copied());
            let args = Args::try_parse_from(argv).expect("recipe Watch Board should parse");
            assert!(args.tui);
            assert_eq!(args.dry_run, !extra.is_empty());
        }
    }

    #[test]
    fn recipe_conflicts_with_scope_profile_and_picker_flags() {
        assert!(
            Args::try_parse_from(["nightshift", "--recipe", "run.yaml", "--parent", "1"]).is_err()
        );
        assert!(
            Args::try_parse_from(["nightshift", "--recipe", "run.yaml", "--pick-agents"]).is_err()
        );
        assert!(
            Args::try_parse_from(["nightshift", "--recipe", "run.yaml", "--issue", "5"]).is_err()
        );
        assert!(
            Args::try_parse_from([
                "nightshift",
                "--recipe",
                "run.yaml",
                "--base-branch",
                "develop"
            ])
            .is_err()
        );
        for extra in [
            ["--agent", "pi"].as_slice(),
            ["--repo", "o/r"].as_slice(),
            ["--model", "m"].as_slice(),
            ["--reasoning-effort", "high"].as_slice(),
            ["--prompt-file", "p.md"].as_slice(),
            ["--append-prompt-file", "p.md"].as_slice(),
            ["--pick-efforts"].as_slice(),
            ["--pick-models"].as_slice(),
            ["--pick-prompts"].as_slice(),
        ] {
            let mut argv = vec!["nightshift", "--recipe", "run.yaml"];
            argv.extend(extra.iter().copied());
            assert!(
                Args::try_parse_from(&argv).is_err(),
                "expected conflict for {extra:?}"
            );
        }
    }

    #[test]
    fn write_recipe_requires_parent_and_agent() {
        assert!(Args::try_parse_from(["nightshift", "--write-recipe"]).is_err());
        let args = Args::try_parse_from([
            "nightshift",
            "--parent",
            "12",
            "--agent",
            "claude",
            "--write-recipe",
        ])
        .expect("--write-recipe with no path should parse");
        assert_eq!(
            args.write_recipe_path(),
            Some(std::path::PathBuf::from("parent-12-recipe.yaml"))
        );
    }

    #[test]
    fn write_recipe_keeps_explicit_path() {
        let args = Args::try_parse_from([
            "nightshift",
            "--parent",
            "12",
            "--agent",
            "claude",
            "--write-recipe",
            "out.yaml",
        ])
        .expect("explicit write path");
        assert_eq!(
            args.write_recipe_path(),
            Some(std::path::PathBuf::from("out.yaml"))
        );
    }

    #[test]
    fn write_recipe_conflicts_with_recipe_and_tui_and_pickers() {
        assert!(
            Args::try_parse_from([
                "nightshift",
                "--parent",
                "1",
                "--agent",
                "pi",
                "--write-recipe",
                "--recipe",
                "run.yaml"
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from([
                "nightshift",
                "--parent",
                "1",
                "--agent",
                "pi",
                "--write-recipe",
                "--tui"
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from([
                "nightshift",
                "--parent",
                "1",
                "--agent",
                "pi",
                "--write-recipe",
                "--pick-models"
            ])
            .is_err()
        );
    }

    #[test]
    fn write_recipe_allows_dry_run() {
        let args = Args::try_parse_from([
            "nightshift",
            "--parent",
            "12",
            "--agent",
            "claude",
            "--write-recipe",
            "--dry-run",
        ])
        .expect("--write-recipe --dry-run should parse");
        assert!(args.dry_run);
        assert_eq!(
            args.write_recipe_path(),
            Some(std::path::PathBuf::from("parent-12-recipe.yaml"))
        );
    }

    #[test]
    fn help_explains_recipe_flags() {
        let mut command = Args::command();
        let help = command.render_long_help().to_string();
        assert!(help.contains("User-owned YAML run recipe"));
        assert!(help.contains("Exclusive with run flags except --dry-run"));
        assert!(help.contains("parent-<parent>-recipe.yaml"));
    }
}
