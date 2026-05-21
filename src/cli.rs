use clap::Parser;
use std::path::PathBuf;

use crate::agent::Agent;

#[derive(Parser)]
#[command(
    name = "nightshift",
    author = "Shaurya Sethi",
    version,
    about = "Autonomous Issue Completion Loop"
)]
pub struct Args {
    #[arg(long)]
    pub prd: u32,
    #[arg(long, default_value_t = 0)]
    pub issue: u32,
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long)]
    pub agent: Agent,
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,
    #[arg(long, default_value = "main")]
    pub base_branch: String,
    #[arg(long)]
    pub dry_run: bool,
}
