# Context Management & Agent Session Lifecycle

When `nightshift` prepares to run a coding agent for a selected issue, it constructs a single unified prompt and feeds it to the agent via `stdin`.

## Prompt Skeleton

The constructed prompt always follows this exact format:

```text
You are working on issue #<ISSUE_NUMBER>: "<ISSUE_TITLE>" in <REPOSITORY> repository.

## PRD Context

```markdown
<FULL_BODY_OF_YOUR_PRD_ISSUE>
```

## Task Description & Acceptance Criteria

```markdown
<FULL_BODY_OF_THE_CHILD_ISSUE>
```

## Instructions
<DIRECTIVES>
```

---

## Detailed Components

### 1. Orientation Header

`You are working on issue #<ISSUE_NUMBER>: "<ISSUE_TITLE>" in <REPOSITORY> repository.`
This line sets the immediate target repository, the child issue number, and its title. It orientates the agent on the exact task it is expected to complete and close.

### 2. PRD Context

The full body of the parent PRD issue is injected inside a fenced ````markdown` block. This keeps the PRD headers and formatting isolated so they do not leak or disrupt the surrounding prompt structure. The PRD acts as the overall source of truth for the codebase's feature set.

### 3. Task Description & Acceptance Criteria

The full body of the selected child issue is injected inside a fenced ````markdown` block. This provides the agent with the isolated task definition, acceptance criteria, and specific requirements for this single iteration.

### 4. Instructions (`<DIRECTIVES>`)

The final section contains the step-by-step instructions the agent must follow. By design, `nightshift` supports these modes for instructions:

#### A. Default Directives

If `--prompt-file` and `--append-prompt-file` are omitted, built-in directives follow the **resolved** invocation agent for that issue (the whole-run `--agent`, or a `--pick-agents` row). Non-Pi agents receive these default instructions guiding them through the automated feature branch, test-driven development, PR, and merge loop:

```text
1. Orient yourself in the repository.
2. Create a feature branch: git checkout -b issue-{issue_number}
3. Implement using test-driven development.
4. Run project lint/test checks and test behavior after implementation.
5. Push branch and open a PR using 'gh pr create'.
6. Self-review using sub-agents.
7. Squash merge using 'gh pr merge' and delete branch.
8. Close the issue using 'gh issue close'.
9. Checkout the base branch and pull.
```

A Pi row omits step 6 (`Self-review using sub-agents.`). Pi has no sub-agent support, so that instruction is dropped rather than left in as dead weight. Remaining steps keep their original numbers. All other agents receive the full list above.

#### B. Custom Directives (`--prompt-file`)

You can provide a custom instructions file via `--prompt-file <path>`.

> [!IMPORTANT]
> A custom prompt file **completely overrides** the default directives for **every issue that does not pick a file**, regardless of per-issue agent. If you provide a custom prompt file, you are responsible for instructing the agent on how to branch, test, open a PR, and close the issue, or whichever workflow you prefer the agent to execute.

#### C. Appended Directives (`--append-prompt-file`)

`--append-prompt-file <path>` keeps the resolved agent's built-in directives, then adds a blank line and the file contents. This is mutually exclusive with `--prompt-file` at the CLI. Append uses Pi built-ins (no sub-agent step) when the resolved agent is Pi. Like B, this applies for every issue that does not pick a file.

#### Per-issue override (`--pick-prompts`)

TTY-only. For each planned child, type an optional path and choose append or replace. Enter on the path inherits the run-wide policy (A, B, or C). A supplied path fully overrides that policy: path + append uses that issue's resolved-agent built-ins plus a blank line plus the file; path + replace uses the file only. Files are read once at preflight (snapshot); mid-run edits are ignored.

| Run-wide | Per-item row | Directives for that issue |
| --- | --- | --- |
| none | blank | agent built-ins |
| `--prompt-file F` | blank | contents of F |
| `--append-prompt-file F` | blank | agent built-ins + blank line + contents of F |
| any | path + append | agent built-ins + blank line + that file (run-wide ignored) |
| any | path + replace | that file only (run-wide ignored) |

---

## Tips for Writing Custom Directives

Since `nightshift` automatically injects the PRD context, child issue body, and orientation details beforehand, you do not need to duplicate them.

- **Clarify the exact loop**: Explicitly instruct the agent on the exact Git workflow (e.g., `git checkout -b issue-{issue_number}`), the commands to run for tests and lints, how to create/merge PRs, and the maximum review/retry attempts allowed for sub-agents to avoid infinite loops.
- **Optimize for Prompt Caching**: Keep static guidelines, coding conventions, and stack definitions at the top of your custom file, and place dynamic, highly variable, or high-entropy information at the very end to maximize cache hits and reduce API costs.
- **Provide skill and mcp usage guidance**: Direct the agent on what skills and specific MCP tools it should use.
- **Leverage Caveman mode**: I think you should consider installing and instructing the agent to use the [caveman](https://github.com/juliusbrussee/caveman) skill. This drops filler words and unnecessary conversational overhead, preserving full technical accuracy which mitigates context bloat and cuts output tokens by ~75%.

---

### Key Session Principles

1. **Fresh Context Window**: Each issue is worked on by the agent in a brand-new process execution. The agent has **no memory** of previous runs, earlier issues, or past conversations.
2. **State via Git, Not Conversation Memory**: The agent inherits the results of previous steps exclusively through the filesystem and the Git repository state (which `nightshift` automatically syncs and pulls from the base branch before starting a new run).
3. **Session Termination**: Once the agent process exits (either because it completed its instructions or encountered an error), the session terminates and a new one starts for a valid new issue.
