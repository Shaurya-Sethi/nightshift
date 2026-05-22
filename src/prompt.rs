//! Prompt construction for coding-agent runs.
//!
//! The orchestrator combines PRD context, the selected child issue body, and
//! maintainer directives into one prompt. The parser decides which issue to run,
//! while this module preserves the selected issue details and instructions in a
//! form that can be sent to an agent over stdin.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::github::GithubIssue;

/// Returns the built-in maintainer directives appended to every prompt.
///
/// These defaults describe the expected agent workflow: branch from the base,
/// implement with tests, open a PR, self-review, merge, close the issue, and
/// return to the base branch.
pub fn default_directives() -> &'static str {
    r#"1. Orient yourself in the repository.
2. Create a feature branch: git checkout -b issue-{issue_number}
3. Implement using test-driven development.
4. Run project lint/test checks and test behavior after implementation.
5. Push branch and open a PR using 'gh pr create'.
6. Self-review using sub-agents.
7. Squash merge using 'gh pr merge' and delete branch.
8. Close the issue using 'gh issue close'.
9. Checkout the base branch and pull."#
}

/// Loads maintainer directives from a file or falls back to [`default_directives`].
///
/// File contents are trimmed before being appended to the rendered issue prompt.
///
/// # Errors
///
/// Returns a user-facing error string when a requested prompt file cannot be
/// read.
pub fn load_directives(prompt_file: Option<&Path>) -> Result<String, String> {
    match prompt_file {
        Some(prompt_file) => std::fs::read_to_string(prompt_file)
            .map(|prompt| prompt.trim().to_string())
            .map_err(|_| {
                format!(
                    "nightshift: failed to read prompt file: {}",
                    prompt_file.display()
                )
            }),
        None => Ok(default_directives().to_string()),
    }
}

/// Renders the prompt sent to the coding agent for one selected issue.
///
/// The prompt includes repository context, the PRD body, the selected child
/// issue body, and the maintainer directives. It does not parse issue sections;
/// candidate selection has already happened in [`crate::orchestrator`].
///
/// # Examples
///
/// ```rust
/// # use nightshift::github::GithubIssue;
/// # use nightshift::prompt::render_issue_prompt;
/// let issue = GithubIssue {
///     number: 7,
///     title: "Add endpoint".into(),
///     body: "Acceptance criteria".into(),
/// };
///
/// let prompt = render_issue_prompt("owner/repo", "PRD body", &issue, "Run tests.");
/// assert!(prompt.contains("issue #7"));
/// assert!(prompt.contains("PRD body"));
/// ```
pub fn render_issue_prompt(
    repo: &str,
    prd_body: &str,
    issue: &GithubIssue,
    directives: &str,
) -> String {
    format!(
        "You are working on issue #{num}: \"{title}\" in {repo_name} repository.\n\n\
         ## PRD Context\n\n\
         ```markdown\n\
         {prd_body}\n\
         ```\n\n\
         ## Task Description & Acceptance Criteria\n\n\
         ```markdown\n\
         {issue_body}\n\
         ```\n\n\
         ## Instructions\n\
         {directives}",
        num = issue.number,
        title = issue.title,
        repo_name = repo,
        prd_body = prd_body,
        issue_body = issue.body,
        directives = directives
    )
}

/// Writes a temporary copy of the prompt for debugging an agent run.
///
/// Failures are intentionally ignored because this is diagnostic output and
/// should not stop the main workflow.
pub fn save_prompt_copy(issue_number: u32, prompt: &str) {
    let mut temp_path: PathBuf = std::env::temp_dir();
    temp_path.push(format!("nightshift-prompt-{}.txt", issue_number));

    if let Ok(mut file) = File::create(&temp_path) {
        let _ = file.write_all(prompt.as_bytes());
        println!("nightshift: saved prompt copy to {}", temp_path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GithubIssue;

    #[test]
    fn render_issue_prompt_includes_prd_task_and_directives() {
        let issue = GithubIssue {
            number: 7,
            title: "Add endpoint".into(),
            body: "Acceptance: returns 200".into(),
        };
        let prompt = render_issue_prompt(
            "foobar/repo",
            "PRD acceptance criteria",
            &issue,
            "1. Write tests\n2. Open PR",
        );
        assert!(prompt.contains("issue #7"));
        assert!(prompt.contains("Add endpoint"));
        assert!(prompt.contains("PRD acceptance criteria"));
        assert!(prompt.contains("Acceptance: returns 200"));
        assert!(prompt.contains("1. Write tests"));
        assert!(prompt.contains("foobar/repo"));
    }
}
