# nightshift

[![CI](https://img.shields.io/github/actions/workflow/status/Shaurya-Sethi/nightshift/ci.yml?branch=main&logo=github)](https://github.com/Shaurya-Sethi/nightshift/actions/workflows/ci.yml) [![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE) [![Rust 2024](https://img.shields.io/badge/rust-2024-%23b7410e?logo=rust)](https://www.rust-lang.org)

Go to sleep with a backlog and wake up with merged PRs.

nightshift autonomously works through your GitHub issues while you are afk. Point it at a PRD, pick your [favourite coding agent](#supported-agents), and it handles the rest: branch, implement, PR, merge, repeat. It stops when every child issue is done. Inspired by the [Ralph Wiggum](https://ghuntley.com/loop/) loop pattern.

> [!WARNING]
> **nightshift selects work from native GitHub relationships, not issue-body text.** Child issues must be sub-issues of the PRD (`gh issue create --parent`), declare dependencies with `--blocked-by`, and carry the `ready-for-agent` label. Bodies are not parsed for membership or ordering. The bundled [skills](#skills) produce exactly this shape.

## Prerequisites

- **Rust + Cargo**: [install via rustup](https://rustup.rs)
- **Git**
- **GitHub CLI (`gh`) >= 2.94.0**: [install gh](https://cli.github.com), then run `gh auth login`. nightshift uses this to read native issue relationships (`parent`, `blockedBy`) and find your repository.
- **A coding agent**: install and sign in to whichever agent you pass to `--agent`. You only need one. See [Supported Agents](#supported-agents).

## Installation

```bash
cargo install --git https://github.com/Shaurya-Sethi/nightshift
```

This places the `nightshift` binary in `~/.cargo/bin`, which is on your `$PATH` after a standard Rust install.

## Usage

```bash
nightshift --prd 12 --agent claude --model claude-sonnet-4-6
```


| Flag            | Required | Default                   | Description                                                            |
| --------------- | -------- | ------------------------- | ---------------------------------------------------------------------- |
| `--prd`         | yes      | n/a                       | The PRD issue number to work through                                   |
| `--agent`       | yes      | n/a                       | Which agent to use: `claude`, `codex`, `antigravity`, `cursor`, `pi`   |
| `--model`       |          | agent's persisted default | Explicit model for agents that support non-interactive model selection |
| `--issue`       |          | `0`                       | Skip issues below this number (useful when resuming)                   |
| `--repo`        |          | detected from `gh`        | Repository as `owner/name`                                             |
| `--base-branch` |          | `main`                    | Branch to sync to before each issue                                    |
| `--prompt-file` |          | built-in guidelines       | File with extra instructions for your agent                            |
| `--dry-run`     |          | `false`                   | Show what would run, without starting an agent                         |


## Supported Agents

nightshift hands your agent a single prompt per issue. These agents work out of the box:


| `--agent` value | Command run | `--model` support | Project                                                                                      |
| --------------- | ----------- | ----------------- | -------------------------------------------------------------------------------------------- |
| `claude`        | `claude`    | yes               | [Anthropic Claude Code](https://docs.anthropic.com/en/docs/claude-code)                      |
| `codex`         | `codex`     | yes               | [OpenAI Codex CLI](https://github.com/openai/codex)                                          |
| `antigravity`   | `agy`       | no                | [Google Antigravity CLI](https://antigravity.google/blog/introducing-google-antigravity-cli) |
| `cursor`        | `agent`     | yes               | [Cursor](https://cursor.com/cli)                                                             |
| `pi`            | `pi`        | yes               | [Pi](https://pi.dev/)                                                                        |


When `--model` is omitted, nightshift lets the selected agent use its persisted default model. When `--model` is provided, nightshift passes it through unchanged for agents with a documented non-interactive model flag. If an agent does not support that flag, nightshift fails fast and tells you to retry without `--model`.

To add support for a new agent, see [CONTRIBUTING.md](CONTRIBUTING.md).

## How It Works

nightshift works through your PRD one issue at a time, stopping when there is nothing left to pick up.

Each iteration starts from a clean state: nightshift checks out and pulls your base branch, then fetches all open `ready-for-agent` issues from GitHub. It keeps issues whose native parent is your PRD, then picks the lowest-numbered one whose `blockedBy` issues are all closed. If nothing is unblocked, it stops.

For the selected issue, nightshift constructs a unified prompt and pipes it to the coding agent via `stdin`. For details on prompt structures, default instructions, custom directives, and how nightshift manages isolated session context, see the [Context Management & Session Lifecycle Guide](docs/context-management.md).

> [!NOTE]
> **Terminal Output Behavior**: While an agent is running, your terminal shows only `nightshift` orchestrator output (issue blocks, git hygiene, completion footers). Agent `stdout` and `stderr` are discarded; use the agent's own UI or history for session detail. On failure, `nightshift` reports the process exit status (and a `--model` retry hint when applicable), not agent log text.

After the agent exits, nightshift checks that the issue is actually closed on GitHub. If it is, the loop continues from step one. If not, nightshift stops and tells you; the agent may have exited cleanly but left the issue open, which usually means something needs your attention.

Use `--dry-run` to see which issue would be selected and what the prompt looks like, without invoking an agent.

## Skills

Writing sub-issues and blocked-by links by hand is tedious, so nightshift ships two agent skills under [`skills/`](skills/) that do it for you:

- **`to-nightshift-prd`**: turns the current conversation into a PRD (problem, solution, user stories, implementation and testing decisions) and publishes it as a GitHub issue.
- **`to-nightshift-issues`**: breaks a PRD into tracer-bullet vertical slices and publishes each as a sub-issue of the PRD via `gh issue create --parent`, with `--blocked-by` links and the `ready-for-agent` label (or `ready-for-human` for slices that need a person). It creates the labels if they are missing.

The intended flow is `to-nightshift-prd` → `to-nightshift-issues` → `nightshift --dry-run` to check the order → `nightshift`. Both skills need an authenticated `gh` (see [Prerequisites](#prerequisites)) and publish to the repository `gh` detects from your working directory.

### Installing

Copy the skill folders into wherever your agent discovers skills, in the repository you want issues for:

```bash
cp -r /path/to/nightshift/skills/* .claude/skills/   # Claude Code (project)
cp -r /path/to/nightshift/skills/* .agents/skills/   # Codex and other agents
cp -r /path/to/nightshift/skills/* ~/.claude/skills/ # Claude Code (all projects)
```

Then invoke them by name in your agent, e.g. `/to-nightshift-prd` in Claude Code.

### Running the skills non-interactively

The skills also work without a chat session. With [Pi](https://pi.dev/) for example, load the skill explicitly and tell the agent to proceed without asking questions:

```bash
pi -p --no-session --skill /path/to/nightshift/skills/to-nightshift-prd \
  --append-system-prompt "Non-interactive run: treat every 'check with the user' step as approved." \
  "Use the to-nightshift-prd skill. Feature: <describe the feature>. Publish the PRD."

pi -p --no-session --skill /path/to/nightshift/skills/to-nightshift-issues \
  --append-system-prompt "Non-interactive run: treat the breakdown as approved and publish." \
  "Use the to-nightshift-issues skill to break down PRD #<n>."
```

## Keeping Your System Awake

For long-running PRD loops, see [docs/keep-alive.md](docs/keep-alive.md).

## Contributing

`nightshift` is under active development, and i'd love your help! Whether you are fixing a bug, adding support for a new coding agent, or proposing new features, all contributions are extremely welcome.

### How to get involved:

- **File an Issue:** If you find a bug, encounter unexpected behavior, or have an idea for a new feature - [Open an issue](https://github.com/Shaurya-Sethi/nightshift/issues) to start a discussion.
- **Add a New Agent:** Want to use `nightshift` with another coding assistant? Follow the step-by-step agent integration guide in [CONTRIBUTING.md](CONTRIBUTING.md#tier-1-adding-a-new-agent).
- **Propose Other Changes:** For parser, orchestrator, or CLI changes, please open an issue first so we can align on the design. Check out [CONTRIBUTING.md](CONTRIBUTING.md#tier-2-everything-else) for codebase style and testing guidelines.

