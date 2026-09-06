//! Coordinates the PRD issue loop.
//!
//! Fetches ready GitHub issues, asks [`crate::parser`] which child to run, and
//! invokes the configured agent with a rendered prompt. Also owns dry-run
//! behavior, git hygiene, Invocation Profile Preflight, cooked vs `--tui`
//! output, and the post-agent check that the selected issue was closed.

use crate::agent::AgentRunner;
use crate::console;
use crate::git::GitOps;
use crate::github::{GithubIssue, GithubIssues};
use crate::invocation_profile::{
    InvocationProfile, PreflightDimensions, RunEphemeralProfileMap, WholeRunInvocationDefaults,
    resolve,
};
use crate::parser::{IssuePlan, plan_order};
use crate::prompt::{
    DirectivePolicy, directives_for_invocation, render_issue_prompt, save_prompt_copy,
};
use crate::tui::{LiveHeader, NullWatch, RosterIssue, Watch, WatchEvent};
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
    /// When true, print planned-order assignment lines and the first prompt without invoking an agent; requested preflight still runs.
    pub dry_run: bool,
    /// When true, drive the Watch Board after cooked preflight instead of streaming console banners.
    pub tui: bool,
    /// Whole-Run Invocation Defaults resolved for every issue without an override.
    pub whole_run_defaults: WholeRunInvocationDefaults<'a>,
    /// Run-Ephemeral Profile Map whose per-issue fields override whole-run defaults.
    pub per_issue_profiles: RunEphemeralProfileMap,
    /// Invocation Profile Preflight columns collected after plan simulation.
    pub preflight_dimensions: PreflightDimensions,
    /// Run-wide maintainer-directive policy applied to each generated issue prompt unless a per-issue prompt override is present.
    pub directive_policy: DirectivePolicy<'a>,
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

#[cfg(test)]
fn run_with_preflight_io(
    mut config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    preflight_io: Option<&mut crate::preflight::Io<'_>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let prd_body = startup(&mut config, &runtime, preflight_io)?;
    after_prepare(config, runtime, &prd_body, None)
}

#[cfg(test)]
fn run_watched(
    mut config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    watch: &dyn Watch,
) -> Result<(), Box<dyn std::error::Error>> {
    let prd_body = startup(&mut config, &runtime, None)?;
    after_prepare(config, runtime, &prd_body, Some(watch))
}

#[cfg(test)]
fn run_watched_with_preflight(
    mut config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    preflight_io: &mut crate::preflight::Io<'_>,
    watch: &dyn Watch,
) -> Result<(), Box<dyn std::error::Error>> {
    let prd_body = startup(&mut config, &runtime, Some(preflight_io))?;
    after_prepare(config, runtime, &prd_body, Some(watch))
}

fn startup(
    config: &mut WorkflowConfig<'_>,
    runtime: &Runtime<'_>,
    mut preflight_io: Option<&mut crate::preflight::Io<'_>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let default_agent = config.whole_run_defaults.agent;
    let whole_run_profile = resolve(config.whole_run_defaults, None);
    default_agent.get_command_with_profile(whole_run_profile)?;
    let dimensions = config.preflight_dimensions;
    if dimensions.efforts && dimensions.models {
        return Err("nightshift: --pick-efforts and --pick-models are mutually exclusive".into());
    }
    let preflight_enabled = dimensions.requested();
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

    if !config.tui {
        console::session_start(config.prd);
    }

    if preflight_enabled {
        let issues_json = runtime
            .github
            .fetch_issues(config.repo)
            .map_err(|e| format!("nightshift: failed to fetch issues: {}. Exiting.", e))?;
        let plan = plan_order(&issues_json, config.prd, config.issue)?;
        let Some(io) = preflight_io.as_mut() else {
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

    Ok(prd_body)
}

fn after_prepare(
    config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    prd_body: &str,
    watch: Option<&dyn Watch>,
) -> Result<(), Box<dyn std::error::Error>> {
    if config.tui && watch.is_none() {
        let header = LiveHeader {
            prd: config.prd,
            repo: config.repo.to_string(),
            branch: config.base_branch.to_string(),
        };
        let preview =
            crate::tui::with_live_board(header, |watch| drive(config, runtime, prd_body, watch))?;
        print_deferred_dry_run(preview);
        return Ok(());
    }
    let null = NullWatch;
    let watch = watch.unwrap_or(&null);
    let preview = drive(config, runtime, prd_body, watch)?;
    print_deferred_dry_run(preview);
    Ok(())
}

fn drive(
    config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    prd_body: &str,
    watch: &dyn Watch,
) -> Result<Option<DeferredDryRun>, Box<dyn std::error::Error>> {
    watch.emit(WatchEvent::Session {
        prd: config.prd,
        repo: config.repo.to_string(),
        branch: config.base_branch.to_string(),
    });
    if config.dry_run {
        run_dry_run(config, runtime, prd_body, watch)
    } else {
        run_loop(config, runtime, prd_body, watch)?;
        Ok(None)
    }
}

fn run_loop(
    config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    prd_body: &str,
    watch: &dyn Watch,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        if !config.tui {
            console::git_hygiene(config.repo, config.base_branch);
        }
        watch.emit(WatchEvent::Hygiene);
        if let Err(e) = runtime.git.ensure_hygiene(config.base_branch) {
            return fail(
                watch,
                config.prd,
                format!("nightshift: git hygiene check failed: {}. Exiting.", e),
            );
        }
        if watch.stop_requested() {
            watch.emit(WatchEvent::Done);
            break;
        }

        let issues_json = match runtime.github.fetch_issues(config.repo) {
            Ok(json) => json,
            Err(e) => {
                return fail(
                    watch,
                    config.prd,
                    format!("nightshift: failed to fetch issues: {}. Exiting.", e),
                );
            }
        };

        let plan = match plan_order(&issues_json, config.prd, config.issue) {
            Ok(plan) => plan,
            Err(e) => return fail(watch, config.prd, e.to_string()),
        };
        emit_roster(watch, &plan, &config);
        if watch.stop_requested() {
            watch.emit(WatchEvent::Done);
            break;
        }

        let Some(selected_issue) = plan.planned.into_iter().next() else {
            if plan.blocked.is_empty() {
                complete_without_candidates(
                    config.prd,
                    config.issue,
                    plan.has_open_children,
                    config.tui,
                );
            } else if !config.tui {
                console::loop_complete("All remaining issues are blocked");
            }
            watch.emit(WatchEvent::Done);
            break;
        };

        if watch.stop_requested() {
            watch.emit(WatchEvent::Done);
            break;
        }

        let profile = resolve(
            config.whole_run_defaults,
            config.per_issue_profiles.get(&selected_issue.number),
        );
        watch.emit(WatchEvent::Dispatch {
            issue: selected_issue.number,
        });

        let issue_run = if config.tui {
            None
        } else {
            let run = console::IssueRun::begin(selected_issue.number, &selected_issue.title);
            run.invocation_profile(profile);
            Some(run)
        };

        let per_issue = config
            .per_issue_profiles
            .get(&selected_issue.number)
            .and_then(|row| row.prompt.as_ref());
        let directives =
            directives_for_invocation(config.directive_policy, per_issue, profile.agent);
        let final_prompt =
            render_issue_prompt(config.repo, prd_body, &selected_issue, directives.as_ref());

        if let Some(path) = save_prompt_copy(selected_issue.number, &final_prompt)
            && let Some(run) = issue_run.as_ref()
        {
            run.meta(&format!("prompt {}", path.display()));
        }

        watch.emit(WatchEvent::Running {
            issue: selected_issue.number,
        });
        if let Err(e) = runtime.agent_runner.run(profile, &final_prompt) {
            return fail(watch, selected_issue.number, e.to_string());
        }

        watch.emit(WatchEvent::Verify {
            issue: selected_issue.number,
        });
        match runtime
            .github
            .is_issue_closed(config.repo, selected_issue.number)
        {
            Ok(true) => {}
            Ok(false) => {
                return fail(
                    watch,
                    selected_issue.number,
                    format!(
                        "nightshift: agent exited successfully, but issue #{} is still open on GitHub.\n                Exiting.",
                        selected_issue.number
                    ),
                );
            }
            Err(e) => return fail(watch, selected_issue.number, e.to_string()),
        }
        watch.emit(WatchEvent::Completed {
            issue: selected_issue.number,
        });
        if let Some(run) = issue_run {
            run.complete();
        }
        if watch.stop_requested() {
            watch.emit(WatchEvent::Done);
            break;
        }
    }

    Ok(())
}

fn fail<T>(
    watch: &dyn Watch,
    issue: u32,
    message: String,
) -> Result<T, Box<dyn std::error::Error>> {
    watch.emit(WatchEvent::Failed {
        issue,
        message: message.clone(),
    });
    Err(message.into())
}

fn emit_roster(watch: &dyn Watch, plan: &IssuePlan, config: &WorkflowConfig<'_>) {
    watch.emit(WatchEvent::Roster {
        planned: plan
            .planned
            .iter()
            .map(|issue| roster_issue(issue, config))
            .collect(),
        blocked: plan
            .blocked
            .iter()
            .map(|issue| roster_issue(issue, config))
            .collect(),
    });
}

fn roster_issue(issue: &GithubIssue, config: &WorkflowConfig<'_>) -> RosterIssue {
    let profile = resolve(
        config.whole_run_defaults,
        config.per_issue_profiles.get(&issue.number),
    );
    RosterIssue {
        number: issue.number,
        title: issue.title.clone(),
        agent: profile.agent.name().to_string(),
        model: profile.model.map(str::to_string),
        effort: profile.reasoning_effort.map(str::to_string),
    }
}

/// Runs the PRD child-issue loop until no eligible candidate remains.
///
/// Each iteration enforces git hygiene, fetches open `ready-for-agent` issues,
/// filters direct children of the requested PRD, then selects the
/// lowest-numbered unblocked child. Dry runs print the full simulated solve order,
/// the agent command preview, and the first planned issue's prompt, then exit.
/// With `--tui`, that preview prints only after the Watch Board is dismissed.
/// Non-dry runs save a prompt copy, invoke the agent,
/// and require the selected GitHub issue to be closed before continuing.
///
/// When pick flags are set, Invocation Profile Preflight runs before dry-run
/// or the live loop. Cooked mode prints a session header first; `--tui` skips
/// that header. Git hygiene stays inside those later phases. `--tui` starts the
/// Watch Board only after cooked preflight releases stdin/stdout. While the
/// board is active, `q` / Ctrl-C request stop after the current issue without
/// killing the agent.
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
/// # use nightshift::prompt::DirectivePolicy;
/// # let github = GhCliAdapter;
/// # let git = GitCliAdapter::for_repo("owner/repo")?;
/// # let agent_runner = ProcessAgentRunner;
/// let config = WorkflowConfig {
///     prd: 42,
///     issue: 0,
///     repo: "owner/repo",
///     base_branch: "main",
///     dry_run: true,
///     tui: false,
///     whole_run_defaults: WholeRunInvocationDefaults {
///         agent: Agent::Cursor,
///         model: Some("gpt-5.2"),
///         reasoning_effort: None,
///     },
///     per_issue_profiles: RunEphemeralProfileMap::new(),
///     preflight_dimensions: PreflightDimensions::default(),
///     directive_policy: DirectivePolicy::Replace("Run tests before opening a PR."),
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
    mut config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let prd_body = if !config.preflight_dimensions.requested() {
        startup(&mut config, &runtime, None)?
    } else {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let terminal = stdin.is_terminal() && stdout.is_terminal();
        let mut input = stdin.lock();
        let mut output = stdout.lock();
        let mut io = crate::preflight::Io::new(terminal, &mut input, &mut output);
        startup(&mut config, &runtime, Some(&mut io))?
    };
    after_prepare(config, runtime, &prd_body, None)
}

type DryRunPreview<'a> = (
    Vec<(
        u32,
        &'a str,
        crate::invocation_profile::InvocationProfile<'a>,
    )>,
    String,
);

/// Builds resolved dry-run assignments and first-issue command preview.
///
/// Every planned child issue receives its own resolved profile. The command
/// uses the first assignment because the dry run previews the loop's next
/// agent invocation.
fn build_dry_run_preview<'a>(
    planned: &'a [GithubIssue],
    defaults: WholeRunInvocationDefaults<'a>,
    profiles: &'a RunEphemeralProfileMap,
) -> Result<DryRunPreview<'a>, String> {
    let assignments: Vec<_> = planned
        .iter()
        .map(|issue| {
            (
                issue.number,
                issue.title.as_str(),
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

struct DeferredAssignment {
    number: u32,
    title: String,
    agent: crate::agent::Agent,
    model: Option<String>,
    effort: Option<String>,
}

struct DeferredDryRun {
    planned: Vec<DeferredAssignment>,
    blocked: Vec<(u32, String)>,
    agent_cmd: String,
    prompt: Option<String>,
}

fn print_deferred_dry_run(preview: Option<DeferredDryRun>) {
    let Some(preview) = preview else {
        return;
    };
    let planned: Vec<(u32, &str, InvocationProfile<'_>)> = preview
        .planned
        .iter()
        .map(|row| {
            (
                row.number,
                row.title.as_str(),
                InvocationProfile {
                    agent: row.agent,
                    model: row.model.as_deref(),
                    reasoning_effort: row.effort.as_deref(),
                },
            )
        })
        .collect();
    let blocked: Vec<(u32, &str)> = preview
        .blocked
        .iter()
        .map(|(number, title)| (*number, title.as_str()))
        .collect();
    console::dry_run_planned_order(&planned, &blocked, &preview.agent_cmd);
    if let Some(prompt) = &preview.prompt {
        console::dry_run_prompt(prompt);
    }
}

/// Runs a dry-run pass: planned order, agent command preview, and first-issue prompt.
fn run_dry_run(
    config: WorkflowConfig<'_>,
    runtime: Runtime<'_>,
    prd_body: &str,
    watch: &dyn Watch,
) -> Result<Option<DeferredDryRun>, Box<dyn std::error::Error>> {
    if !config.tui {
        console::git_hygiene(config.repo, config.base_branch);
    }
    watch.emit(WatchEvent::Hygiene);

    if let Err(e) = runtime.git.ensure_hygiene(config.base_branch) {
        return fail(
            watch,
            config.prd,
            format!("nightshift: git hygiene check failed: {}. Exiting.", e),
        );
    }

    let issues_json = match runtime.github.fetch_issues(config.repo) {
        Ok(json) => json,
        Err(e) => {
            return fail(
                watch,
                config.prd,
                format!("nightshift: failed to fetch issues: {}. Exiting.", e),
            );
        }
    };

    let plan = match plan_order(&issues_json, config.prd, config.issue) {
        Ok(plan) => plan,
        Err(e) => return fail(watch, config.prd, e.to_string()),
    };
    emit_roster(watch, &plan, &config);

    if plan.planned.is_empty() && plan.blocked.is_empty() {
        complete_without_candidates(config.prd, config.issue, plan.has_open_children, config.tui);
        watch.emit(WatchEvent::Done);
        return Ok(None);
    }

    let (planned, agent_cmd) = match build_dry_run_preview(
        &plan.planned,
        config.whole_run_defaults,
        &config.per_issue_profiles,
    ) {
        Ok(preview) => preview,
        Err(e) => return fail(watch, config.prd, e),
    };
    let blocked: Vec<(u32, String)> = plan
        .blocked
        .iter()
        .map(|issue| (issue.number, issue.title.clone()))
        .collect();

    let prompt = if let Some(first) = plan.planned.first() {
        let profile = resolve(
            config.whole_run_defaults,
            config.per_issue_profiles.get(&first.number),
        );
        let per_issue = config
            .per_issue_profiles
            .get(&first.number)
            .and_then(|row| row.prompt.as_ref());
        let directives =
            directives_for_invocation(config.directive_policy, per_issue, profile.agent);
        let final_prompt = render_issue_prompt(config.repo, prd_body, first, directives.as_ref());
        #[cfg(test)]
        LAST_RENDERED_PROMPT.with(|slot| *slot.borrow_mut() = Some(final_prompt.clone()));
        Some(final_prompt)
    } else {
        None
    };

    watch.emit(WatchEvent::Done);
    Ok(Some(DeferredDryRun {
        planned: planned
            .iter()
            .map(|(number, title, profile)| DeferredAssignment {
                number: *number,
                title: (*title).to_string(),
                agent: profile.agent,
                model: profile.model.map(str::to_string),
                effort: profile.reasoning_effort.map(str::to_string),
            })
            .collect(),
        blocked,
        agent_cmd,
        prompt,
    }))
}

fn complete_without_candidates(prd: u32, min_issue: u32, has_open_children: bool, tui: bool) {
    if tui {
        return;
    }
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
thread_local! {
    static LAST_RENDERED_PROMPT: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
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
    use crate::prompt::{PerIssuePrompt, PromptMode, default_directives_for};
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
        directive_policy: DirectivePolicy<'a>,
    ) -> WorkflowConfig<'a> {
        WorkflowConfig {
            prd: 42,
            issue,
            repo: "foobar/repo",
            base_branch: "main",
            dry_run,
            tui: false,
            whole_run_defaults,
            per_issue_profiles,
            preflight_dimensions,
            directive_policy,
        }
    }

    struct MockGithub {
        issues_json: String,
        bodies: HashMap<u32, String>,
        closed: RefCell<HashSet<u32>>,
        fetch_body_calls: Cell<u32>,
        fetch_issues_calls: Cell<u32>,
    }

    impl GithubIssues for MockGithub {
        fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
            Ok(repo.unwrap_or("foobar/repo").to_string())
        }

        fn fetch_issues(&self, _repo: &str) -> Result<String, Box<dyn std::error::Error>> {
            self.fetch_issues_calls
                .set(self.fetch_issues_calls.get() + 1);
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
            self.fetch_body_calls.set(self.fetch_body_calls.get() + 1);
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

    fn mock_github(issues_json: impl Into<String>, bodies: HashMap<u32, String>) -> MockGithub {
        MockGithub {
            issues_json: issues_json.into(),
            bodies,
            closed: RefCell::new(HashSet::new()),
            fetch_body_calls: Cell::new(0),
            fetch_issues_calls: Cell::new(0),
        }
    }

    struct RecordingCloser<'a> {
        agents: RefCell<Vec<Agent>>,
        models: RefCell<Vec<Option<String>>>,
        prompts: RefCell<Vec<String>>,
        closed: &'a RefCell<HashSet<u32>>,
        next_close: Cell<u32>,
    }

    impl AgentRunner for RecordingCloser<'_> {
        fn run(
            &self,
            profile: InvocationProfile<'_>,
            prompt: &str,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.agents.borrow_mut().push(profile.agent);
            self.models
                .borrow_mut()
                .push(profile.model.map(str::to_string));
            self.prompts.borrow_mut().push(prompt.to_string());
            let number = self.next_close.get();
            self.closed.borrow_mut().insert(number);
            self.next_close.set(number + 1);
            Ok(())
        }
    }

    fn recording_closer(closed: &RefCell<HashSet<u32>>) -> RecordingCloser<'_> {
        RecordingCloser {
            agents: RefCell::new(Vec::new()),
            models: RefCell::new(Vec::new()),
            prompts: RefCell::new(Vec::new()),
            closed,
            next_close: Cell::new(10),
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
        received_effort: RefCell<Option<String>>,
        error_on_run: bool,
    }

    impl AgentRunner for MockAgent {
        fn run(
            &self,
            profile: InvocationProfile<'_>,
            _prompt: &str,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.ran.set(true);
            self.received_model
                .replace(profile.model.map(str::to_string));
            self.received_effort
                .replace(profile.reasoning_effort.map(str::to_string));
            if self.error_on_run {
                return Err("mock agent stopped after recording run".into());
            }
            Ok(())
        }
    }

    fn prd_github() -> MockGithub {
        mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        )
    }

    fn idle_agent() -> MockAgent {
        MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            received_effort: RefCell::new(None),
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
            DirectivePolicy::Replace("test directives"),
        );
        run(config, runtime(&github, &agent)).unwrap();
        assert!(!agent.ran.get());
    }

    #[test]
    fn dry_run_accepts_explicit_model_without_invoking_agent() {
        let github = mock_github(
            graph(&[child(10, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = idle_agent();
        let config = workflow(
            1,
            true,
            defaults(Agent::Cursor, Some("gpt-5.2"), None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
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
                    agent: Some(Agent::Codex),
                    model: Some("issue-one-model".to_string()),
                    reasoning_effort: Some("high".to_string()),
                    ..Default::default()
                },
            ),
            (
                11,
                PerIssueInvocationOverride {
                    agent: None,
                    model: Some("issue-two-model".to_string()),
                    reasoning_effort: Some("low".to_string()),
                    ..Default::default()
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
        assert_eq!(assignments[0].2.agent, Agent::Codex);
        assert_eq!(assignments[0].2.model, Some("issue-one-model"));
        assert_eq!(assignments[0].2.reasoning_effort, Some("high"));
        assert_eq!(assignments[1].0, 11);
        assert_eq!(assignments[1].2.agent, Agent::Pi);
        assert_eq!(assignments[1].2.model, Some("issue-two-model"));
        assert_eq!(assignments[1].2.reasoning_effort, Some("low"));
        assert_eq!(
            command,
            "codex exec --model issue-one-model -c model_reasoning_effort=high - --ephemeral"
        );
    }

    #[test]
    fn non_dry_run_passes_explicit_model_to_agent_runner() {
        let github = mock_github(
            graph(&[child(10, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            received_effort: RefCell::new(None),
            error_on_run: true,
        };
        let config = workflow(
            1,
            false,
            defaults(Agent::Cursor, Some("gpt-5.2"), None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        let err = run(config, runtime(&github, &agent)).unwrap_err();
        assert!(err.to_string().contains("mock agent stopped"));
        assert!(agent.ran.get());
        assert_eq!(agent.received_model.borrow().as_deref(), Some("gpt-5.2"));
    }

    #[test]
    fn non_dry_run_resolves_per_issue_profile_before_agent_invocation() {
        let github = mock_github(
            graph(&[child(10, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            received_effort: RefCell::new(None),
            error_on_run: true,
        };
        let profiles = RunEphemeralProfileMap::from([(
            10,
            PerIssueInvocationOverride {
                agent: None,
                model: Some("issue-model".to_string()),
                reasoning_effort: None,
                ..Default::default()
            },
        )]);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, Some("run-model"), Some("medium")),
            profiles,
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );

        let error = run(config, runtime(&github, &agent))
            .expect_err("fake agent stops after recording resolved profile")
            .to_string();
        assert!(error.contains("mock agent stopped"));
        assert_eq!(
            agent.received_model.borrow().as_deref(),
            Some("issue-model")
        );
        assert_eq!(agent.received_effort.borrow().as_deref(), Some("medium"));
    }

    #[test]
    fn non_dry_run_uses_defaults_for_unmapped_issue_profile() {
        let github = mock_github(
            graph(&[child(10, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            received_effort: RefCell::new(None),
            error_on_run: true,
        };
        let profiles = RunEphemeralProfileMap::from([(
            11,
            PerIssueInvocationOverride {
                agent: None,
                model: Some("other-issue-model".to_string()),
                reasoning_effort: Some("low".to_string()),
                ..Default::default()
            },
        )]);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, Some("run-model"), Some("medium")),
            profiles,
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );

        let error = run(config, runtime(&github, &agent))
            .expect_err("fake agent stops after recording defaults")
            .to_string();
        assert!(error.contains("mock agent stopped"));
        assert_eq!(agent.received_model.borrow().as_deref(), Some("run-model"));
        assert_eq!(agent.received_effort.borrow().as_deref(), Some("medium"));
    }

    #[test]
    fn live_loop_resolves_a_second_issue_to_a_different_model() {
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = recording_closer(&github.closed);
        let profiles = RunEphemeralProfileMap::from([
            (
                10,
                PerIssueInvocationOverride {
                    agent: None,
                    model: Some("issue-one-model".to_string()),
                    reasoning_effort: None,
                    ..Default::default()
                },
            ),
            (
                11,
                PerIssueInvocationOverride {
                    agent: None,
                    model: Some("issue-two-model".to_string()),
                    reasoning_effort: None,
                    ..Default::default()
                },
            ),
        ]);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, Some("run-model"), None),
            profiles,
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
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
        let github = mock_github(graph(&[child(10, 42, &[])]), HashMap::new());
        let agent = idle_agent();
        let config = workflow(
            0,
            true,
            defaults(Agent::Cursor, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        let err = run(config, runtime(&github, &agent)).unwrap_err();
        assert!(err.to_string().contains("PRD issue 42 not found"));
    }

    #[test]
    fn empty_children_stops_cleanly() {
        let github = mock_github("[]", HashMap::from([(42, "Product requirements".into())]));
        let agent = idle_agent();
        let config = workflow(
            0,
            true,
            defaults(Agent::Cursor, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        run(config, runtime(&github, &agent)).unwrap();
        assert!(!agent.ran.get());
    }

    fn assert_no_github_calls(github: &MockGithub) {
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
            let github = mock_github("[]", HashMap::new());
            let runner = idle_agent();
            let config = workflow(
                0,
                false,
                defaults(agent, model, effort),
                RunEphemeralProfileMap::new(),
                PreflightDimensions::default(),
                DirectivePolicy::Replace("test directives"),
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
            let github = mock_github("[]", HashMap::new());
            let runner = idle_agent();
            let config = workflow(
                1,
                false,
                defaults(agent, None, None),
                RunEphemeralProfileMap::new(),
                PreflightDimensions {
                    efforts: true,
                    ..PreflightDimensions::default()
                },
                DirectivePolicy::Replace("test directives"),
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
        let github = mock_github("[]", HashMap::new());
        let runner = idle_agent();
        let config = workflow(
            1,
            false,
            defaults(Agent::Antigravity, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                models: true,
                ..PreflightDimensions::default()
            },
            DirectivePolicy::Replace("test directives"),
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
        let github = mock_github("[]", HashMap::new());
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
                efforts: true,
                ..PreflightDimensions::default()
            },
            DirectivePolicy::Replace("test directives"),
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
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = recording_closer(&github.closed);
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
                ..PreflightDimensions::default()
            },
            DirectivePolicy::BuiltIn,
        );

        run_with_preflight_io(config, runtime(&github, &agent), Some(&mut preflight_io))
            .expect("two-issue pick-agents run should finish");
        assert_eq!(*agent.agents.borrow(), vec![Agent::Pi, Agent::Claude]);
        let prompts = agent.prompts.borrow();
        assert!(!prompts[0].contains("sub-agents"));
        assert!(prompts[0].contains("pi -p --no-session"));
        assert!(prompts[1].contains("sub-agents"));
        assert!(
            String::from_utf8(output)
                .expect("preflight output is utf-8")
                .contains("Agent choices")
        );
    }

    #[test]
    fn effort_preflight_excludes_forever_blocked_issues_from_scripted_input() {
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[(99, "OPEN")])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            received_effort: RefCell::new(None),
            error_on_run: true,
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
                efforts: true,
                ..PreflightDimensions::default()
            },
            DirectivePolicy::Replace("test directives"),
        );

        let error =
            run_with_preflight_io(config, runtime(&github, &agent), Some(&mut preflight_io))
                .expect_err("fake agent stops after recording preflight profile")
                .to_string();
        assert!(error.contains("mock agent stopped"));
        assert_eq!(agent.received_model.borrow().as_deref(), None);
        assert_eq!(agent.received_effort.borrow().as_deref(), Some("high"));
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("#10"));
        assert!(!output.contains("#11"));
    }

    #[test]
    fn pick_models_dry_run_never_invokes_agent() {
        let github = mock_github(
            graph(&[child(10, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
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
                models: true,
                ..PreflightDimensions::default()
            },
            DirectivePolicy::Replace("test directives"),
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
        let github = mock_github(
            graph(&[child(10, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
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
                efforts: true,
                ..PreflightDimensions::default()
            },
            DirectivePolicy::Replace("test directives"),
        );

        let error =
            run_with_preflight_io(config, runtime(&github, &agent), Some(&mut preflight_io))
                .expect_err("preflight abort must stop the run")
                .to_string();
        assert!(error.contains("Preflight aborted"));
        assert!(!agent.ran.get());
    }

    #[test]
    fn prompt_file_without_pick_flags_replaces_directives_for_every_issue() {
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = recording_closer(&github.closed);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("F"),
        );

        run(config, runtime(&github, &agent)).unwrap();
        let prompts = agent.prompts.borrow();
        assert_eq!(prompts.len(), 2);
        for prompt in prompts.iter() {
            assert!(prompt.contains("F"));
            assert!(!prompt.contains("gh pr create"));
        }
    }

    #[test]
    fn picked_prompt_replace_and_append_used_at_invoke() {
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = recording_closer(&github.closed);
        let profiles = RunEphemeralProfileMap::from([
            (
                10,
                PerIssueInvocationOverride {
                    prompt: Some(PerIssuePrompt {
                        mode: PromptMode::Replace,
                        contents: "row-replace-only".into(),
                    }),
                    ..Default::default()
                },
            ),
            (
                11,
                PerIssueInvocationOverride {
                    prompt: Some(PerIssuePrompt {
                        mode: PromptMode::Append,
                        contents: "row-extra".into(),
                    }),
                    ..Default::default()
                },
            ),
        ]);
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, None, None),
            profiles,
            PreflightDimensions::default(),
            DirectivePolicy::Replace("run-wide-should-not-appear"),
        );

        run(config, runtime(&github, &agent)).unwrap();
        let prompts = agent.prompts.borrow();
        assert!(prompts[0].contains("row-replace-only"));
        assert!(!prompts[0].contains("run-wide-should-not-appear"));
        assert!(!prompts[0].contains("gh pr create"));
        assert!(prompts[1].contains("row-extra"));
        assert!(prompts[1].contains(&default_directives_for(Agent::Pi)));
        assert!(!prompts[1].contains("run-wide-should-not-appear"));
    }

    #[test]
    fn dry_run_first_issue_prompt_uses_seeded_per_issue_prompt() {
        LAST_RENDERED_PROMPT.with(|slot| *slot.borrow_mut() = None);
        let github = mock_github(
            graph(&[child(10, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = idle_agent();
        let profiles = RunEphemeralProfileMap::from([(
            10,
            PerIssueInvocationOverride {
                prompt: Some(PerIssuePrompt {
                    mode: PromptMode::Append,
                    contents: "row-extra".into(),
                }),
                ..Default::default()
            },
        )]);
        let config = workflow(
            1,
            true,
            defaults(Agent::Pi, None, None),
            profiles,
            PreflightDimensions::default(),
            DirectivePolicy::Replace("run-wide-should-not-appear"),
        );

        run(config, runtime(&github, &agent)).unwrap();
        assert!(!agent.ran.get());
        let prompt = LAST_RENDERED_PROMPT
            .with(|slot| slot.borrow().clone())
            .expect("dry-run should render the first-issue prompt");
        assert!(prompt.contains(&format!(
            "{}\n\nrow-extra",
            default_directives_for(Agent::Pi)
        )));
        assert!(!prompt.contains("run-wide-should-not-appear"));
    }

    #[test]
    fn non_tty_pick_prompts_fails_before_any_github_call() {
        let github = mock_github("[]", HashMap::new());
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
                prompts: true,
                ..PreflightDimensions::default()
            },
            DirectivePolicy::BuiltIn,
        );
        let error =
            run_with_preflight_io(config, runtime(&github, &runner), Some(&mut preflight_io))
                .expect_err("non-terminal pick-prompts must fail")
                .to_string();
        assert!(error.contains("--pick-prompts"));
        assert!(error.contains("--reasoning-effort"));
        assert_no_github_calls(&github);
        assert!(!runner.ran.get());

        let github = mock_github("[]", HashMap::new());
        let runner = idle_agent();
        let config = workflow(
            1,
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                prompts: true,
                ..PreflightDimensions::default()
            },
            DirectivePolicy::BuiltIn,
        );
        let error = run_with_preflight_io(config, runtime(&github, &runner), None)
            .expect_err("pick-prompts alone must not skip preflight I/O")
            .to_string();
        assert!(error.contains("Invocation Profile Preflight requires terminal I/O"));
        assert_no_github_calls(&github);
    }

    fn tui_config<'a>(
        dry_run: bool,
        whole_run_defaults: WholeRunInvocationDefaults<'a>,
        per_issue_profiles: RunEphemeralProfileMap,
        preflight_dimensions: PreflightDimensions,
        directive_policy: DirectivePolicy<'a>,
    ) -> WorkflowConfig<'a> {
        let mut config = workflow(
            1,
            dry_run,
            whole_run_defaults,
            per_issue_profiles,
            preflight_dimensions,
            directive_policy,
        );
        config.tui = true;
        config
    }

    fn events_contain(watch: &crate::tui::WatchLog, check: impl Fn(&WatchEvent) -> bool) -> bool {
        watch.events().iter().any(check)
    }

    struct StoppingGit<'a> {
        watch: &'a crate::tui::WatchLog,
    }

    impl GitOps for StoppingGit<'_> {
        fn base_branch_exists(&self, _base_branch: &str) -> bool {
            true
        }

        fn ensure_hygiene(&self, _base_branch: &str) -> Result<(), Box<dyn std::error::Error>> {
            self.watch.request_stop();
            Ok(())
        }
    }

    struct StoppingFetchGithub<'a> {
        inner: MockGithub,
        watch: &'a crate::tui::WatchLog,
    }

    impl GithubIssues for StoppingFetchGithub<'_> {
        fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
            self.inner.resolve_repo(repo)
        }

        fn fetch_issues(&self, repo: &str) -> Result<String, Box<dyn std::error::Error>> {
            let json = self.inner.fetch_issues(repo)?;
            self.watch.request_stop();
            Ok(json)
        }

        fn fetch_issue_body(
            &self,
            repo: &str,
            issue_number: u32,
        ) -> Result<String, Box<dyn std::error::Error>> {
            self.inner.fetch_issue_body(repo, issue_number)
        }

        fn is_issue_closed(
            &self,
            repo: &str,
            issue_number: u32,
        ) -> Result<bool, Box<dyn std::error::Error>> {
            self.inner.is_issue_closed(repo, issue_number)
        }
    }

    struct RawIssuesGithub {
        inner: MockGithub,
        payload: String,
    }

    impl GithubIssues for RawIssuesGithub {
        fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
            self.inner.resolve_repo(repo)
        }

        fn fetch_issues(&self, _repo: &str) -> Result<String, Box<dyn std::error::Error>> {
            Ok(self.payload.clone())
        }

        fn fetch_issue_body(
            &self,
            repo: &str,
            issue_number: u32,
        ) -> Result<String, Box<dyn std::error::Error>> {
            self.inner.fetch_issue_body(repo, issue_number)
        }

        fn is_issue_closed(
            &self,
            repo: &str,
            issue_number: u32,
        ) -> Result<bool, Box<dyn std::error::Error>> {
            self.inner.is_issue_closed(repo, issue_number)
        }
    }

    struct FailingFetchGithub {
        inner: MockGithub,
    }

    impl GithubIssues for FailingFetchGithub {
        fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
            self.inner.resolve_repo(repo)
        }

        fn fetch_issues(&self, _repo: &str) -> Result<String, Box<dyn std::error::Error>> {
            Err("gh: failed to list issues".into())
        }

        fn fetch_issue_body(
            &self,
            repo: &str,
            issue_number: u32,
        ) -> Result<String, Box<dyn std::error::Error>> {
            self.inner.fetch_issue_body(repo, issue_number)
        }

        fn is_issue_closed(
            &self,
            repo: &str,
            issue_number: u32,
        ) -> Result<bool, Box<dyn std::error::Error>> {
            self.inner.is_issue_closed(repo, issue_number)
        }
    }

    struct StopAfterCurrent<'a> {
        closer: RecordingCloser<'a>,
        watch: &'a crate::tui::WatchLog,
    }

    impl AgentRunner for StopAfterCurrent<'_> {
        fn run(
            &self,
            profile: InvocationProfile<'_>,
            prompt: &str,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.watch.request_stop();
            self.closer.run(profile, prompt)
        }
    }

    #[test]
    fn still_open_after_agent_is_failed_not_completed() {
        let github = mock_github(
            graph(&[child(10, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = idle_agent();
        let watch = crate::tui::WatchLog::new();
        let config = tui_config(
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        let error = run_watched(config, runtime(&github, &agent), &watch)
            .expect_err("open issue after agent must fail")
            .to_string();
        assert!(error.contains("still open"));
        assert!(agent.ran.get());
        assert!(events_contain(&watch, |event| {
            matches!(event, WatchEvent::Failed { issue: 10, .. })
        }));
        assert!(!events_contain(&watch, |event| {
            matches!(event, WatchEvent::Completed { issue: 10 })
        }));
    }

    #[test]
    fn github_closed_issue_is_the_only_completed_signal() {
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = recording_closer(&github.closed);
        let watch = crate::tui::WatchLog::new();
        let config = tui_config(
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        run_watched(config, runtime(&github, &agent), &watch).unwrap();
        let events = watch.events();
        assert!(
            events
                .iter()
                .any(|event| { matches!(event, WatchEvent::Hygiene) })
        );
        assert!(
            events
                .iter()
                .any(|event| { matches!(event, WatchEvent::Dispatch { issue: 10 }) })
        );
        assert!(
            events
                .iter()
                .any(|event| { matches!(event, WatchEvent::Running { issue: 10 }) })
        );
        assert!(
            events
                .iter()
                .any(|event| { matches!(event, WatchEvent::Verify { issue: 10 }) })
        );
        assert!(
            events
                .iter()
                .any(|event| { matches!(event, WatchEvent::Completed { issue: 10 }) })
        );
        assert!(
            events
                .iter()
                .any(|event| { matches!(event, WatchEvent::Completed { issue: 11 }) })
        );
        assert!(events.iter().any(|event| matches!(event, WatchEvent::Done)));
        let completed: Vec<u32> = events
            .iter()
            .filter_map(|event| match event {
                WatchEvent::Completed { issue } => Some(*issue),
                _ => None,
            })
            .collect();
        assert_eq!(completed, vec![10, 11]);
    }

    #[test]
    fn stop_after_hygiene_does_not_dispatch() {
        let github = prd_github();
        let agent = idle_agent();
        let watch = crate::tui::WatchLog::new();
        let git = StoppingGit { watch: &watch };
        let config = tui_config(
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        let runtime = Runtime {
            github: &github,
            git: &git,
            agent_runner: &agent,
        };
        run_watched(config, runtime, &watch).unwrap();
        assert!(!agent.ran.get());
        assert!(events_contain(&watch, |event| {
            matches!(event, WatchEvent::Hygiene)
        }));
        assert!(!events_contain(&watch, |event| {
            matches!(event, WatchEvent::Dispatch { .. })
        }));
        assert!(!events_contain(&watch, |event| {
            matches!(event, WatchEvent::Running { .. })
        }));
    }

    #[test]
    fn stop_after_slow_fetch_does_not_dispatch() {
        let watch = crate::tui::WatchLog::new();
        let github = StoppingFetchGithub {
            inner: prd_github(),
            watch: &watch,
        };
        let agent = idle_agent();
        let config = tui_config(
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        run_watched(config, runtime(&github, &agent), &watch).unwrap();
        assert!(!agent.ran.get());
        assert!(events_contain(&watch, |event| {
            matches!(event, WatchEvent::Roster { .. })
        }));
        assert!(!events_contain(&watch, |event| {
            matches!(event, WatchEvent::Dispatch { .. })
        }));
    }

    #[test]
    fn stop_after_current_issue_does_not_dispatch_the_next() {
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let watch = crate::tui::WatchLog::new();
        let agent = StopAfterCurrent {
            closer: recording_closer(&github.closed),
            watch: &watch,
        };
        let config = tui_config(
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        run_watched(config, runtime(&github, &agent), &watch).unwrap();
        assert_eq!(agent.closer.agents.borrow().len(), 1);
        assert!(events_contain(&watch, |event| {
            matches!(event, WatchEvent::Completed { issue: 10 })
        }));
        assert!(!events_contain(&watch, |event| {
            matches!(event, WatchEvent::Dispatch { issue: 11 })
        }));
        assert!(!events_contain(&watch, |event| {
            matches!(event, WatchEvent::Running { issue: 11 })
        }));
        assert!(events_contain(&watch, |event| matches!(
            event,
            WatchEvent::Done
        )));
    }

    #[test]
    fn tui_dry_run_emits_plan_and_does_not_spawn() {
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[(10, "OPEN")])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = idle_agent();
        let watch = crate::tui::WatchLog::new();
        let config = tui_config(
            true,
            defaults(Agent::Pi, Some("issue-model"), Some("high")),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        run_watched(config, runtime(&github, &agent), &watch).unwrap();
        assert!(!agent.ran.get());
        let events = watch.events();
        let roster = events
            .iter()
            .find_map(|event| match event {
                WatchEvent::Roster { planned, blocked } => Some((planned, blocked)),
                _ => None,
            })
            .expect("dry-run should emit the plan");
        assert_eq!(roster.0.len(), 2);
        assert_eq!(roster.0[0].number, 10);
        assert_eq!(roster.0[0].agent, "pi");
        assert_eq!(roster.0[0].model.as_deref(), Some("issue-model"));
        assert_eq!(roster.0[0].effort.as_deref(), Some("high"));
        assert!(roster.1.is_empty());
        assert!(events.iter().any(|event| matches!(event, WatchEvent::Done)));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, WatchEvent::Running { .. }))
        );
    }

    #[test]
    fn tui_dry_run_bad_json_emits_failed_and_returns_parse_error() {
        let github = RawIssuesGithub {
            inner: mock_github("[]", HashMap::from([(42, "Product requirements".into())])),
            payload: "not-json{".to_string(),
        };
        let agent = idle_agent();
        let watch = crate::tui::WatchLog::new();
        let config = tui_config(
            true,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        let error = run_watched(config, runtime(&github, &agent), &watch)
            .expect_err("malformed issue list must fail")
            .to_string();
        assert!(error.contains("failed to parse issue list"), "{error}");
        assert!(
            error.contains("not-json{") || error.contains("expected"),
            "{error}"
        );
        assert!(events_contain(&watch, |event| {
            matches!(event, WatchEvent::Failed { issue: 42, .. })
        }));
        assert!(!events_contain(&watch, |event| matches!(
            event,
            WatchEvent::Done
        )));
        assert!(!agent.ran.get());
    }

    #[test]
    fn tui_dry_run_fetch_error_emits_failed_and_returns_original() {
        let github = FailingFetchGithub {
            inner: mock_github("[]", HashMap::from([(42, "Product requirements".into())])),
        };
        let agent = idle_agent();
        let watch = crate::tui::WatchLog::new();
        let config = tui_config(
            true,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions::default(),
            DirectivePolicy::Replace("test directives"),
        );
        let error = run_watched(config, runtime(&github, &agent), &watch)
            .expect_err("fetch error must fail")
            .to_string();
        assert!(error.contains("failed to fetch issues"), "{error}");
        assert!(error.contains("gh: failed to list issues"), "{error}");
        assert!(events_contain(&watch, |event| {
            matches!(event, WatchEvent::Failed { issue: 42, .. })
        }));
        assert!(!events_contain(&watch, |event| matches!(
            event,
            WatchEvent::Done
        )));
        assert!(!agent.ran.get());
    }

    #[test]
    fn tui_preflight_still_assigns_effort_and_skips_blocked() {
        let github = mock_github(
            graph(&[child(10, 42, &[]), child(11, 42, &[(99, "OPEN")])]),
            HashMap::from([(42, "Product requirements".into())]),
        );
        let agent = MockAgent {
            ran: Cell::new(false),
            received_model: RefCell::new(None),
            received_effort: RefCell::new(None),
            error_on_run: true,
        };
        let watch = crate::tui::WatchLog::new();
        let mut input = Cursor::new(b"5\n\n".as_slice());
        let mut output = Vec::new();
        let mut preflight_io = crate::preflight::Io::new(true, &mut input, &mut output);
        let config = tui_config(
            false,
            defaults(Agent::Pi, None, None),
            RunEphemeralProfileMap::new(),
            PreflightDimensions {
                efforts: true,
                ..PreflightDimensions::default()
            },
            DirectivePolicy::Replace("test directives"),
        );

        let error =
            run_watched_with_preflight(config, runtime(&github, &agent), &mut preflight_io, &watch)
                .expect_err("fake agent stops after recording preflight profile")
                .to_string();
        assert!(error.contains("mock agent stopped"));
        assert_eq!(agent.received_effort.borrow().as_deref(), Some("high"));
        let output = String::from_utf8(output).expect("preflight output is utf-8");
        assert!(output.contains("#10"));
        assert!(!output.contains("#11"));
        assert!(events_contain(&watch, |event| {
            matches!(event, WatchEvent::Dispatch { issue: 10 })
        }));
    }
}
