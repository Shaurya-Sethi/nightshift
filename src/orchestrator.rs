//! Coordinates the PRD issue loop.
//!
//! Fetches ready GitHub issues, asks [`crate::parser`] which child to run, and
//! invokes the configured agent with a rendered prompt. Also owns dry-run
//! behavior, git hygiene, Invocation Profile Preflight, and the post-agent
//! check that the selected issue was closed.

use crate::agent::AgentRunner;
use crate::console;
use crate::git::GitOps;
use crate::github::{GithubIssue, GithubIssues};
use crate::invocation_profile::{
    PreflightDimensions, RunEphemeralProfileMap, WholeRunInvocationDefaults, resolve,
};
use crate::parser::plan_order;
use crate::prompt::{directives_for_invocation, render_issue_prompt, save_prompt_copy};
use std::io::IsTerminal;

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
    /// Whole-Run Invocation Defaults resolved for every issue without an override.
    pub whole_run_defaults: WholeRunInvocationDefaults<'a>,
    /// Run-Ephemeral Profile Map whose per-issue fields override whole-run defaults.
    pub per_issue_profiles: RunEphemeralProfileMap,
    /// Invocation Profile Preflight columns collected after plan simulation.
    pub preflight_dimensions: PreflightDimensions,
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

fn run_with_preflight_io(
    mut config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    preflight_io: Option<&mut crate::preflight::Io<'_>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let default_agent = config.whole_run_defaults.agent;
    let whole_run_profile = resolve(config.whole_run_defaults, None);
    default_agent.get_command_with_profile(whole_run_profile)?;
    let dimensions = config.preflight_dimensions;
    if dimensions.efforts && dimensions.models {
        return Err("nightshift: --pick-efforts and --pick-models are mutually exclusive".into());
    }
    let preflight_enabled = dimensions.agents || dimensions.efforts || dimensions.models;
    if !dimensions.agents && dimensions.efforts {
        default_agent.supported_reasoning_efforts().ok_or_else(|| {
            format!(
                "nightshift: agent {} does not support --pick-efforts; use --reasoning-effort only with an agent that supports separate effort control",
                default_agent.name()
            )
        })?;
    }
    if !dimensions.agents && dimensions.models {
        default_agent
            .ensure_model_supported()
            .map_err(|error| format!("{error}; --pick-models is unavailable for this agent"))?;
    }
    if preflight_enabled {
        let Some(io) = preflight_io.as_ref() else {
            return Err("nightshift: Invocation Profile Preflight requires terminal I/O".into());
        };
        io.ensure_terminal()?;
    }

    let prd_body = runtime
        .github
        .fetch_issue_body(config.repo, config.prd)
        .map_err(|err| format!("nightshift: PRD issue {} not found: {}", config.prd, err))?;

    console::session_start(config.prd);

    if preflight_enabled {
        let issues_json = runtime
            .github
            .fetch_issues(config.repo)
            .map_err(|e| format!("nightshift: failed to fetch issues: {}. Exiting.", e))?;
        let plan = plan_order(&issues_json, config.prd, config.issue)?;
        let Some(io) = preflight_io else {
            return Err("nightshift: Invocation Profile Preflight requires terminal I/O".into());
        };
        config.per_issue_profiles = crate::preflight::pick_profiles(
            &plan.planned,
            config.whole_run_defaults,
            dimensions,
            config.dry_run,
            io,
        )?;
    }

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

        let profile = resolve(
            config.whole_run_defaults,
            config.per_issue_profiles.get(&selected_issue.number),
        );
        let issue_run = console::IssueRun::begin(selected_issue.number, &selected_issue.title);
        issue_run.invocation_profile(profile);

        let directives = directives_for_invocation(config.directives, profile.agent);
        let final_prompt =
            render_issue_prompt(config.repo, &prd_body, &selected_issue, directives.as_ref());

        if let Some(path) = save_prompt_copy(selected_issue.number, &final_prompt) {
            issue_run.meta(&format!("prompt {}", path.display()));
        }

        runtime
            .agent_runner
            .run(profile.agent, profile, &final_prompt)?;

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

/// Runs the PRD child-issue loop until no eligible candidate remains.
///
/// Each iteration enforces git hygiene, fetches open `ready-for-agent` issues,
/// filters direct children of the requested PRD, then selects the
/// lowest-numbered unblocked child. Dry runs print the full simulated solve order,
/// the agent command preview, and the first planned issue's prompt, then exit.
/// Non-dry runs save a prompt copy, invoke the agent,
/// and require the selected GitHub issue to be closed before continuing.
///
/// When pick flags are set, Invocation Profile Preflight runs after the session
/// header and before dry-run or the live loop. Git hygiene stays inside those
/// later phases.
///
/// # Errors
///
/// Returns an error when GitHub or git adapters fail, the PRD issue cannot be
/// found, the whole-run profile or pick flags are illegal, preflight aborts,
/// the agent command fails, or the selected issue remains open after a
/// successful agent exit.
///
/// # Examples
///
/// ```rust,no_run
/// # use nightshift::agent::{Agent, ProcessAgentRunner};
/// # use nightshift::git::GitCliAdapter;
/// # use nightshift::github::GhCliAdapter;
/// # use nightshift::invocation_profile::{
/// #     PreflightDimensions, RunEphemeralProfileMap, WholeRunInvocationDefaults,
/// # };
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
///     whole_run_defaults: WholeRunInvocationDefaults {
///         agent: Agent::Cursor,
///         model: Some("gpt-5.2"),
///         reasoning_effort: None,
///     },
///     per_issue_profiles: RunEphemeralProfileMap::new(),
///     preflight_dimensions: PreflightDimensions::default(),
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
    if !config.preflight_dimensions.agents
        && !config.preflight_dimensions.efforts
        && !config.preflight_dimensions.models
    {
        return run_with_preflight_io(config, runtime, None);
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let terminal = stdin.is_terminal() && stdout.is_terminal();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    let mut io = crate::preflight::Io::new(terminal, &mut input, &mut output);
    run_with_preflight_io(config, runtime, Some(&mut io))
}

type DryRunAssignment<'a> = (
    u32,
    String,
    crate::invocation_profile::InvocationProfile<'a>,
);
type DryRunPreview<'a> = (Vec<DryRunAssignment<'a>>, String);

/// Builds resolved dry-run assignments and first-issue command preview.
///
/// Every planned child issue receives its own resolved profile. The command
/// uses the first assignment because the dry run previews the loop's next
/// agent invocation.
fn build_dry_run_preview<'a>(
    planned: &[GithubIssue],
    defaults: WholeRunInvocationDefaults<'a>,
    profiles: &'a RunEphemeralProfileMap,
) -> Result<DryRunPreview<'a>, String> {
    let assignments: Vec<_> = planned
        .iter()
        .map(|issue| {
            (
                issue.number,
                issue.title.clone(),
                resolve(defaults, profiles.get(&issue.number)),
            )
        })
        .collect();
    let first_profile = assignments
        .first()
        .map(|(_, _, profile)| *profile)
        .unwrap_or_else(|| resolve(defaults, None));
    let (cmd_name, cmd_args) = first_profile
        .agent
        .get_command_with_profile(first_profile)?;

    Ok((assignments, format!("{cmd_name} {}", cmd_args.join(" "))))
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

    let (planned, agent_cmd) = build_dry_run_preview(
        &plan.planned,
        config.whole_run_defaults,
        &config.per_issue_profiles,
    )?;
    let blocked: Vec<_> = plan
        .blocked
        .iter()
        .map(|issue| (issue.number, issue.title.as_str()))
        .collect();
    console::dry_run_planned_order(&planned, &blocked, &agent_cmd);

    if let Some(first) = plan.planned.first() {
        let profile = resolve(
            config.whole_run_defaults,
            config.per_issue_profiles.get(&first.number),
        );
        let directives = directives_for_invocation(config.directives, profile.agent);
        let final_prompt = render_issue_prompt(config.repo, prd_body, first, directives.as_ref());
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
    use crate::invocation_profile::{
        InvocationProfile, PerIssueInvocationOverride, RunEphemeralProfileMap,
    };
    use crate::prompt::BUILT_IN_DIRECTIVES;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::io::Cursor;

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

    fn github_child(number: u32) -> GithubIssue {
        GithubIssue {
            number,
            title: format!("Child {number}"),
            body: format!("Task {number}"),
        }
    }

    fn defaults<'a>(
        agent: Agent,
        model: Option<&'a str>,
        reasoning_effort: Option<&'a str>,
    ) -> WholeRunInvocationDefaults<'a> {
        WholeRunInvocationDefaults {
            agent,
            model,
            reasoning_effort,
        }
    }

    fn workflow<'a>(
        issue: u32,
        dry_run: bool,
        whole_run_defaults: WholeRunInvocationDefaults<'a>,
        per_issue_profiles: RunEphemeralProfileMap,
        preflight_dimensions: PreflightDimensions,
        directives: &'a str,
    ) -> WorkflowConfig<'a> {
        WorkflowConfig {
            prd: 42,
            issue,
            repo: "foobar/repo",
            base_branch: "main",
            dry_run,
            whole_run_defaults,
            per_issue_profiles,
            preflight_dimensions,
            directives,
        }
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

    struct CountingGithub {
        fetch_body_calls: Cell<u32>,
        fetch_issues_calls: Cell<u32>,
    }

    impl GithubIssues for CountingGithub {
        fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
            Ok(repo.unwrap_or("foobar/repo").to_string())
        }

        fn fetch_issues(&self, _repo: &str) -> Result<String, Box<dyn std::error::Error>> {
            self.fetch_issues_calls
                .set(self.fetch_issues_calls.get() + 1);
            Ok("[]".into())
        }

        fn fetch_issue_body(
            &self,
            _repo: &str,
            _issue_number: u32,
        ) -> Result<String, Box<dyn std::error::Error>> {
            self.fetch_body_calls.set(self.fetch_body_calls.get() + 1);
            Ok("Product requirements".into())
        }

        fn is_issue_closed(
            &self,
            _repo: &str,
            _issue_number: u32,
        ) -> Result<bool, Box<dyn std::error::Error>> {
            Ok(false)
        }
    }

    struct SharedGithub<'a> {
        issues_json: String,
        bodies: HashMap<u32, String>,
        closed: &'a RefCell<HashSet<u32>>,
    }

    impl GithubIssues for SharedGithub<'_> {
        fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
            Ok(repo.unwrap_or("foobar/repo").to_string())
        }

        fn fetch_issues(&self, _repo: &str) -> Result<String, Box<dyn std::error::Error>> {
            let issues: Vec<serde_json::Value> = serde_json::from_str(&self.issues_json)?;
            let closed = self.closed.borrow();
            let open: Vec<serde_json::Value> = issues
                .into_iter()
                .filter(|issue| {
                    issue
                        .get("number")
                        .and_then(|number| number.as_u64())
                        .is_some_and(|number| !closed.contains(&(number as u32)))
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
            Ok(self.closed.borrow().contains(&issue_number))
        }
    }

    struct MockGit;

    const MOCK_GIT: MockGit = MockGit;

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
            profile: InvocationProfile<'_>,
            _prompt: &str,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.ran.set(true);
            if let Some(model) = profile.model {
                self.received_model.replace(Some(model.to_string()));
            }
            if self.error_on_run {
                return Err("mock agent stopped after recording run".into());
            }
            Ok(())
        }
    }

    struct ProfileRecordingAgent {
        received_profile: RefCell<Option<(Option<String>, Option<String>)>>,
    }

    impl AgentRunner for ProfileRecordingAgent {
        fn run(
            &self,
            _agent: Agent,
            profile: InvocationProfile<'_>,
            _prompt: &str,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.received_profile.replace(Some((
                profile.model.map(str::to_string),
                profile.reasoning_effort.map(str::to_string),
            )));
            Err("mock agent stopped after recording profile".into())
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

    fn idle_agent() -> MockAgent {
        MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            error_on_run: false,
        }
    }

    fn runtime<'a>(github: &'a dyn GithubIssues, agent: &'a dyn AgentRunner) -> Runtime<'a> {
        Runtime {
            github,
            git: &MOCK_GIT,
            agent_runner: agent,
        }
    }

    #[test]
    fn dry_run_does_not_invoke_agent() {
        let github = prd_github();
        let agent = idle_agent();
        let config = workflow(
            1,
            true,
            defaults(Agent::Cursor, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            "test directives",
        );
        run(config, runtime(&github, &agent)).unwrap();
        assert!(!agent.ran.get());
    }

    #[test]
    fn dry_run_accepts_explicit_model_without_invoking_agent() {
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        };
        let agent = idle_agent();
        let config = workflow(
            1,
            true,
            defaults(Agent::Cursor, Some("gpt-5.2"), None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            "test directives",
        );
        run(config, runtime(&github, &agent)).unwrap();
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
    fn dry_run_preview_uses_each_issue_profile_and_first_issue_command() {
        let planned = [github_child(10), github_child(11)];
        let profiles = RunEphemeralProfileMap::from([
            (
                10,
                PerIssueInvocationOverride {
                    agent: None,
                    model: Some("issue-one-model".to_string()),
                    reasoning_effort: Some("high".to_string()),
                },
            ),
            (
                11,
                PerIssueInvocationOverride {
                    agent: None,
                    model: Some("issue-two-model".to_string()),
                    reasoning_effort: Some("low".to_string()),
                },
            ),
        ]);

        let (assignments, command) = build_dry_run_preview(
            &planned,
            defaults(Agent::Pi, Some("run-model"), Some("medium")),
            &profiles,
        )
        .expect("preview should build commands for supported profiles");

        assert_eq!(assignments.len(), 2);
        assert_eq!(assignments[0].0, 10);
        assert_eq!(assignments[0].2.model, Some("issue-one-model"));
        assert_eq!(assignments[0].2.reasoning_effort, Some("high"));
        assert_eq!(assignments[1].0, 11);
        assert_eq!(assignments[1].2.model, Some("issue-two-model"));
        assert_eq!(assignments[1].2.reasoning_effort, Some("low"));
        assert_eq!(command, "pi -p --model issue-one-model --thinking high");
    }

    #[test]
    fn dry_run_preview_uses_first_resolved_agent_command() {
        let planned = [github_child(10)];
        let profiles = RunEphemeralProfileMap::from([(
            10,
            PerIssueInvocationOverride {
                agent: Some(Agent::Codex),
                model: None,
                reasoning_effort: None,
            },
        )]);

        let (assignments, command) =
            build_dry_run_preview(&planned, defaults(Agent::Pi, None, None), &profiles)
                .expect("preview should use selected agent");

        assert_eq!(assignments[0].2.agent, Agent::Codex);
        assert_eq!(command, "codex exec - --ephemeral");
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
        let config = workflow(
            1,
            false,
            defaults(Agent::Cursor, Some("gpt-5.2"), None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            "test directives",
        );
        let err = run(config, runtime(&github, &agent)).unwrap_err();
        assert!(err.to_string().contains("mock agent stopped"));
        assert!(agent.ran.get());
        assert_eq!(agent.received_model.borrow().as_deref(), Some("gpt-5.2"));
    }

    #[test]
    fn non_dry_run_resolves_per_issue_profile_before_agent_invocation() {
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        };
        let agent = ProfileRecordingAgent {
            received_profile: RefCell::new(None),
        };
        let profiles = RunEphemeralProfileMap::from([(
            10,
            PerIssueInvocationOverride {
                agent: None,
                model: Some("issue-model".to_string()),
                reasoning_effort: None,
            },
        )]);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, Some("run-model"), Some("medium")),
            profiles,
            PreflightDimensions::default(),
            "test directives",
        );

        let error = run(config, runtime(&github, &agent))
            .expect_err("fake agent stops after recording resolved profile")
            .to_string();
        assert!(error.contains("mock agent stopped after recording profile"));
        assert_eq!(
            *agent.received_profile.borrow(),
            Some((Some("issue-model".to_string()), Some("medium".to_string())))
        );
    }

    #[test]
    fn non_dry_run_uses_defaults_for_unmapped_issue_profile() {
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        };
        let agent = ProfileRecordingAgent {
            received_profile: RefCell::new(None),
        };
        let profiles = RunEphemeralProfileMap::from([(
            11,
            PerIssueInvocationOverride {
                agent: None,
                model: Some("other-issue-model".to_string()),
                reasoning_effort: Some("low".to_string()),
            },
        )]);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, Some("run-model"), Some("medium")),
            profiles,
            PreflightDimensions::default(),
            "test directives",
        );

        let error = run(config, runtime(&github, &agent))
            .expect_err("fake agent stops after recording defaults")
            .to_string();
        assert!(error.contains("mock agent stopped after recording profile"));
        assert_eq!(
            *agent.received_profile.borrow(),
            Some((Some("run-model".to_string()), Some("medium".to_string())))
        );
    }

    #[test]
    fn live_loop_resolves_a_second_issue_to_a_different_model() {
        struct RecordingCloser<'a> {
            models: RefCell<Vec<Option<String>>>,
            closed: &'a RefCell<HashSet<u32>>,
            next_close: Cell<u32>,
        }

        impl AgentRunner for RecordingCloser<'_> {
            fn run(
                &self,
                _agent: Agent,
                profile: InvocationProfile<'_>,
                _prompt: &str,
            ) -> Result<(), Box<dyn std::error::Error>> {
                self.models
                    .borrow_mut()
                    .push(profile.model.map(str::to_string));
                let number = self.next_close.get();
                self.closed.borrow_mut().insert(number);
                self.next_close.set(number + 1);
                Ok(())
            }
        }

        let closed = RefCell::new(HashSet::new());
        let github = SharedGithub {
            issues_json: graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: &closed,
        };
        let agent = RecordingCloser {
            models: RefCell::new(Vec::new()),
            closed: &closed,
            next_close: Cell::new(10),
        };
        let profiles = RunEphemeralProfileMap::from([
            (
                10,
                PerIssueInvocationOverride {
                    agent: None,
                    model: Some("issue-one-model".to_string()),
                    reasoning_effort: None,
                },
            ),
            (
                11,
                PerIssueInvocationOverride {
                    agent: None,
                    model: Some("issue-two-model".to_string()),
                    reasoning_effort: None,
                },
            ),
        ]);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, Some("run-model"), None),
            profiles,
            PreflightDimensions::default(),
            "test directives",
        );

        run(config, runtime(&github, &agent)).unwrap();
        assert_eq!(
            *agent.models.borrow(),
            vec![
                Some("issue-one-model".to_string()),
                Some("issue-two-model".to_string())
            ]
        );
    }

    #[test]
    fn missing_prd_is_an_error() {
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::new(),
            closed: HashSet::new(),
        };
        let agent = idle_agent();
        let config = workflow(
            0,
            true,
            defaults(Agent::Cursor, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            "test directives",
        );
        let err = run(config, runtime(&github, &agent)).unwrap_err();
        assert!(err.to_string().contains("PRD issue 42 not found"));
    }

    #[test]
    fn empty_children_stops_cleanly() {
        let github = MockGithub {
            issues_json: "[]".into(),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        };
        let agent = idle_agent();
        let config = workflow(
            0,
            true,
            defaults(Agent::Cursor, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            "test directives",
        );
        run(config, runtime(&github, &agent)).unwrap();
        assert!(!agent.ran.get());
    }

    fn assert_no_github_calls(github: &CountingGithub) {
        assert_eq!(github.fetch_body_calls.get(), 0);
        assert_eq!(github.fetch_issues_calls.get(), 0);
    }

    #[test]
    fn illegal_whole_run_profile_fails_before_any_github_call() {
        let cases = [
            (
                Agent::Antigravity,
                Some("gemini"),
                None,
                "does not support --model",
            ),
            (
                Agent::Cursor,
                None,
                Some("high"),
                "does not support --reasoning-effort",
            ),
            (
                Agent::Claude,
                None,
                Some("xhigh"),
                "does not support --reasoning-effort xhigh",
            ),
        ];
        for (agent, model, effort, needle) in cases {
            let github = CountingGithub {
                fetch_body_calls: Cell::new(0),
                fetch_issues_calls: Cell::new(0),
            };
            let runner = idle_agent();
            let config = workflow(
                0,
                false,
                defaults(agent, model, effort),
                RunEphemeralProfileMap::new(),
                PreflightDimensions::default(),
                "test directives",
            );
            let error = run(config, runtime(&github, &runner))
                .expect_err("illegal whole-run profile must fail at startup")
                .to_string();
            assert!(error.contains(needle), "{error}");
            assert_no_github_calls(&github);
            assert!(!runner.ran.get());
        }
    }

    #[test]
    fn pick_efforts_without_pick_agents_fails_before_any_github_call() {
        for agent in [Agent::Cursor, Agent::Antigravity] {
            let github = CountingGithub {
                fetch_body_calls: Cell::new(0),
                fetch_issues_calls: Cell::new(0),
            };
            let runner = idle_agent();
            let config = workflow(
                1,
                false,
                defaults(agent, None, None),
                RunEphemeralProfileMap::new(),
                PreflightDimensions {
                    agents: false,
                    efforts: true,
                    models: false,
                },
                "test directives",
            );
            let error = run_with_preflight_io(config, runtime(&github, &runner), None)
                .expect_err("incapable agent must reject --pick-efforts")
                .to_string();
            assert!(error.contains("does not support --pick-efforts"));
            assert_no_github_calls(&github);
            assert!(!runner.ran.get());
        }
    }

    #[test]
    fn pick_models_on_antigravity_fails_before_any_github_call() {
        let github = CountingGithub {
            fetch_body_calls: Cell::new(0),
            fetch_issues_calls: Cell::new(0),
        };
        let runner = idle_agent();
        let config = workflow(
            1,
            false,
            defaults(Agent::Antigravity, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                agents: false,
                efforts: false,
                models: true,
            },
            "test directives",
        );
        let error = run_with_preflight_io(config, runtime(&github, &runner), None)
            .expect_err("antigravity must reject --pick-models")
            .to_string();
        assert!(error.contains("does not support --model"));
        assert!(error.contains("--pick-models"));
        assert_no_github_calls(&github);
        assert!(!runner.ran.get());
    }

    #[test]
    fn non_tty_pick_flags_fail_before_any_github_call() {
        let github = CountingGithub {
            fetch_body_calls: Cell::new(0),
            fetch_issues_calls: Cell::new(0),
        };
        let runner = idle_agent();
        let mut input = Cursor::new(b"".as_slice());
        let mut output = Vec::new();
        let mut preflight_io = crate::preflight::Io::new(false, &mut input, &mut output);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                agents: false,
                efforts: true,
                models: false,
            },
            "test directives",
        );
        let error =
            run_with_preflight_io(config, runtime(&github, &runner), Some(&mut preflight_io))
                .expect_err("non-terminal preflight must fail")
                .to_string();
        assert!(error.contains("--reasoning-effort"));
        assert_no_github_calls(&github);
        assert!(!runner.ran.get());
    }

    #[test]
    fn agent_preflight_to_pi_uses_github_built_in_directives() {
        struct RecordingCloser<'a> {
            agents: RefCell<Vec<Agent>>,
            prompts: RefCell<Vec<String>>,
            closed: &'a RefCell<HashSet<u32>>,
            next_close: Cell<u32>,
        }

        impl AgentRunner for RecordingCloser<'_> {
            fn run(
                &self,
                agent: Agent,
                _profile: InvocationProfile<'_>,
                prompt: &str,
            ) -> Result<(), Box<dyn std::error::Error>> {
                self.agents.borrow_mut().push(agent);
                self.prompts.borrow_mut().push(prompt.to_string());
                let number = self.next_close.get();
                self.closed.borrow_mut().insert(number);
                self.next_close.set(number + 1);
                Ok(())
            }
        }

        let closed = RefCell::new(HashSet::new());
        let github = SharedGithub {
            issues_json: graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: &closed,
        };
        let agent = RecordingCloser {
            agents: RefCell::new(Vec::new()),
            prompts: RefCell::new(Vec::new()),
            closed: &closed,
            next_close: Cell::new(10),
        };
        let mut input = Cursor::new(b"5\n\n\n".as_slice());
        let mut output = Vec::new();
        let mut preflight_io = crate::preflight::Io::new(true, &mut input, &mut output);
        let config = workflow(
            1,
            false,
            defaults(Agent::Claude, Some("run-model"), Some("high")),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                agents: true,
                efforts: false,
                models: false,
            },
            BUILT_IN_DIRECTIVES,
        );

        run_with_preflight_io(config, runtime(&github, &agent), Some(&mut preflight_io))
            .expect("two-issue pick-agents run should finish");
        assert_eq!(*agent.agents.borrow(), vec![Agent::Pi, Agent::Claude]);
        let prompts = agent.prompts.borrow();
        assert!(!prompts[0].contains("sub-agents"));
        assert!(!prompts[0].contains("pi -p --no-session"));
        assert!(prompts[1].contains("sub-agents"));
        assert!(
            String::from_utf8(output)
                .expect("preflight output is utf-8")
                .contains("Agent choices")
        );
    }

    #[test]
    fn effort_preflight_excludes_forever_blocked_issues_from_scripted_input() {
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[]), child(11, 42, &[(99, "OPEN")])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        };
        let agent = ProfileRecordingAgent {
            received_profile: RefCell::new(None),
        };
        let mut input = Cursor::new(b"5\n\n".as_slice());
        let mut output = Vec::new();
        let mut preflight_io = crate::preflight::Io::new(true, &mut input, &mut output);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                agents: false,
                efforts: true,
                models: false,
            },
            "test directives",
        );

        let error =
            run_with_preflight_io(config, runtime(&github, &agent), Some(&mut preflight_io))
                .expect_err("fake agent stops after recording preflight profile")
                .to_string();
        assert!(error.contains("mock agent stopped after recording profile"));
        assert_eq!(
            *agent.received_profile.borrow(),
            Some((None, Some("high".to_string())))
        );
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("#10"));
        assert!(!output.contains("#11"));
    }

    #[test]
    fn pick_models_dry_run_never_invokes_agent() {
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        };
        let agent = idle_agent();
        let mut input = Cursor::new(b"issue-model\n5\n\n".as_slice());
        let mut output = Vec::new();
        let mut preflight_io = crate::preflight::Io::new(true, &mut input, &mut output);
        let config = workflow(
            1,
            true,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                agents: false,
                efforts: false,
                models: true,
            },
            "test directives",
        );

        run_with_preflight_io(config, runtime(&github, &agent), Some(&mut preflight_io))
            .expect("dry run should finish after full profile preflight");
        assert!(!agent.ran.get());
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("model = free string; blank keeps default"));
        assert!(output.contains("Continue dry-run?"));
    }

    #[test]
    fn effort_preflight_abort_prevents_agent_invocation() {
        let github = MockGithub {
            issues_json: graph(&[child(10, 42, &[])]),
            bodies: HashMap::from([(42, "Product requirements".into())]),
            closed: HashSet::new(),
        };
        let agent = idle_agent();
        let mut input = Cursor::new(b"q\n".as_slice());
        let mut output = Vec::new();
        let mut preflight_io = crate::preflight::Io::new(true, &mut input, &mut output);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                agents: false,
                efforts: true,
                models: false,
            },
            "test directives",
        );

        let error =
            run_with_preflight_io(config, runtime(&github, &agent), Some(&mut preflight_io))
                .expect_err("preflight abort must stop the run")
                .to_string();
        assert!(error.contains("Preflight aborted"));
        assert!(!agent.ran.get());
    }
}
