//! Coordinates the PRD issue loop.
//!
//! Fetches ready GitHub issues, asks [`crate::parser`] which child to run, and
//! invokes the configured agent with a rendered prompt. Also owns dry-run
//! behavior, git hygiene, and the post-agent check that the selected issue was
//! closed.

use crate::agent::{Agent, AgentRunner};
use crate::console;
use crate::git::GitOps;
use crate::github::GithubIssues;
use crate::parser::plan_order;
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
    /// GitHub issue source and PRD-body fetcher.
    pub github: &'a dyn GithubIssues,
    /// Git workspace hygiene implementation.
    pub git: &'a dyn GitOps,
    /// Agent process runner.
    pub agent_runner: &'a dyn AgentRunner,
}

/// Runs the PRD child-issue loop until no eligible candidate remains.
///
/// Each iteration enforces git hygiene, fetches open `ready-for-agent` issues,
/// filters direct children of the requested PRD, then selects the
/// lowest-numbered unblocked child. Dry runs print the full simulated solve order,
/// the agent command preview, and the first planned issue's prompt, then exit.
/// Non-dry runs save a prompt copy, invoke the agent,
/// and require the selected GitHub issue to be closed before continuing.
///
/// # Errors
///
/// Returns an error when GitHub or git adapters fail, the PRD issue cannot be
/// found, the agent command fails, or the selected issue remains open after a
/// successful agent exit.
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
    let prd_body = runtime
        .github
        .fetch_issue_body(config.repo, config.prd)
        .map_err(|err| format!("nightshift: PRD issue {} not found: {}", config.prd, err))?;

    console::session_start(config.prd);

    if config.dry_run {
        return run_dry_run(config, runtime, &prd_body);
    }

    loop {
        console::git_hygiene(config.repo, config.base_branch);

        if let Err(e) = runtime.git.ensure_hygiene(config.base_branch) {
            return Err(format!("nightshift: git hygiene check failed: {}. Exiting.", e).into());
        }

        let issues_json = runtime
            .github
            .fetch_issues(config.repo)
            .map_err(|e| format!("nightshift: failed to fetch issues: {}. Exiting.", e))?;

        let plan = plan_order(&issues_json, config.prd, config.issue)?;
        let Some(selected_issue) = plan.planned.into_iter().next() else {
            if plan.blocked.is_empty() {
                complete_without_candidates(config.prd, config.issue, plan.has_open_children);
            } else {
                console::loop_complete("All remaining issues are blocked");
            }
            break;
        };

        let issue_run = console::IssueRun::begin(selected_issue.number, &selected_issue.title);

        let final_prompt =
            render_issue_prompt(config.repo, &prd_body, &selected_issue, config.directives);

        if let Some(path) = save_prompt_copy(selected_issue.number, &final_prompt) {
            issue_run.meta(&format!("prompt {}", path.display()));
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

        issue_run.complete();
    }

    Ok(())
}

/// Runs a dry-run pass: planned order, agent command preview, and first-issue prompt.
fn run_dry_run(
    config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    prd_body: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    console::git_hygiene(config.repo, config.base_branch);

    if let Err(e) = runtime.git.ensure_hygiene(config.base_branch) {
        return Err(format!("nightshift: git hygiene check failed: {}. Exiting.", e).into());
    }

    let issues_json = runtime
        .github
        .fetch_issues(config.repo)
        .map_err(|e| format!("nightshift: failed to fetch issues: {}. Exiting.", e))?;

    let plan = plan_order(&issues_json, config.prd, config.issue)?;

    if plan.planned.is_empty() && plan.blocked.is_empty() {
        complete_without_candidates(config.prd, config.issue, plan.has_open_children);
        return Ok(());
    }

    let (cmd_name, cmd_args) = config.agent.get_command_with_model(config.model)?;
    let agent_cmd = format!("{cmd_name} {}", cmd_args.join(" "));
    let planned: Vec<_> = plan
        .planned
        .iter()
        .map(|issue| (issue.number, issue.title.as_str()))
        .collect();
    let blocked: Vec<_> = plan
        .blocked
        .iter()
        .map(|issue| (issue.number, issue.title.as_str()))
        .collect();
    console::dry_run_planned_order(&planned, &blocked, &agent_cmd);

    if let Some(first) = plan.planned.first() {
        let final_prompt = render_issue_prompt(config.repo, prd_body, first, config.directives);
        console::dry_run_prompt(&final_prompt);
    }

    Ok(())
}

fn complete_without_candidates(prd: u32, min_issue: u32, has_open_children: bool) {
    if has_open_children {
        console::loop_complete(&format!(
            "No candidates from issue #{}; open issues remain below threshold",
            min_issue
        ));
    } else {
        console::loop_complete(&format!("All issues for PRD #{prd} resolved"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentRunner};
    use crate::git::GitOps;
    use crate::github::GithubIssues;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};

    fn child(number: u32, parent: u32, blockers: &[(u32, &str)]) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> = blockers
            .iter()
            .map(|(blocker, state)| json!({ "number": blocker, "state": state }))
            .collect();
        json!({
            "number": number,
            "title": format!("Child {number}"),
            "body": format!("Task {number}"),
            "parent": { "number": parent },
            "blockedBy": { "nodes": nodes, "totalCount": nodes.len() }
        })
    }

    fn graph(issues: &[serde_json::Value]) -> String {
        serde_json::Value::Array(issues.to_vec()).to_string()
    }

    struct MockGithub {
        issues_json: String,
        bodies: HashMap<u32, String>,
        closed: HashSet<u32>,
    }

    impl GithubIssues for MockGithub {
        fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
            Ok(repo.unwrap_or("foobar/repo").to_string())
        }

        fn fetch_issues(&self, _repo: &str) -> Result<String, Box<dyn std::error::Error>> {
            let issues: Vec<serde_json::Value> = serde_json::from_str(&self.issues_json)?;
            let open: Vec<serde_json::Value> = issues
                .into_iter()
                .filter(|issue| {
                    issue
                        .get("number")
                        .and_then(|number| number.as_u64())
                        .is_some_and(|number| !self.closed.contains(&(number as u32)))
                })
                .collect();
            Ok(serde_json::Value::Array(open).to_string())
        }

        fn fetch_issue_body(
            &self,
            _repo: &str,
            issue_number: u32,
        ) -> Result<String, Box<dyn std::error::Error>> {
            self.bodies
                .get(&issue_number)
                .cloned()
                .ok_or_else(|| format!("nightshift: failed to fetch issue #{issue_number}").into())
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

    fn prd_github() -> MockGithub {
        let issues = graph(&[child(10, 42, &[]), child(11, 42, &[])]);
        MockGithub {
            issues_json: issues,
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        }
    }

    #[test]
    fn dry_run_does_not_invoke_agent() {
        let github = prd_github();
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
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
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
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
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

    #[test]
    fn missing_prd_is_an_error() {
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::new(),
            closed: HashSet::new(),
        };
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            error_on_run: false,
        };
        let config = WorkflowConfig {
            prd: 42,
            issue: 0,
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
        let err = run(config, runtime).unwrap_err();
        assert!(err.to_string().contains("PRD issue 42 not found"));
    }

    #[test]
    fn empty_children_stops_cleanly() {
        let github = MockGithub {
            issues_json: "[]".into(),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        };
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            error_on_run: false,
        };
        let config = WorkflowConfig {
            prd: 42,
            issue: 0,
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
}
