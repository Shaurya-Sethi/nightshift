//! Binary entrypoint for the nightshift CLI.
//!
//! Parses arguments, rejects `--tui` unless stdin and stdout are TTYs (before
//! any GitHub or git work), wires adapters (capturing git stdio on the TUI
//! path), then hands control to [`nightshift::orchestrator::run`]. `--recipe`
//! loads a YAML run spec before GitHub work. `--write-recipe` stamps a planned
//! set and exits without starting a run.

use clap::Parser;
use nightshift::agent::ProcessAgentRunner;
use nightshift::cli::{Args, ensure_tui_tty};
use nightshift::git::{GitCliAdapter, GitOps};
use nightshift::github::{GhCliAdapter, GithubIssues};
use nightshift::invocation_profile::PreflightDimensions;
use nightshift::orchestrator::{Runtime, WorkflowConfig, run};
use nightshift::parser::plan_order;
use nightshift::prompt::{DirectivePolicy, PromptMode, load_directives};
use nightshift::recipe::{GenerateSpec, PreparedRecipe, Recipe};
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

fn die(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn main() -> ExitCode {
    let args = Args::parse();
    if let Err(e) = ensure_tui_tty(
        args.tui,
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
        die(e);
    }

    if let Some(path) = args.recipe.clone() {
        return run_from_recipe(&path, args.dry_run);
    }

    let github = GhCliAdapter;
    let agent_runner = ProcessAgentRunner;

    let repo = match github.resolve_repo(args.repo.as_deref()) {
        Ok(repo) => repo,
        Err(e) => die(e),
    };

    if let Some(dest) = args.write_recipe_path() {
        write_recipe(&args, &github, &repo, &dest);
        return ExitCode::SUCCESS;
    }

    let git = match GitCliAdapter::for_repo(&repo) {
        Ok(git) => {
            if args.tui {
                git.capture_stdio()
            } else {
                git
            }
        }
        Err(e) => die(e),
    };

    if !git.base_branch_exists(&args.base_branch) {
        die(format!(
            "nightshift: base branch {} not found in {}",
            args.base_branch,
            git.workdir().display()
        ));
    }

    let loaded = args
        .prompt_file
        .as_deref()
        .or(args.append_prompt_file.as_deref())
        .map(|path| load_directives(path).unwrap_or_else(|e| die(e)));
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
        die(e);
    }
    ExitCode::SUCCESS
}

fn run_from_recipe(path: &Path, dry_run: bool) -> ExitCode {
    let prepared = PreparedRecipe::load(path).unwrap_or_else(|e| die(e));
    let github = GhCliAdapter;
    let agent_runner = ProcessAgentRunner;
    let repo = match github.resolve_repo(prepared.repo()) {
        Ok(repo) => repo,
        Err(e) => die(e),
    };
    let git = match GitCliAdapter::for_repo(&repo) {
        Ok(git) => git,
        Err(e) => die(e),
    };
    if !git.base_branch_exists(prepared.base_branch()) {
        die(format!(
            "nightshift: base branch {} not found in {}",
            prepared.base_branch(),
            git.workdir().display()
        ));
    }
    let config = WorkflowConfig {
        prd: prepared.prd(),
        issue: prepared.issue(),
        repo: &repo,
        base_branch: prepared.base_branch(),
        dry_run,
        tui: false,
        whole_run_defaults: prepared.whole_run_defaults(),
        per_issue_profiles: prepared.per_issue_profiles().clone(),
        preflight_dimensions: PreflightDimensions::default(),
        directive_policy: prepared.directive_policy(),
        recipe_lock: true,
    };
    let runtime = Runtime {
        github: &github,
        git: &git,
        agent_runner: &agent_runner,
    };
    if let Err(e) = run(config, runtime) {
        die(e);
    }
    ExitCode::SUCCESS
}

fn write_recipe(args: &Args, github: &GhCliAdapter, repo: &str, dest: &Path) {
    let prd = args.prd.expect("clap requires --prd unless --recipe");
    let agent = args.agent.expect("clap requires --agent unless --recipe");
    let issues_json = github
        .fetch_issues(repo)
        .unwrap_or_else(|e| die(format!("nightshift: failed to fetch issues: {e}. Exiting.")));
    let plan = plan_order(&issues_json, prd, args.issue).unwrap_or_else(|e| die(e));
    let (prompt_file, prompt_mode) = match (
        args.prompt_file.as_deref(),
        args.append_prompt_file.as_deref(),
    ) {
        (Some(path), None) => (Some(path), Some(PromptMode::Replace)),
        (None, Some(path)) => (Some(path), Some(PromptMode::Append)),
        (None, None) => (None, None),
        _ => unreachable!("clap rejects combining --prompt-file with --append-prompt-file"),
    };
    let recipe = Recipe::from_planned(GenerateSpec {
        prd,
        repo,
        agent,
        issue: args.issue,
        base_branch: &args.base_branch,
        model: args.model.as_deref(),
        reasoning_effort: args.reasoning_effort.as_deref(),
        prompt_file,
        prompt_mode,
        planned: &plan.planned,
    })
    .unwrap_or_else(|e| die(e));
    recipe.clone().prepare().unwrap_or_else(|e| die(e));
    recipe.write_to(dest).unwrap_or_else(|e| die(e));
    println!("{}", dest.display());
}
