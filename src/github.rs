use serde::Deserialize;
use serde_json::from_slice;
use std::process::Command;

#[derive(Debug, Deserialize, Clone)]
pub struct GithubIssue {
    pub number: u32,
    pub title: String,
    pub body: String,
}

#[derive(Deserialize)]
struct IssueState {
    state: String,
}

pub trait GithubIssues {
    fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>>;
    fn fetch_issues(&self, repo: &str) -> Result<Vec<GithubIssue>, Box<dyn std::error::Error>>;
    fn all_blockers_closed(
        &self,
        repo: &str,
        blockers: &[u32],
    ) -> Result<bool, Box<dyn std::error::Error>>;
    fn is_issue_closed(
        &self,
        repo: &str,
        issue_number: u32,
    ) -> Result<bool, Box<dyn std::error::Error>>;
}

pub struct GhCliAdapter;

impl GithubIssues for GhCliAdapter {
    fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
        if let Some(repo) = repo
            && !repo.is_empty()
        {
            return Ok(repo.to_string());
        }

        let output = Command::new("gh")
            .args([
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "-q",
                ".nameWithOwner",
            ])
            .output()
            .map_err(|e| format!("nightshift: failed to execute gh command: {}", e))?;

        if !output.status.success() {
            let err_msg = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "nightshift: failed to resolve repository: {}",
                err_msg.trim()
            )
            .into());
        }

        let repo = String::from_utf8(output.stdout)
            .map_err(|e| format!("nightshift: invalid UTF-8 output from gh: {}", e))?;
        let repo = repo.trim();
        if repo.is_empty() {
            return Err("nightshift: gh repo view returned empty repository name".into());
        }

        Ok(repo.to_string())
    }

    fn fetch_issues(&self, repo: &str) -> Result<Vec<GithubIssue>, Box<dyn std::error::Error>> {
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

    fn all_blockers_closed(
        &self,
        repo: &str,
        blockers: &[u32],
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
                return Err(Box::new(std::io::Error::other("failed to check blocker")));
            }

            let issue_state: IssueState = from_slice(&output.stdout)?;
            if issue_state.state.to_lowercase() != "closed" {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn is_issue_closed(
        &self,
        repo: &str,
        issue_number: u32,
    ) -> Result<bool, Box<dyn std::error::Error>> {
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
            .output()?;

        if !output.status.success() {
            let err_msg = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "nightshift: failed to check issue #{} state: {}",
                issue_number,
                err_msg.trim()
            )
            .into());
        }

        let issue_state: IssueState = from_slice(&output.stdout)?;
        Ok(issue_state.state.to_lowercase() == "closed")
    }
}
