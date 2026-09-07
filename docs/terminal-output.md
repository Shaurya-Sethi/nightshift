# Terminal output

nightshift never prints agent logs. It reports orchestration: which issue is running, git hygiene, and whether the issue closed.

## Default (cooked)

Without `--tui`, the terminal shows only nightshift output: issue blocks, git checkout/pull, and completion footers. Agent `stdout` and `stderr` are discarded. Use the agent's own UI or history for session detail.

## Watch Board (`--tui`)

`--tui` replaces the cooked stream with a full-screen Watch Board. Git checkout/pull output is not shown (it would corrupt the board). Agent `stdout` and `stderr` are still discarded.

`--tui` needs stdin and stdout TTYs. It fails before any GitHub or git work if either is missing.

### Stopping

While work is active, `q` and Ctrl-C stop after the current issue, or at the next safe git/GitHub boundary. The running agent is not killed or detached, and the next issue is not started.

When the board is idle, `q`, Ctrl-C, or Enter dismisses it.

## Failures

On failure, nightshift reports the process exit status (and a `--model` retry hint when that applies), not agent log text.

The Watch Board keeps the error visible until `q`, Ctrl-C, or Enter. The original exit status is preserved.

## Dry-run with `--tui`

`--tui --dry-run` shows the planned set on the board without spawning an agent. The planned-order / would-invoke / first-prompt preview prints after you dismiss. Without `--tui`, dry-run output is unchanged.
