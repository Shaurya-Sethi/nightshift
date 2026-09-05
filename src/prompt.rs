//! Prompt construction for coding-agent runs.
//!
//! The orchestrator combines PRD context, the selected child issue body, and
//! maintainer directives into one prompt. The parser decides which issue to run,
//! while this module preserves the selected issue details and instructions in a
//! form that can be sent to an agent over stdin.

use std::borrow::Cow;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::agent::Agent;
use crate::github::GithubIssue;

/// Run-wide maintainer-directive source for one Nightshift process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectivePolicy<'a> {
    /// `default_directives_for(resolved_agent)` at invoke time.
    BuiltIn,
    /// File text used as-is; built-ins are not injected.
    Replace(&'a str),
    /// `default_directives_for(resolved_agent)` plus a blank line plus this extra.
    Append(&'a str),
}

/// Whether a picked prompt file is appended to built-ins or used as a full replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    /// `default_directives_for(resolved_agent)` plus a blank line plus the file.
    Append,
    /// File text used as-is; built-ins are not injected.
    Replace,
}

/// Snapshot of a `--pick-prompts` file, loaded before confirm.
///
/// `contents` is already trimmed by [`load_directives`]. The path is not stored;
/// the loop never re-reads the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerIssuePrompt {
    /// Append vs replace for this issue. Run-wide policy is ignored when this
    /// struct is present.
    pub mode: PromptMode,
    /// Trimmed file text snapshotted at preflight.
    pub contents: String,
}

impl PerIssuePrompt {
    fn as_policy(&self) -> DirectivePolicy<'_> {
        match self.mode {
            PromptMode::Append => DirectivePolicy::Append(&self.contents),
            PromptMode::Replace => DirectivePolicy::Replace(&self.contents),
        }
    }
}

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

/// Returns built-in directives appropriate for the selected coding agent.
///
/// Pi has no native subagent support, so its review step launches an
/// independent, read-only Pi process instead. All other agents use the shared
/// default directives.
pub fn default_directives_for(agent: Agent) -> String {
    match agent {
        Agent::Pi => default_directives().replace(
            "6. Self-review using sub-agents.",
            r#"6. Run an independent read-only Pi review from the repository root, then address its findings:

   pi -p --no-session --tools read,bash "Review the current issue implementation and tests. Inspect the working tree and diff against the target base branch. Do not edit files, run formatters, commit, push, change GitHub issues, create a PR, or invoke Pi. Report only actionable findings, ordered by severity, with file:line and concise rationale. If none, say 'No findings.'"

   Review output is input to your self-review. Fix valid findings, then continue."#,
        ),
        _ => default_directives().to_string(),
    }
}

/// Resolves run-wide or per-issue directives for one invocation.
///
/// A per-issue snapshot fully overrides the run-wide policy. [`DirectivePolicy::BuiltIn`]
/// and [`DirectivePolicy::Append`] use [`default_directives_for`] for the resolved
/// agent. [`DirectivePolicy::Replace`] returns the file text unchanged.
pub fn directives_for_invocation<'a>(
    run: DirectivePolicy<'a>,
    per_issue: Option<&'a PerIssuePrompt>,
    agent: Agent,
) -> Cow<'a, str> {
    let policy = match per_issue {
        Some(item) => item.as_policy(),
        None => run,
    };
    match policy {
        DirectivePolicy::BuiltIn => Cow::Owned(default_directives_for(agent)),
        DirectivePolicy::Replace(text) => Cow::Borrowed(text),
        DirectivePolicy::Append(extra) => {
            Cow::Owned(format!("{}\n\n{}", default_directives_for(agent), extra))
        }
    }
}

/// Loads maintainer directives from a file.
///
/// File contents are trimmed and returned. This does not choose replace versus
/// append; callers apply a [`DirectivePolicy`].
///
/// # Errors
///
/// Returns a user-facing error string when the prompt file cannot be read.
pub fn load_directives(prompt_file: &Path) -> Result<String, String> {
    std::fs::read_to_string(prompt_file)
        .map(|prompt| prompt.trim().to_string())
        .map_err(|_| {
            format!(
                "nightshift: failed to read prompt file: {}",
                prompt_file.display()
            )
        })
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
pub fn save_prompt_copy(issue_number: u32, prompt: &str) -> Option<PathBuf> {
    let mut temp_path: PathBuf = std::env::temp_dir();
    temp_path.push(format!("nightshift-prompt-{}.txt", issue_number));

    File::create(&temp_path)
        .and_then(|mut file| file.write_all(prompt.as_bytes()).map(|_| temp_path))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
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

    #[test]
    fn pi_default_directives_run_an_independent_read_only_pi_reviewer() {
        let directives = default_directives_for(Agent::Pi);

        assert!(directives.contains("pi -p --no-session --tools read,bash"));
        assert!(!directives.contains("read,bash,grep,find,ls"));
        assert!(directives.contains("Do not edit files"));
        assert!(directives.contains("If none, say 'No findings.'"));
        assert!(!directives.contains("using sub-agents"));
        assert!(directives.contains("6. Run an independent read-only Pi review"));
        assert!(directives.contains("7. Squash merge using 'gh pr merge'"));
        assert!(directives.contains("git checkout -b issue-{issue_number}"));
        assert!(directives.contains("gh pr create"));
        assert!(directives.contains("gh issue close"));
    }

    #[test]
    fn non_pi_agents_keep_shared_default_directives() {
        for agent in Agent::all()
            .iter()
            .copied()
            .filter(|agent| *agent != Agent::Pi)
        {
            assert_eq!(default_directives_for(agent), default_directives());
        }
    }

    #[test]
    fn built_in_directives_follow_the_resolved_agent() {
        let directives = directives_for_invocation(DirectivePolicy::BuiltIn, None, Agent::Pi);
        assert!(directives.contains("pi -p --no-session --tools read,bash"));
        assert_eq!(
            directives_for_invocation(
                DirectivePolicy::Replace("Custom directives."),
                None,
                Agent::Pi
            ),
            "Custom directives."
        );
    }

    #[test]
    fn append_concatenates_with_pi_built_ins() {
        let directives =
            directives_for_invocation(DirectivePolicy::Append("extra"), None, Agent::Pi);
        assert_eq!(
            directives.as_ref(),
            format!("{}\n\nextra", default_directives_for(Agent::Pi))
        );
        assert!(directives.contains("pi -p --no-session --tools read,bash"));
    }

    #[test]
    fn replace_ignores_built_ins() {
        let directives =
            directives_for_invocation(DirectivePolicy::Replace("Custom only."), None, Agent::Pi);
        assert_eq!(directives.as_ref(), "Custom only.");
        assert!(!directives.contains("git checkout -b"));
    }

    #[test]
    fn blank_per_item_uses_run_wide() {
        assert_eq!(
            directives_for_invocation(DirectivePolicy::Replace("F"), None, Agent::Pi).as_ref(),
            "F"
        );
        assert_eq!(
            directives_for_invocation(DirectivePolicy::Append("F"), None, Agent::Pi).as_ref(),
            format!("{}\n\nF", default_directives_for(Agent::Pi))
        );
        assert_eq!(
            directives_for_invocation(DirectivePolicy::BuiltIn, None, Agent::Pi).as_ref(),
            default_directives_for(Agent::Pi)
        );
    }

    #[test]
    fn per_item_append_ignores_run_wide_replace() {
        let item = PerIssuePrompt {
            mode: PromptMode::Append,
            contents: "row-extra".into(),
        };
        let directives =
            directives_for_invocation(DirectivePolicy::Replace("run-wide"), Some(&item), Agent::Pi);
        assert_eq!(
            directives.as_ref(),
            format!("{}\n\nrow-extra", default_directives_for(Agent::Pi))
        );
        assert!(!directives.contains("run-wide"));
    }
}
