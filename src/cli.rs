//! Command-line interface for configuring a nightshift run.
//!
//! The parsed arguments are translated into [`crate::orchestrator::WorkflowConfig`]
//! by the binary entrypoint. They identify the PRD, optional issue floor,
//! repository, agent, directive source, base branch, and dry-run mode.

use clap::Parser;
use std::path::PathBuf;

use crate::agent::Agent;

/// CLI arguments for one PRD child-issue loop.
#[derive(Parser)]
#[command(
    name = "nightshift",
    author = "Shaurya Sethi",
    version,
    about = "Autonomous Issue Completion Loop"
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
    /// Coding agent CLI to invoke for each selected issue.
    #[arg(long)]
    pub agent: Agent,
    /// Optional file containing maintainer directives to append to each prompt.
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,
    /// Base branch checked out and pulled before each agent run.
    #[arg(long, default_value = "main")]
    pub base_branch: String,
    /// Print the selected issue and rendered prompt without invoking an agent.
    #[arg(long)]
    pub dry_run: bool,
}
