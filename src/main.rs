use clap::{Parser, ValueEnum};
use regex::Regex;
use serde::Deserialize;
use serde_json::from_slice;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

impl Args {
    pub fn resolve_repo(&self) -> String {
        if let Some(repo) = &self.repo {
            if !repo.is_empty() {
                return repo.clone();
            }
        }
        let repo = Command::new("gh")
            .args([
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "-q",
                ".nameWithOwner",
            ])
            .output()
            .expect("Failed to execute gh command");
        let repo = String::from_utf8(repo.stdout).expect("Invalid UTF-8 output from gh");
        repo.trim().to_string()
    }

    pub fn ensure_branch_valid(&self) {
        let local = format!("refs/heads/{}", self.base_branch);
        let remote = format!("refs/remotes/origin/{}", self.base_branch);

        for reference in [&local, &remote] {
            if Command::new("git")
                .args(["show-ref", "--verify", "--quiet", reference])
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
            {
                return;
            }
        }
        eprintln!("nightshift: base branch {} not found", self.base_branch);
        std::process::exit(1);
    }
}

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
            Self::Copilot => (
                "copilot",
                vec!["--allow-all", "--no-ask-user", "-s"],
            ),
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

#[derive(Debug, Deserialize, Clone)]
pub struct GithubIssue {
    pub number: u32,
    pub title: String,
    pub body: String,
}

pub fn fetch_issues(repo: &str) -> Result<Vec<GithubIssue>, Box<dyn std::error::Error>> {
    let output = Command::new("gh")
        .args([
            "issue",
            "list",
            "-R",
            repo,
            "--json",
            "number,title,body",
            "--label",
            "ready-for-agent",
            "--state",
            "open",
        ])
        .output()?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to fetch issues: {}", err_msg).into());
    }

    let issues = from_slice(&output.stdout)?;
    Ok(issues)
}

pub fn extract_section_issue_numbers(body: &str, section_name: &str) -> Vec<u32> {
    let mut numbers = Vec::new();
    let mut capturing: bool = false;

    let re_num = Regex::new(r"#([0-9]+)").unwrap();
    let re_header = Regex::new(r"^#{1,6}\s").unwrap();

    let mut scan_line = |line: &str| {
        for cap in re_num.captures_iter(line) {
            if let Ok(num) = cap[1].parse::<u32>() {
                numbers.push(num);
            }
        }
    };

    for line in body.lines() {
        if line.to_lowercase().contains(&section_name.to_lowercase()) {
            capturing = true;
            scan_line(line);
            continue;
        }

        if capturing && re_header.is_match(line) {
            capturing = false;
        }

        if capturing {
            scan_line(line);
        }
    }
    numbers
}

pub fn extract_parent_prd(body: &str) -> Option<u32> {
    extract_section_issue_numbers(body, "parent")
        .into_iter()
        .next()
}

pub fn extract_blockers(body: &str) -> Vec<u32> {
    extract_section_issue_numbers(body, "blocked by")
}

#[derive(Deserialize)]
struct IssueState {
    state: String,
}

pub fn all_blockers_closed(
    repo: &str,
    blockers: &Vec<u32>,
) -> Result<bool, Box<dyn std::error::Error>> {
    for blocker in blockers {
        let output = Command::new("gh")
            .args([
                "issue",
                "view",
                &blocker.to_string(),
                "-R",
                repo,
                "--json",
                "state",
            ])
            .output()?;

        if !output.status.success() {
            let err_msg = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "nightshift: failed to check blocker #{}: {}",
                blocker, err_msg
            );
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "failed to check blocker",
            )));
        }

        let issue_state: IssueState = from_slice(&output.stdout)?;
        if issue_state.state.to_lowercase() != "closed" {
            return Ok(false);
        }
        continue;
    }
    Ok(true)
}

pub fn is_issue_closed(repo: &str, issue_number: u32) -> bool {
    let output = Command::new("gh")
        .args([
            "issue",
            "view",
            &issue_number.to_string(),
            "-R",
            repo,
            "--json",
            "state",
        ])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            if let Ok(issue_state) = from_slice::<IssueState>(&output.stdout) {
                issue_state.state.to_lowercase() == "closed"
            } else {
                false
            }
        }
        _ => false,
    }
}

pub fn ensure_git_hygiene(base_branch: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "nightshift: enforcing git hygiene (checking out and pulling {})...",
        base_branch
    );

    let checkout_status = Command::new("git")
        .args(["checkout", base_branch])
        .status()?;

    if !checkout_status.success() {
        return Err(format!("failed to checkout base branch '{}'", base_branch).into());
    }

    let pull_status = Command::new("git").args(["pull"]).status()?;

    if !pull_status.success() {
        return Err(format!("failed to pull latest changes from remote").into());
    }
    Ok(())
}

fn main() {
    let args = Args::parse();

    args.ensure_branch_valid();

    let repo = args.resolve_repo();

    let default_directives = r#"1. Orient yourself in the repository.
    2. Create a feature branch: git checkout -b issue-{issue_number}
    3. Implement using test-driven development.
    4. Run project lint/test checks and test behavior after implementation.
    5. Push branch and open a PR using 'gh pr create'.
    6. Self-review using sub-agents.
    7. Squash merge using 'gh pr merge' and delete branch.
    8. Close the issue using 'gh issue close'.
    9. Checkout the base branch and pull."#;

    let directives = match args.prompt_file {
        Some(prompt_file) => {
            if let Ok(prompt) = std::fs::read_to_string(&prompt_file) {
                prompt.trim().to_string()
            } else {
                eprintln!(
                    "nightshift: failed to read prompt file: {}",
                    prompt_file.display()
                );
                std::process::exit(1);
            }
        }
        None => default_directives.to_string(),
    };

    let initial_issues = match fetch_issues(&repo) {
        Ok(issues) => issues,
        Err(e) => {
            eprintln!(
                "nightshift: failed to fetch initial issues: {}. Exiting.",
                e
            );
            std::process::exit(1);
        }
    };
    if initial_issues.is_empty() {
        eprintln!("nightshift: no issues found");
        std::process::exit(1);
    }

    let prd: Option<String> = initial_issues
        .iter()
        .find(|i| i.number == args.prd)
        .map(|i| i.body.clone());

    let Some(prd_body) = prd else {
        eprintln!("nightshift: PRD issue {} not found", args.prd);
        std::process::exit(1);
    };

    println!("nightshift starting for PRD #{}...", args.prd);
    loop {
        if let Err(e) = ensure_git_hygiene(&args.base_branch) {
            eprintln!("nightshift: git hygiene check failed: {}. Exiting.", e);
            std::process::exit(1);
        }

        let issues = match fetch_issues(&repo) {
            Ok(issues) => issues,
            Err(e) => {
                println!("nightshift: failed to fetch issues: {}. Exiting.", e);
                std::process::exit(1);
            }
        };

        let mut candidates: Vec<GithubIssue> = Vec::new();
        let mut prd_has_open_issues = false;
        for issue in &issues {
            if let Some(parent) = extract_parent_prd(&issue.body) {
                if parent == args.prd {
                    prd_has_open_issues = true;
                    if issue.number >= args.issue {
                        candidates.push(issue.clone());
                    }
                }
            }
        }
        if candidates.is_empty() {
            if prd_has_open_issues {
                println!(
                    "nightshift: no candidates found starting from issue #{}, but
                    some open issues still exist below this threshold.
                    Loop complete.",
                    args.issue
                );
            } else {
                println!(
                    "nightshift: all issues for PRD #{} are resolved.
                    Loop complete.",
                    args.prd
                );
            }
            break;
        }

        candidates.sort_by_key(|issue| issue.number);

        let mut next_issue_to_solve: Option<GithubIssue> = None;
        for issue in candidates {
            let blockers = extract_blockers(&issue.body);
            match all_blockers_closed(&repo, &blockers) {
                Ok(true) => {
                    next_issue_to_solve = Some(issue);
                    break;
                }
                Ok(false) => {
                    continue;
                }
                Err(err) => {
                    eprintln!(
                        "nightshift: API or connection error while checking blockers: {}",
                        err
                    );
                    std::process::exit(1);
                }
            }
        }

        let Some(selected_issue) = next_issue_to_solve else {
            println!("nightshift: all remaining issues are blocked, loop complete.");
            break;
        };

        println!(
            "nightshift: solving issue #{} - {}",
            selected_issue.number, selected_issue.title
        );

        let final_prompt = format!(
            "You are working on issue #{num}: \"{title}\" in {repo_name} repository.

        ## PRD Context
        {prd_body}

        ## Task Description & Acceptance Criteria
        {issue_body}

        ## Instructions
        {directives}",
            num = selected_issue.number,
            title = selected_issue.title,
            repo_name = repo,
            prd_body = prd_body,
            issue_body = selected_issue.body,
            directives = directives
        );

        let (cmd_name, cmd_args) = args.agent.get_command();

        let mut temp_path = std::env::temp_dir();
        temp_path.push(format!("nightshift-prompt-{}.txt", selected_issue.number));

        if let Ok(mut file) = File::create(&temp_path) {
            let _ = file.write_all(final_prompt.as_bytes());
            println!("nightshift: saved prompt copy to {}", temp_path.display());
        }

        if args.dry_run {
            println!(
                "nightshift: [DRY-RUN] Selected issue: #{} - {}",
                selected_issue.number, selected_issue.title
            );
            println!("nightshift: [DRY-RUN] Would invoke agent: {}", cmd_name);
            println!("nightshift: [DRY-RUN] Prompt preview: \n{}", final_prompt);
            std::process::exit(0);
        }

        let mut child = match Command::new(cmd_name)
            .args(&cmd_args)
            .stdin(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                eprintln!(
                    "nightshift: failed to spawn agent command: '{}': {}. Exiting.",
                    cmd_name, e
                );
                std::process::exit(1);
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(final_prompt.as_bytes()) {
                eprintln!(
                    "nightshift: failed to write prompt to agent's stdin: {}. Exiting.",
                    e
                );
                std::process::exit(1);
            }
        }

        let status = match child.wait() {
            Ok(status) => status,
            Err(e) => {
                eprintln!(
                    "nightshift: failed to wait on agent process: {}. Exiting.",
                    e
                );
                std::process::exit(1);
            }
        };

        if !status.success() {
            eprintln!(
                "nightshift: command failed: {} {}",
                cmd_name,
                cmd_args.join(" ")
            );
            std::process::exit(1);
        }

        if !is_issue_closed(&repo, selected_issue.number) {
            eprintln!(
                "nightshift: agent exited successfully, but issue #{} is still open on GitHub.
                Exiting.",
                selected_issue.number
            );
            std::process::exit(1);
        }

        println!(
            "nightshift: issue #{} - {} completed",
            selected_issue.number, selected_issue.title
        );
    }
}
