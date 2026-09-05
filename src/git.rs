//! Git workspace resolution and hygiene for agent runs.
//!
//! Nightshift must run inside the local clone for the selected GitHub
//! repository before it can ask agents to make changes. This module verifies the
//! current worktree belongs to the requested `owner/name` slug and performs the
//! base-branch checkout and pull that happen before each orchestrator iteration.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Git operations required by the orchestrator.
///
/// Implement this trait in tests to avoid modifying a real checkout.
///
/// # Examples
///
/// ```rust
/// # use nightshift::git::GitOps;
/// # struct CleanGit;
/// # impl GitOps for CleanGit {
/// #     fn base_branch_exists(&self, _base_branch: &str) -> bool { true }
/// #     fn ensure_hygiene(&self, _base_branch: &str) -> Result<(), Box<dyn std::error::Error>> {
/// #         Ok(())
/// #     }
/// # }
/// let git = CleanGit;
/// assert!(git.base_branch_exists("main"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait GitOps {
    /// Returns whether `base_branch` exists locally or as `origin/base_branch`.
    fn base_branch_exists(&self, base_branch: &str) -> bool;
    /// Checks out `base_branch` and pulls latest changes before an agent run.
    fn ensure_hygiene(&self, base_branch: &str) -> Result<(), Box<dyn std::error::Error>>;
}

/// [`GitOps`] implementation backed by the `git` command-line tool.
pub struct GitCliAdapter {
    workdir: PathBuf,
}

impl GitCliAdapter {
    /// Creates a git adapter for the local clone that matches `repo`.
    ///
    /// `repo` must be the GitHub slug in `owner/name` form. The current working
    /// directory must be inside a git worktree whose remotes point at that slug.
    pub fn for_repo(repo: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let workdir = resolve_workspace(repo)?;
        Ok(Self { workdir })
    }

    /// Returns the verified worktree root used for git commands.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    fn git(&self) -> Command {
        let mut command = Command::new("git");
        command.current_dir(&self.workdir);
        command
    }
}

impl GitOps for GitCliAdapter {
    fn base_branch_exists(&self, base_branch: &str) -> bool {
        let local = format!("refs/heads/{}", base_branch);
        let remote = format!("refs/remotes/origin/{}", base_branch);

        for reference in [&local, &remote] {
            if self
                .git()
                .args(["show-ref", "--verify", "--quiet", reference])
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
            {
                return true;
            }
        }

        false
    }

    fn ensure_hygiene(&self, base_branch: &str) -> Result<(), Box<dyn std::error::Error>> {
        println!(
            "nightshift: enforcing git hygiene in {} (checking out and pulling {})...",
            self.workdir.display(),
            base_branch
        );

        let checkout_status = self.git().args(["checkout", base_branch]).status()?;

        if !checkout_status.success() {
            return Err(format!("failed to checkout base branch '{}'", base_branch).into());
        }

        let pull_status = self.git().args(["pull"]).status()?;

        if !pull_status.success() {
            return Err("failed to pull latest changes from remote".into());
        }

        Ok(())
    }
}

/// Returns the git worktree root for `repo` when cwd is inside that clone.
///
/// The repository comparison is based on configured git remotes, not directory
/// names. Remote URLs are normalized with `parse_github_repo_slug` before
/// comparison so SSH, HTTPS, credential-bearing, and trailing-slash GitHub URLs
/// match the same `owner/name` slug. Extra path segments after `owner/name` are
/// ignored.
pub fn resolve_workspace(repo: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let start = std::env::current_dir()?;
    let Some(toplevel) = git_toplevel(&start) else {
        return Err(format!(
            "nightshift: not inside a git repository; cd into a local clone of {repo} and run nightshift again"
        )
        .into());
    };

    if workspace_matches_repo(&toplevel, repo)? {
        return Ok(toplevel);
    }

    let origin = git_remote_url(&toplevel, "origin").unwrap_or_default();
    Err(format!(
        "nightshift: current directory is not a clone of {repo} (origin is {origin}); \
         cd into a local clone of {repo} and run nightshift again"
    )
    .into())
}

/// Finds the git top-level directory for `start`.
fn git_toplevel(start: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let toplevel = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(toplevel.trim());
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

/// Checks all configured remotes for a GitHub slug matching `repo`.
fn workspace_matches_repo(toplevel: &Path, repo: &str) -> Result<bool, Box<dyn std::error::Error>> {
    for remote in git_remotes(toplevel)? {
        let url = git_remote_url(toplevel, &remote)?;
        if origin_matches_repo(&url, repo) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Lists git remote names configured in the worktree.
fn git_remotes(toplevel: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(["remote"])
        .current_dir(toplevel)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "nightshift: failed to list git remotes in {}: {}",
            toplevel.display(),
            stderr.trim()
        )
        .into());
    }

    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect())
}

/// Reads a single remote URL from the worktree.
fn git_remote_url(toplevel: &Path, remote: &str) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(["remote", "get-url", remote])
        .current_dir(toplevel)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "nightshift: failed to read git remote '{remote}' in {}: {}",
            toplevel.display(),
            stderr.trim()
        )
        .into());
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Compares a remote URL with an expected GitHub `owner/name` slug.
fn origin_matches_repo(origin: &str, repo: &str) -> bool {
    parse_github_repo_slug(origin).is_some_and(|slug| slug.eq_ignore_ascii_case(repo))
}

/// Parses common GitHub remote URL forms into an `owner/name` slug.
///
/// Supported forms include `git@github.com:owner/repo.git`,
/// `ssh://git@github.com/owner/repo.git`, `https://github.com/owner/repo`,
/// `http://github.com/owner/repo.git`, URLs with a trailing slash, and URLs with
/// credentials before `github.com`. Extra path segments after `owner/name` are
/// ignored. Non-GitHub remotes return [`None`].
fn parse_github_repo_slug(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches(".git");

    if let Some(rest) = remote.strip_prefix("git@github.com:") {
        return slug_from_path(rest);
    }

    if let Some(rest) = remote.strip_prefix("ssh://git@github.com/") {
        return slug_from_path(rest);
    }

    if let Some(rest) = remote.strip_prefix("https://github.com/") {
        return slug_from_path(rest);
    }

    if let Some(rest) = remote.strip_prefix("http://github.com/") {
        return slug_from_path(rest);
    }

    let remote_lower = remote.to_lowercase();
    if let Some(idx) = remote_lower.find("github.com/") {
        let path = &remote[idx + "github.com/".len()..];
        return slug_from_path(path);
    }
    if let Some(idx) = remote_lower.find("github.com:") {
        let path = &remote[idx + "github.com:".len()..];
        return slug_from_path(path);
    }

    None
}

/// Builds an `owner/name` slug from the path portion of a GitHub remote URL.
fn slug_from_path(path: &str) -> Option<String> {
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    if repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::{origin_matches_repo, parse_github_repo_slug};

    #[test]
    fn parse_ssh_git_url() {
        assert_eq!(
            parse_github_repo_slug("git@github.com:foobar/nightshift.git"),
            Some("foobar/nightshift".to_string())
        );
    }

    #[test]
    fn parse_https_url() {
        assert_eq!(
            parse_github_repo_slug("https://github.com/foobar/nightshift"),
            Some("foobar/nightshift".to_string())
        );
    }

    #[test]
    fn parse_https_url_with_trailing_slash() {
        assert_eq!(
            parse_github_repo_slug("https://github.com/foobar/nightshift/"),
            Some("foobar/nightshift".to_string())
        );
    }

    #[test]
    fn parse_https_url_ignores_extra_path_segments() {
        assert_eq!(
            parse_github_repo_slug("https://github.com/foobar/nightshift/pull/1"),
            Some("foobar/nightshift".to_string())
        );
    }

    #[test]
    fn parse_ssh_scheme_url() {
        assert_eq!(
            parse_github_repo_slug("ssh://git@github.com/foobar/nightshift.git"),
            Some("foobar/nightshift".to_string())
        );
    }

    #[test]
    fn parse_http_url() {
        assert_eq!(
            parse_github_repo_slug("http://github.com/foobar/nightshift.git"),
            Some("foobar/nightshift".to_string())
        );
    }

    #[test]
    fn parse_embedded_github_com_path() {
        assert_eq!(
            parse_github_repo_slug("https://oauth@github.com/foobar/nightshift.git"),
            Some("foobar/nightshift".to_string())
        );
    }

    #[test]
    fn parse_embedded_github_com_colon_ssh_style() {
        assert_eq!(
            parse_github_repo_slug("git@github.com:foobar/nightshift"),
            Some("foobar/nightshift".to_string())
        );
    }

    #[test]
    fn parse_non_github_remote_returns_none() {
        assert_eq!(
            parse_github_repo_slug("git@gitlab.com:foobar/nightshift.git"),
            None
        );
    }

    #[test]
    fn origin_matches_repo_case_insensitive() {
        assert!(origin_matches_repo(
            "https://github.com/Foobar/NightShift.git",
            "foobar/nightshift"
        ));
        assert!(!origin_matches_repo(
            "https://github.com/foobar/other.git",
            "foobar/nightshift"
        ));
    }
}
