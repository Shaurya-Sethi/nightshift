use clap::Parser;
use nightshift::agent::ProcessAgentRunner;
use nightshift::cli::Args;
use nightshift::git::{GitCliAdapter, GitOps};
use nightshift::github::{GhCliAdapter, GithubIssues};
use nightshift::orchestrator::{Runtime, WorkflowConfig, run};
use nightshift::prompt::load_directives;

fn main() {
    let args = Args::parse();
    let git = GitCliAdapter;
    let github = GhCliAdapter;
    let agent_runner = ProcessAgentRunner;

    if !git.base_branch_exists(&args.base_branch) {
        eprintln!("nightshift: base branch {} not found", args.base_branch);
        std::process::exit(1);
    }

    let repo = match github.resolve_repo(args.repo.as_deref()) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let directives = match load_directives(args.prompt_file.as_deref()) {
        Ok(directives) => directives,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let config = WorkflowConfig {
        prd: args.prd,
        issue: args.issue,
        repo: &repo,
        base_branch: &args.base_branch,
        dry_run: args.dry_run,
        agent: args.agent,
        directives: &directives,
    };

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
