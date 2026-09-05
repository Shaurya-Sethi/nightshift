//! GitHub issue access for the nightshift loop.
//!
//! This module defines the issue shape consumed by [`crate::orchestrator`] and
//! the [`crate::github::GithubIssues`] adapter trait used in tests and production. The
//! production adapter shells out to `gh issue list` for the open `ready-for-agent`
//! graph, `gh issue view` for the PRD body and completion checks, and `gh repo view`
//! when the repository slug is not passed explicitly.

use serde::Deserialize;
use serde_json::from_slice;
use std::process::Command;

/// Open GitHub issue data needed by the prompt renderer.
#[derive(Debug)]
pub struct GithubIssue {
    /// GitHub issue number shown in logs and included in the agent prompt.
    pub number: u32,
    /// GitHub issue title shown in logs and included in the agent prompt.
    pub title: String,
    /// Markdown issue body included in the agent prompt.
    pub body: String,
}

#[derive(Deserialize)]
struct IssueState {
    state: String,
}

#[derive(Deserialize)]
struct IssueBody {
    body: Option<String>,
}

/// GitHub operations required by the orchestrator.
///
/// Implement this trait for tests or alternate GitHub clients when the loop
/// should not shell out to the GitHub CLI.
///
/// # Examples
///
/// ```rust
/// # use nightshift::github::GithubIssues;
/// # struct EmptyGithub;
/// # impl GithubIssues for EmptyGithub {
/// #     fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
/// #         Ok(repo.unwrap_or("owner/repo").to_string())
/// #     }
/// #     fn fetch_issues(&self, _repo: &str) -> Result<String, Box<dyn std::error::Error>> {
/// #         Ok("[]".to_string())
/// #     }
/// #     fn fetch_issue_body(&self, _repo: &str, _issue_number: u32) -> Result<String, Box<dyn std::error::Error>> {
/// #         Ok(String::new())
/// #     }
/// #     fn is_issue_closed(&self, _repo: &str, _issue_number: u32) -> Result<bool, Box<dyn std::error::Error>> {
/// #         Ok(false)
/// #     }
/// # }
/// let github = EmptyGithub;
/// assert_eq!(github.resolve_repo(Some("owner/repo"))?, "owner/repo");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait GithubIssues {
    /// Resolves the repository slug to use for GitHub API calls.
    ///
    /// Implementations should return the provided slug when present, or discover
    /// the current repository when possible.
    fn resolve_repo(&self, repo: Option<&str>) -> Result<String, Box<dyn std::error::Error>>;
    /// Fetches the raw `gh issue list` JSON for open `ready-for-agent` issues.
    ///
    /// The payload includes native `parent` and `blockedBy` fields used by
    /// [`crate::parser`].
    fn fetch_issues(&self, repo: &str) -> Result<String, Box<dyn std::error::Error>>;
    /// Returns the markdown body of a single issue, used for PRD context.
    ///
    /// # Errors
    ///
    /// Returns an error when the issue cannot be read from GitHub.
    fn fetch_issue_body(
        &self,
        repo: &str,
        issue_number: u32,
    ) -> Result<String, Box<dyn std::error::Error>>;
    /// Returns whether a selected child issue is closed after the agent exits.
    fn is_issue_closed(
        &self,
        repo: &str,
        issue_number: u32,
    ) -> Result<bool, Box<dyn std::error::Error>>;
}

/// [`GithubIssues`] implementation backed by the `gh` command-line tool.
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

    fn fetch_issues(&self, repo: &str) -> Result<String, Box<dyn std::error::Error>> {
        let output = Command::new("gh")
            .args([
                "issue",
                "list",
                "-R",
                repo,
                "--json",
                "number,title,body,parent,blockedBy",
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

        let json = String::from_utf8(output.stdout)
            .map_err(|e| format!("nightshift: invalid UTF-8 output from gh: {}", e))?;
        Ok(json)
    }

    fn fetch_issue_body(
        &self,
        repo: &str,
        issue_number: u32,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let output = Command::new("gh")
            .args([
                "issue",
                "view",
                &issue_number.to_string(),
                "-R",
                repo,
                "--json",
                "body",
            ])
            .output()?;

        if !output.status.success() {
            let err_msg = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "nightshift: failed to fetch issue #{}: {}",
                issue_number,
                err_msg.trim()
            )
            .into());
        }

        let issue_body: IssueBody = from_slice(&output.stdout)?;
        Ok(issue_body.body.unwrap_or_default())
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
