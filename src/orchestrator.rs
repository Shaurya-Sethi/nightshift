//! Coordinates the PRD issue loop.
//!
//! The orchestrator fetches ready GitHub issues, uses [`crate::parser`] to keep
//! only child issues for the requested PRD, skips children whose blockers are
//! still open, and invokes the configured agent with a rendered prompt. It owns
//! workflow policy such as candidate ordering, dry-run behavior, git hygiene,
//! and the post-agent check that the selected issue was closed.

use crate::agent::{Agent, AgentRunner};
use crate::git::GitOps;
use crate::github::{GithubIssue, GithubIssues};
use crate::parser::{extract_blockers, extract_parent_prd};
use crate::prompt::{render_issue_prompt, save_prompt_copy};

/// Configuration for one nightshift PRD loop.
pub struct WorkflowConfig<'a> {
    /// PRD issue number whose body becomes shared context for every child issue.
    pub prd: u32,
    /// Lowest child issue number to consider when selecting candidates.
    pub issue: u32,
    /// GitHub repository slug in `owner/name` form.
    pub repo: &'a str,
    /// Base branch to check out and pull before each agent run.
    pub base_branch: &'a str,
    /// When true, render and print the selected prompt without invoking an agent.
    pub dry_run: bool,
    /// Coding agent variant to invoke for selected issues.
    pub agent: Agent,
    /// Explicit model to request from the selected agent, when supported.
    pub model: Option<&'a str>,
    /// Maintainer instructions appended to each generated issue prompt.
    pub directives: &'a str,
}

/// Runtime adapters used by [`run`].
///
/// Tests provide fake implementations here so the loop can be exercised without
/// shelling out to `gh`, `git`, or a coding-agent CLI.
pub struct Runtime<'a> {
    /// GitHub issue source and blocker-state checker.
    pub github: &'a dyn GithubIssues,
    /// Git workspace hygiene implementation.
    pub git: &'a dyn GitOps,
    /// Agent process runner.
    pub agent_runner: &'a dyn AgentRunner,
}

/// Runs the PRD child-issue loop until no eligible candidate remains.
///
/// Each iteration enforces git hygiene, fetches open `ready-for-agent` issues,
/// filters children that declare the requested PRD parent, then selects the
/// lowest-numbered unblocked child. Dry runs stop after printing the selected
/// issue and prompt preview. Non-dry runs save a prompt copy, invoke the agent,
/// and require the selected GitHub issue to be closed before continuing.
///
/// # Errors
///
/// Returns an error when GitHub or git adapters fail, the PRD issue cannot be
/// found, no issues are available, the agent command fails, or the selected
/// issue remains open after a successful agent exit.
///
/// # Examples
///
/// ```rust,no_run
/// # use nightshift::agent::{Agent, ProcessAgentRunner};
/// # use nightshift::git::GitCliAdapter;
/// # use nightshift::github::GhCliAdapter;
/// # use nightshift::orchestrator::{Runtime, WorkflowConfig, run};
/// # let github = GhCliAdapter;
/// # let git = GitCliAdapter::for_repo("owner/repo")?;
/// # let agent_runner = ProcessAgentRunner;
/// let config = WorkflowConfig {
///     prd: 42,
///     issue: 0,
///     repo: "owner/repo",
///     base_branch: "main",
///     dry_run: true,
///     agent: Agent::Cursor,
///     model: Some("gpt-5.2"),
///     directives: "Run tests before opening a PR.",
/// };
/// let runtime = Runtime {
///     github: &github,
///     git: &git,
///     agent_runner: &agent_runner,
/// };
///
/// run(config, runtime)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn run(
    config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let initial_issues = runtime.github.fetch_issues(config.repo).map_err(|e| {
        format!(
            "nightshift: failed to fetch initial issues: {}. Exiting.",
            e
        )
    })?;

    if initial_issues.is_empty() {
        return Err("nightshift: no issues found".into());
    }

    let prd: Option<String> = initial_issues
        .iter()
        .find(|i| i.number == config.prd)
        .map(|i| i.body.clone());

    let Some(prd_body) = prd else {
        return Err(format!("nightshift: PRD issue {} not found", config.prd).into());
    };

    println!("nightshift starting for PRD #{}...", config.prd);

    loop {
        if let Err(e) = runtime.git.ensure_hygiene(config.base_branch) {
            return Err(format!("nightshift: git hygiene check failed: {}. Exiting.", e).into());
        }

        let issues = runtime
            .github
            .fetch_issues(config.repo)
            .map_err(|e| format!("nightshift: failed to fetch issues: {}. Exiting.", e))?;

        let (candidates, prd_has_open_issues) =
            collect_prd_candidates(&issues, config.prd, config.issue);

        if candidates.is_empty() {
            if prd_has_open_issues {
                println!(
                    "nightshift: no candidates found starting from issue #{}, but
                    some open issues still exist below this threshold.
                    Loop complete.",
                    config.issue
                );
            } else {
                println!(
                    "nightshift: all issues for PRD #{} are resolved.
                    Loop complete.",
                    config.prd
                );
            }
            break;
        }

        let next_issue_to_solve =
            pick_next_unblocked_issue(&candidates, runtime.github, config.repo).map_err(|err| {
                format!(
                    "nightshift: API or connection error while checking blockers: {}",
                    err
                )
            })?;

        let Some(selected_issue) = next_issue_to_solve else {
            println!("nightshift: all remaining issues are blocked, loop complete.");
            break;
        };

        println!(
            "nightshift: solving issue #{} - {}",
            selected_issue.number, selected_issue.title
        );

        let final_prompt =
            render_issue_prompt(config.repo, &prd_body, &selected_issue, config.directives);

        save_prompt_copy(selected_issue.number, &final_prompt);

        if config.dry_run {
            let (cmd_name, cmd_args) = config.agent.get_command_with_model(config.model)?;
            println!(
                "nightshift: [DRY-RUN] Selected issue: #{} - {}",
                selected_issue.number, selected_issue.title
            );
            println!(
                "nightshift: [DRY-RUN] Would invoke agent: {} {}",
                cmd_name,
                cmd_args.join(" ")
            );
            println!("nightshift: [DRY-RUN] Prompt preview: \n{}", final_prompt);
            return Ok(());
        }

        runtime
            .agent_runner
            .run(config.agent, config.model, &final_prompt)?;

        if !runtime
            .github
            .is_issue_closed(config.repo, selected_issue.number)?
        {
            return Err(format!(
                "nightshift: agent exited successfully, but issue #{} is still open on GitHub.
                Exiting.",
                selected_issue.number
            )
            .into());
        }

        println!(
            "nightshift: issue #{} - {} completed",
            selected_issue.number, selected_issue.title
        );
    }

    Ok(())
}

/// Collects open child issues for `prd` and reports whether any child exists
/// below the configured issue floor.
pub(crate) fn collect_prd_candidates(
    issues: &[GithubIssue],
    prd: u32,
    min_issue: u32,
) -> (Vec<GithubIssue>, bool) {
    let mut candidates = Vec::new();
    let mut prd_has_open_issues = false;
    for issue in issues {
        if let Some(parent) = extract_parent_prd(&issue.body)
            && parent == prd
        {
            prd_has_open_issues = true;
            if issue.number >= min_issue {
                candidates.push(issue.clone());
            }
        }
    }
    (candidates, prd_has_open_issues)
}

/// Picks the lowest-numbered candidate whose declared blockers are all closed.
pub(crate) fn pick_next_unblocked_issue(
    candidates: &[GithubIssue],
    github: &dyn GithubIssues,
    repo: &str,
) -> Result<Option<GithubIssue>, Box<dyn std::error::Error>> {
    let mut sorted: Vec<_> = candidates.to_vec();
    sorted.sort_by_key(|issue| issue.number);
    for issue in sorted {
        let blockers = extract_blockers(&issue.body);
        if github.all_blockers_closed(repo, &blockers)? {
            return Ok(Some(issue));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentRunner};
    use crate::git::GitOps;
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;

    fn child(number: u32, parent: u32, blockers: &[u32]) -> GithubIssue {
        let blockers_line = if blockers.is_empty() {
            String::new()
        } else {
            let refs: Vec<String> = blockers.iter().map(|n| format!("#{n}")).collect();
            format!("\n## Blocked by\n{}", refs.join(", "))
        };
        GithubIssue {
            number,
            title: format!("Child {number}"),
            body: format!("## Parent\n#{parent}{blockers_line}"),
        }
    }

    struct MockGithub {
        issues: Vec<GithubIssue>,
        closed: HashSet<u32>,
    }

    impl GithubIssues for MockGithub {
        fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
            Ok(repo.unwrap_or("foobar/repo").to_string())
        }

        fn fetch_issues(
            &self,
            _repo: &str,
        ) -> Result<Vec<GithubIssue>, Box<dyn std::error::Error>> {
            Ok(self
                .issues
                .iter()
                .filter(|issue| !self.closed.contains(&issue.number))
                .cloned()
                .collect())
        }

        fn all_blockers_closed(
            &self,
            _repo: &str,
            blockers: &[u32],
        ) -> Result<bool, Box<dyn std::error::Error>> {
            Ok(blockers.iter().all(|blocker| self.closed.contains(blocker)))
        }

        fn is_issue_closed(
            &self,
            _repo: &str,
            issue_number: u32,
        ) -> Result<bool, Box<dyn std::error::Error>> {
            Ok(self.closed.contains(&issue_number))
        }
    }

    struct MockGit;

    impl GitOps for MockGit {
        fn base_branch_exists(&self, _base_branch: &str) -> bool {
            true
        }

        fn ensure_hygiene(&self, _base_branch: &str) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    struct MockAgent {
        ran: Cell<bool>,
        received_model: RefCell<Option<String>>,
        error_on_run: bool,
    }

    impl AgentRunner for MockAgent {
        fn run(
            &self,
            _agent: Agent,
            model: Option<&str>,
            _prompt: &str,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.ran.set(true);
            if let Some(model) = model {
                self.received_model.replace(Some(model.to_string()));
            }
            if self.error_on_run {
                return Err("mock agent stopped after recording run".into());
            }
            Ok(())
        }
    }

    #[test]
    fn collect_prd_candidates_only_matching_parent_and_issue_floor() {
        let issues = vec![
            child(5, 42, &[]),
            child(10, 42, &[]),
            child(11, 99, &[]),
            child(12, 42, &[]),
        ];
        let (candidates, has_open) = collect_prd_candidates(&issues, 42, 10);
        assert!(has_open);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].number, 10);
        assert_eq!(candidates[1].number, 12);
    }

    #[test]
    fn pick_next_unblocked_issue_prefers_lowest_number() {
        let issues = vec![child(20, 42, &[]), child(15, 42, &[])];
        let github = MockGithub {
            issues: vec![],
            closed: HashSet::new(),
        };
        let picked = pick_next_unblocked_issue(&issues, &github, "foobar/repo")
            .unwrap()
            .unwrap();
        assert_eq!(picked.number, 15);
    }

    #[test]
    fn pick_next_unblocked_issue_skips_open_blockers() {
        let blocked = child(10, 42, &[7]);
        let ready = child(11, 42, &[]);
        let github = MockGithub {
            issues: vec![],
            closed: HashSet::new(),
        };
        assert!(
            pick_next_unblocked_issue(std::slice::from_ref(&blocked), &github, "foobar/repo")
                .unwrap()
                .is_none()
        );
        let picked =
            pick_next_unblocked_issue(&[blocked.clone(), ready.clone()], &github, "foobar/repo")
                .unwrap()
                .unwrap();
        assert_eq!(picked.number, 11);

        let github = MockGithub {
            issues: vec![],
            closed: HashSet::from([7]),
        };
        let picked = pick_next_unblocked_issue(&[blocked, ready], &github, "foobar/repo")
            .unwrap()
            .unwrap();
        assert_eq!(picked.number, 10);
    }

    #[test]
    fn dry_run_does_not_invoke_agent() {
        let prd = GithubIssue {
            number: 42,
            title: "PRD".into(),
            body: "Product requirements".into(),
        };
        let issues = vec![prd, child(10, 42, &[]), child(11, 42, &[])];
        let github = MockGithub {
            issues,
            closed: HashSet::new(),
        };
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            error_on_run: false,
        };
        let config = WorkflowConfig {
            prd: 42,
            issue: 1,
            repo: "foobar/repo",
            base_branch: "main",
            dry_run: true,
            agent: Agent::Cursor,
            model: None,
            directives: "test directives",
        };
        let runtime = Runtime {
            github: &github,
            git: &MockGit,
            agent_runner: &agent,
        };
        run(config, runtime).unwrap();
        assert!(!agent.ran.get());
    }

    #[test]
    fn dry_run_accepts_explicit_model_without_invoking_agent() {
        let prd = GithubIssue {
            number: 42,
            title: "PRD".into(),
            body: "Product requirements".into(),
        };
        let issues = vec![prd, child(10, 42, &[])];
        let github = MockGithub {
            issues,
            closed: HashSet::new(),
        };
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            error_on_run: false,
        };
        let config = WorkflowConfig {
            prd: 42,
            issue: 1,
            repo: "foobar/repo",
            base_branch: "main",
            dry_run: true,
            agent: Agent::Cursor,
            model: Some("gpt-5.2"),
            directives: "test directives",
        };
        let runtime = Runtime {
            github: &github,
            git: &MockGit,
            agent_runner: &agent,
        };
        run(config, runtime).unwrap();
        assert!(!agent.ran.get());
        assert_eq!(agent.received_model.borrow().as_deref(), None);

        let (cmd_name, cmd_args) = Agent::Cursor
            .get_command_with_model(Some("gpt-5.2"))
            .expect("dry-run should accept explicit model for cursor");
        assert_eq!(cmd_name, "agent");
        assert!(cmd_args.iter().any(|arg| arg == "--model"));
        assert!(cmd_args.iter().any(|arg| arg == "gpt-5.2"));
    }

    #[test]
    fn non_dry_run_passes_explicit_model_to_agent_runner() {
        let prd = GithubIssue {
            number: 42,
            title: "PRD".into(),
            body: "Product requirements".into(),
        };
        let issues = vec![prd, child(10, 42, &[])];
        let github = MockGithub {
            issues,
            closed: HashSet::new(),
        };
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            error_on_run: true,
        };
        let config = WorkflowConfig {
            prd: 42,
            issue: 1,
            repo: "foobar/repo",
            base_branch: "main",
            dry_run: false,
            agent: Agent::Cursor,
            model: Some("gpt-5.2"),
            directives: "test directives",
        };
        let runtime = Runtime {
            github: &github,
            git: &MockGit,
            agent_runner: &agent,
        };
        let err = run(config, runtime).unwrap_err();
        assert!(err.to_string().contains("mock agent stopped"));
        assert!(agent.ran.get());
        assert_eq!(agent.received_model.borrow().as_deref(), Some("gpt-5.2"));
    }
}
