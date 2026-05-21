use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::github::GithubIssue;

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

pub fn render_issue_prompt(
    repo: &str,
    prd_body: &str,
    issue: &GithubIssue,
    directives: &str,
) -> String {
    format!(
        "You are working on issue #{num}: \"{title}\" in {repo_name} repository.

        ## PRD Context
        {prd_body}

        ## Task Description & Acceptance Criteria
        {issue_body}

        ## Instructions
        {directives}",
        num = issue.number,
        title = issue.title,
        repo_name = repo,
        prd_body = prd_body,
        issue_body = issue.body,
        directives = directives
    )
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

pub fn save_prompt_copy(issue_number: u32, prompt: &str) {
    let mut temp_path: PathBuf = std::env::temp_dir();
    temp_path.push(format!("nightshift-prompt-{}.txt", issue_number));

    if let Ok(mut file) = File::create(&temp_path) {
        let _ = file.write_all(prompt.as_bytes());
        println!("nightshift: saved prompt copy to {}", temp_path.display());
    }
}
