# nightshift

[CI](https://github.com/Shaurya-Sethi/nightshift/actions/workflows/ci.yml)
[License: MIT](LICENSE)
[Rust 2024](https://www.rust-lang.org)

Go to sleep with a backlog and wake up with merged PRs.

nightshift autonomously works through your GitHub issues while you are afk. Point it at a PRD, pick your [favourite coding agent](#supported-agents), and it handles the rest: branch, implement, PR, merge, repeat. It stops when every child issue is done. Inspired by the [Ralph Wiggum](https://ghuntley.com/loop/) loop pattern.

> [!WARNING]
> **nightshift is designed exclusively for the [Matt Pocock skills](https://github.com/mattpocock/skills) workflow**, specifically the `[to-prd](https://github.com/mattpocock/skills)` and `[to-issues](https://github.com/mattpocock/skills)` skills. Your GitHub issues must be structured exactly as those skills produce them: child issues must declare a `## Parent` section referencing the PRD, a `## Blocked by` section listing dependencies, and carry the `ready-for-agent` label. Issues that do not match this structure will be silently skipped.

## Prerequisites

- **Rust + Cargo**: [install via rustup](https://rustup.rs)
- **Git**
- **GitHub CLI (`gh`)**: [install gh](https://cli.github.com), then run `gh auth login`. nightshift uses this to read your issues and find your repository.
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

Each iteration starts from a clean state: nightshift checks out and pulls your base branch, then fetches all open `ready-for-agent` issues from GitHub. It filters down to issues that belong to your PRD (via the `## Parent` section), then picks the lowest-numbered one whose blockers are all closed. If nothing is unblocked, it stops.

For the selected issue, nightshift constructs a unified prompt and pipes it to the coding agent via `stdin`. For details on prompt structures, default instructions, custom directives, and how nightshift manages isolated session context, see the [Context Management & Session Lifecycle Guide](docs/context-management.md).

After the agent exits, nightshift checks that the issue is actually closed on GitHub. If it is, the loop continues from step one. If not, nightshift stops and tells you; the agent may have exited cleanly but left the issue open, which usually means something needs your attention.

Use `--dry-run` to see which issue would be selected and what the prompt looks like, without invoking an agent.

## Keeping Your System Awake

For long-running PRD loops, see [docs/keep-alive.md](docs/keep-alive.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).
