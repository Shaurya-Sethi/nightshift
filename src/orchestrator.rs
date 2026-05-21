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
            pick_next_unblocked_issue(&candidates, runtime.github, config.repo).map_err(
                |err| {
                    format!(
                        "nightshift: API or connection error while checking blockers: {}",
                        err
                    )
                },
            )?;

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
    use std::cell::Cell;
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

        fn fetch_issues(&self, _repo: &str) -> Result<Vec<GithubIssue>, Box<dyn std::error::Error>> {
            Ok(self.issues.clone())
        }

        fn all_blockers_closed(
            &self,
            _repo: &str,
            blockers: &[u32],
        ) -> Result<bool, Box<dyn std::error::Error>> {
            Ok(blockers
                .iter()
                .all(|blocker| self.closed.contains(blocker)))
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
    }

    impl AgentRunner for MockAgent {
        fn run(&self, _agent: Agent, _prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
            self.ran.set(true);
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
        assert!(pick_next_unblocked_issue(&[blocked.clone()], &github, "foobar/repo")
            .unwrap()
            .is_none());
        let picked = pick_next_unblocked_issue(&[blocked.clone(), ready.clone()], &github, "foobar/repo")
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
        let issues = vec![
            prd,
            child(10, 42, &[]),
            child(11, 42, &[]),
        ];
        let github = MockGithub {
            issues,
            closed: HashSet::new(),
        };
        let agent = MockAgent {
            ran: Cell::new(false),
        };
        let config = WorkflowConfig {
            prd: 42,
            issue: 1,
            repo: "foobar/repo",
            base_branch: "main",
            dry_run: true,
            agent: Agent::Cursor,
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
