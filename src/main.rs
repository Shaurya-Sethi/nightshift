//! Binary entrypoint for the nightshift CLI.
//!
//! This wires command-line arguments to the GitHub, git, prompt, and agent
//! adapters, then hands control to [`nightshift::orchestrator::run`].

use clap::Parser;
use nightshift::agent::ProcessAgentRunner;
use nightshift::cli::Args;
use nightshift::git::{GitCliAdapter, GitOps};
use nightshift::github::{GhCliAdapter, GithubIssues};
use nightshift::orchestrator::{Runtime, run};
use nightshift::prompt::{BUILT_IN_DIRECTIVES, load_directives};

fn main() {
    let args = Args::parse();
    let github = GhCliAdapter;
    let agent_runner = ProcessAgentRunner;

    let repo = match github.resolve_repo(args.repo.as_deref()) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let git = match GitCliAdapter::for_repo(&repo) {
        Ok(git) => git,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    if !git.base_branch_exists(&args.base_branch) {
        eprintln!(
            "nightshift: base branch {} not found in {}",
            args.base_branch,
            git.workdir().display()
        );
        std::process::exit(1);
    }

    let directives = match args.prompt_file.as_deref() {
        Some(prompt_file) => match load_directives(Some(prompt_file), args.agent) {
            Ok(directives) => directives,
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        },
        None => BUILT_IN_DIRECTIVES.to_string(),
    };

    let config = args.to_workflow_config(&repo, &directives);

    let runtime = Runtime {
        github: &github,
        git: &git,
        agent_runner: &agent_runner,
    };

    if let Err(e) = run(config, runtime) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
