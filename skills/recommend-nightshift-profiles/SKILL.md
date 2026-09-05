---
name: recommend-nightshift-profiles
disable-model-invocation: true
description: Recommend per-issue Invocation Profiles and a copy-ready nightshift start command for a PRD run.
---

# Recommend Nightshift Profiles

Advice-only skill. Recommend **Invocation Profiles** (agent + model + reasoning effort) for the **Simulated Solvable Set** of a PRD, then emit a **copy-ready live** `nightshift` command. No tracker writes. No profile-map files. No TUI simulation.

North star: **best fit** per issue — no **overkill**, no **underpowered**. Each recommendation needs a short **why**.

Ideal prior flow: plan → `to-nightshift-prd` → `to-nightshift-issues` → this skill. Works without that history if PRD id and repo context exist.

## Process

### 1. Interview run mode

Ask, in order:

1. **Whole-run default agent** — required `--agent` (see README agent matrix). Blank agent rows and no-pick runs keep this value.
2. **Granularity** — one whole-PRD profile vs fine-grained per issue.
3. If fine-grained — which **Preflight Dimensions** to enable:
   - **Agent dimension** (`--pick-agents`) — optional; orthogonal to the others.
   - **Effort dimension** (`--pick-efforts`) **or** **model dimension** (`--pick-models`) — mutually exclusive; either may stack with `--pick-agents`.
4. **Models under consideration** — user names the allowlist and/or whole-run pin. Do **not** scrape agent model catalogs. When `--pick-agents` is on and agents will differ, collect model notes **per candidate agent** the user might choose (still user-supplied, not catalog scrape).

**Capability hard-stop** before any dry-run: check chosen whole-run agent and enabled dimensions against this repo's README agent matrix. Refuse illegal whole-run combos. Point at the matrix; do not invent argv. When issue #5 lands OpenCode and Copilot, reread the README matrix rather than assuming argv.

Rules of thumb:

- **No `--pick-agents`:** whole-run agent must support every enabled knob. Examples that hard-fail: Antigravity with any model/effort/pick-models/pick-efforts; `--pick-efforts` on Cursor; separate effort on Model-Encoded Effort agents; whole-run `--model` / `--reasoning-effort` on Antigravity.
- **With `--pick-agents`:** whole-run model/effort still validated against `--agent` only. Per-row unsupported knobs use **Row-Capable Columns** (skip with short reason) — Cursor skips separate effort; Antigravity skips model and effort. Do not refuse the whole run just because some planned row might pick Cursor/Antigravity.
- `--pick-efforts` and `--pick-models` stay mutually exclusive even when stacked with `--pick-agents`.

**Done when:** default agent, dimensions (or whole-PRD), and model allowlist/pin are explicit, and the combo is capability-legal.

### 2. Resolve execution scope

Infer the repository from `gh` in the current working directory, exactly as `to-nightshift-prd` and `to-nightshift-issues` do. Ask only if `gh` cannot detect a repo or the user wants a different one. Pass `--repo owner/name` only then. **PRD id** is runtime input — take from conversation (e.g. just-published issue number) or ask.

**Done when:** every flag needed for a dry-run is known (`--prd`, `--agent` at minimum; `--repo` only when cwd detection is not enough).

### 3. Plan via dry-run

Run the real binary (prefer `nightshift` on PATH):

```bash
nightshift --prd <prd_id> --agent <agent> --dry-run
# optional: --repo owner/name
```

**Never** pass `--pick-agents`, `--pick-models`, or `--pick-efforts` on this planning dry-run (those force interactive preflight).

Parse the planned order — that is the **Simulated Solvable Set**. Empty set → stop; no recommendations.

Missing binary → stop with install hint from repo README (`cargo install --git …`).

**Done when:** ordered non-empty plan in hand.

### 4. Model character research

For **each distinct** model in the allowlist/pin (and per candidate agent when agents will vary), research cost and performance character (web search). Do not recommend from name vibes alone.

If research fails for a model: **block** until search works **or** the user supplies character notes for that model.

**Done when:** every candidate model has grounded cost/perf notes.

### 5. Recommend profiles

Using PRD, issue bodies, and codebase as needed (fetch/read however repo practice dictates — not prescribed here), assign **best fit** profiles in **planned dry-run order**.

| Mode | Assign |
|---|---|
| Whole-PRD profile | One agent (the `--agent` default) + model + effort (effort omitted/N/A when agent has no separate effort or uses Model-Encoded Effort) |
| Agent-only (`--pick-agents`) | Per-issue agent; model/effort left to cascade (see Same-Agent Defaults Inheritance) |
| Effort-only (`--pick-efforts`) | Fixed whole-run agent + model; per-issue effort from the agent-native enum in the README matrix |
| Full profile (`--pick-models`) | Fixed whole-run agent; per-issue model (from allowlist) + effort where supported |
| Agents + efforts | Per-issue agent, then effort from **that row agent's** native enum; model via cascade |
| Agents + models | Per-issue agent, then model + effort where **that row agent** supports them |

**Same-Agent Defaults Inheritance:** whole-run `--model` / `--reasoning-effort` apply only when the recommended row agent equals `--agent`. A different per-issue agent starts from that agent's defaults unless the recommendation explicitly overrides model/effort for that row. Do not silently carry pin values across agents.

**Row-Capable Columns (recommendations must match real preflight):**

- **Cursor:** effort lives in the model slug — recommend slugs, not a separate effort column value Nightshift would pass. Table effort cell = `—`. Cursor is invoked as `agent`, not `cursor-agent`.
- **Antigravity:** no model/effort columns — recommend agent only; model/effort cells = `—`.
- Other agents: use README matrix enums for effort; free-string models from the user allowlist.

**Done when:** every planned issue has a recommendation and a short **why** defending fit (not overkill, not underpowered), including why that agent when agents vary.

### 6. Optional extras

Ask whether to add any non-profile flags the user cares about (e.g. `--prompt-file`, `--issue`). Only stamp flags they accept. Do not invent defaults for extras.

Note: one `--prompt-file` overrides built-ins for **every** issue regardless of per-issue agent.

**Done when:** extras accepted or explicitly skipped.

### 7. Emit artifacts

Print in this order:

1. **Mode summary** — whole-run `--agent`, enabled Preflight Dimensions (or whole-PRD), pin/allowlist, extras. Mention Same-Agent Defaults Inheritance when agents may differ.
2. **Recommendations**
   - Whole-PRD: one profile (agent + model + effort as applicable) + why.
   - Pick modes: markdown table **before** the command. Columns depend on enabled dimensions:

     | Dimensions | Table |
     |---|---|
     | efforts only | `\| order \| id \| title \| effort \| why \|` |
     | models only | `\| order \| id \| title \| model \| effort \| why \|` |
     | agents only | `\| order \| id \| title \| agent \| why \|` |
     | agents + efforts | `\| order \| id \| title \| agent \| effort \| why \|` |
     | agents + models | `\| order \| id \| title \| agent \| model \| effort \| why \|` |

     Use `—` when a cell is not applicable (Cursor model-encoded effort, Antigravity model/effort, agent-only mode, cascade-to-default).
3. **Copy-ready live command** — fenced full argv for starting the loop (**no** `--dry-run`), including:
   - scope flags: `--prd`, `--agent`, and `--repo` only when needed
   - whole-run: `--model` / `--reasoning-effort` when set (remember: these pre-fill / cascade only for rows that keep `--agent`)
   - fine-grained: any of `--pick-agents`, `--pick-efforts`, `--pick-models` that the interview chose (`--pick-efforts` xor `--pick-models`; `--pick-agents` free to stack)
   - accepted extras
4. **Soft hint** — user may append `--dry-run` to preview plan/preflight without invoking agents. Pick flags still run interactive preflight under dry-run.

Pick-mode table is the human crib sheet for the real TTY preflight; it is **not** encoded into argv (Nightshift has no profile-map file flag). Preflight column order is always agent → model → effort for enabled, row-capable columns; single proceed/abort confirm after all rows.

**Done when:** summary + recs + live command + dry-run hint are all present. Skill ends (advice only — do not start the loop unless the user separately asks).
