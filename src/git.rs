use std::path::{Path, PathBuf};
use std::process::Command;

pub trait GitOps {
    fn base_branch_exists(&self, base_branch: &str) -> bool;
    fn ensure_hygiene(&self, base_branch: &str) -> Result<(), Box<dyn std::error::Error>>;
}

pub struct GitCliAdapter {
    workdir: PathBuf,
}

impl GitCliAdapter {
    pub fn for_repo(repo: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let workdir = resolve_workspace(repo)?;
        Ok(Self { workdir })
    }

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

/// Returns the git worktree root for `repo` (`owner/name`) when cwd is inside that clone.
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

fn workspace_matches_repo(toplevel: &Path, repo: &str) -> Result<bool, Box<dyn std::error::Error>> {
    for remote in git_remotes(toplevel)? {
        let url = git_remote_url(toplevel, &remote)?;
        if origin_matches_repo(&url, repo) {
            return Ok(true);
        }
    }
    Ok(false)
}

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

fn origin_matches_repo(origin: &str, repo: &str) -> bool {
    parse_github_repo_slug(origin).is_some_and(|slug| slug.eq_ignore_ascii_case(repo))
}

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

fn slug_from_path(path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
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
