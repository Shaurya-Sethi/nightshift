//! Binary entrypoint for the nightshift CLI.
//!
//! This wires command-line arguments to the GitHub, git, prompt, and agent
//! adapters, then hands control to [`nightshift::orchestrator::run`].

use clap::Parser;
use nightshift::agent::ProcessAgentRunner;
use nightshift::cli::{Args, ensure_tui_tty};
use nightshift::git::{GitCliAdapter, GitOps};
use nightshift::github::{GhCliAdapter, GithubIssues};
use nightshift::orchestrator::{Runtime, run};
use nightshift::prompt::{DirectivePolicy, load_directives};
use std::io::IsTerminal;

fn main() {
    let args = Args::parse();
    if let Err(e) = ensure_tui_tty(
        args.tui,
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
        eprintln!("{e}");
        std::process::exit(1);
    }
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
        Ok(git) => {
            if args.tui {
                git.capture_stdio()
            } else {
                git
            }
        }
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

    let loaded = args
        .prompt_file
        .as_deref()
        .or(args.append_prompt_file.as_deref())
        .map(|path| {
            load_directives(path).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            })
        });
    let directive_policy = match (
        args.prompt_file.is_some(),
        args.append_prompt_file.is_some(),
        loaded.as_deref(),
    ) {
        (true, false, Some(text)) => DirectivePolicy::Replace(text),
        (false, true, Some(text)) => DirectivePolicy::Append(text),
        (false, false, None) => DirectivePolicy::BuiltIn,
        _ => unreachable!("clap rejects combining --prompt-file with --append-prompt-file"),
    };
    let config = args.to_workflow_config(&repo, directive_policy);

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
