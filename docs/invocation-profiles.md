# Invocation profiles

An **invocation profile** is the agent, model, and reasoning effort used for one issue. `--agent` is always required as the whole-run default. Picker flags are optional and run once, in memory, before the loop starts.

Selections are never written to issues or the repository.

## Whole-run defaults

Pass `--agent` with optional `--model` and `--reasoning-effort`. Every issue uses the fields you supplied. Omitted fields stay at the agent's own persisted default.

```bash
nightshift --prd 12 --agent claude --model claude-opus-5
nightshift --prd 12 --agent claude --pick-agents --pick-models
```

## Per-issue pickers

| Flag | What you pick | Notes |
| ---- | ------------- | ----- |
| `--pick-agents` | One compatible agent per planned issue | Enter keeps `--agent`. The list is not filtered by `PATH`. |
| `--pick-efforts` | One effort key per planned issue | Model stays at `--model` or the agent default. |
| `--pick-models` | A free-string model, plus effort where the agent supports it | Cursor is model-only (effort is encoded in the model slug). |
| `--pick-prompts` | Optional prompt file and append/replace mode | Blank path inherits the run-wide prompt policy. |

`--pick-efforts` and `--pick-models` cannot be combined. `--pick-agents` and `--pick-prompts` stack with either.

Column order is always **agent → model → effort → prompt → mode**. Unsupported knobs for a row are skipped rather than blocking the rest.

The picker covers the **planned set**: every issue nightshift would work through this run, including ones blocked only by another planned issue. `q` or Ctrl-C aborts; a partial selection never starts a run.

Pickers need a TTY. Without one, nightshift fails fast and tells you to use the whole-run flags instead. Cooked preflight still runs before `--tui` takes the terminal.

### Blank fields

In every picker, Enter leaves the field blank:

- Blank **agent** keeps `--agent`.
- Blank **model** or **effort** falls through to the whole-run default, then the agent default.
- Blank **prompt path** inherits `--prompt-file` / `--append-prompt-file` / built-ins.
- Enter on **mode** defaults to append.

### Same-agent defaults

Whole-run `--model` and `--reasoning-effort` apply only while the row still uses `--agent`. If a `--pick-agents` row chooses a different agent, that issue uses the new agent's own defaults unless the row also supplies a model or effort.

### Agent-specific knobs

Without `--pick-agents`, `--pick-efforts` is only valid for `pi`, `copilot`, `claude`, `codex`, and `opencode`. Cursor `--pick-models` is model-only. Antigravity supports neither model nor effort pickers.

With `--pick-agents`, each row skips what that agent cannot do (Cursor: no separate effort; Antigravity: no model or effort).

## Prompts

Built-in directives follow the **resolved** agent for that issue.

- `--prompt-file` replaces built-ins for every issue that does not pick a file.
- `--append-prompt-file` appends to the resolved agent's built-ins for those issues. Mutually exclusive with `--prompt-file`.
- A `--pick-prompts` path fully overrides the run-wide policy (append or replace of that file against the resolved agent's built-ins).

## Dry-run

`--dry-run` does not skip a requested picker. Complete preflight first. Then nightshift prints every planned issue with its resolved agent, model, and effort, the first issue's prompt, and the would-invoke command. No agent process starts.

Without a picker, dry-run resolves rows from whole-run defaults and agent defaults.

`--tui --dry-run` shows that same plan on the Watch Board without spawning. The planned-order / would-invoke / first-prompt preview prints after you dismiss the board. Without `--tui`, dry-run output is unchanged.
