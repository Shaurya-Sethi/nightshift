use crate::agent::{Agent, AgentRunner};
use crate::git::GitOps;
use crate::github::{GithubIssue, GithubIssues};
use crate::parser::{extract_blockers, extract_parent_prd};
use crate::prompt::{render_issue_prompt, save_prompt_copy};

pub struct WorkflowConfig<'a> {
    pub prd: u32,
    pub issue: u32,
    pub repo: &'a str,
    pub base_branch: &'a str,
    pub dry_run: bool,
    pub agent: Agent,
    pub directives: &'a str,
}

pub struct Runtime<'a> {
    pub github: &'a dyn GithubIssues,
    pub git: &'a dyn GitOps,
    pub agent_runner: &'a dyn AgentRunner,
}

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

        let mut candidates: Vec<GithubIssue> = Vec::new();
        let mut prd_has_open_issues = false;
        for issue in &issues {
            if let Some(parent) = extract_parent_prd(&issue.body)
                && parent == config.prd
            {
                prd_has_open_issues = true;
                if issue.number >= config.issue {
                    candidates.push(issue.clone());
                }
            }
        }

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

        candidates.sort_by_key(|issue| issue.number);

        let mut next_issue_to_solve: Option<GithubIssue> = None;
        for issue in candidates {
            let blockers = extract_blockers(&issue.body);
            match runtime.github.all_blockers_closed(config.repo, &blockers) {
                Ok(true) => {
                    next_issue_to_solve = Some(issue);
                    break;
                }
                Ok(false) => {
                    continue;
                }
                Err(err) => {
                    return Err(format!(
                        "nightshift: API or connection error while checking blockers: {}",
                        err
                    )
                    .into());
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

        let final_prompt =
            render_issue_prompt(config.repo, &prd_body, &selected_issue, config.directives);

        save_prompt_copy(selected_issue.number, &final_prompt);

        if config.dry_run {
            println!(
                "nightshift: [DRY-RUN] Selected issue: #{} - {}",
                selected_issue.number, selected_issue.title
            );
            println!(
                "nightshift: [DRY-RUN] Would invoke agent: {}",
                config.agent.get_command().0
            );
            println!("nightshift: [DRY-RUN] Prompt preview: \n{}", final_prompt);
            return Ok(());
        }

        runtime.agent_runner.run(config.agent, &final_prompt)?;

        if !runtime
            .github
            .is_issue_closed(config.repo, selected_issue.number)
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
