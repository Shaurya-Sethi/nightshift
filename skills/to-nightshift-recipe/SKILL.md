---
name: to-nightshift-recipe
description: Write a nightshift YAML run recipe for a PRD and give the user `nightshift --recipe PATH`. Use when the user wants per-issue agent/model/effort choices, a repeatable unattended run, or a start command for a PRD.
disable-model-invocation: true
---

# To Nightshift Recipe

Interview for whole-run defaults, stamp the planned set with `nightshift --write-recipe`, fill per-issue profiles when the user wants variation, and print `nightshift --recipe PATH`.

Do not pass `--pick-*`. Do not emit a copy-ready argv crib sheet as the primary artifact. Do not start the loop unless the user separately asks.

North star: **best fit** per issue, with no **overkill** and no **underpowered**. Each filled row needs a short **why**.

Ideal prior flow: plan with an agent (or otherwise) → `to-nightshift-prd` → `to-nightshift-issues` → this skill.

## Process

### 1. Interview run mode

Ask, in order:

1. **Whole-run default agent**: required `--agent` (see README agent matrix). Recipe rows that omit `agent` keep this value.
2. **Granularity**: one whole-PRD profile vs per-issue variation in the recipe.
3. **Models under consideration**: user names the allowlist and/or whole-run pin. Do **not** scrape agent model catalogs. When agents will differ across rows, collect model notes **per candidate agent** (still user-supplied).
4. Optional extras the user wants stamped on generate: `--model`, `--reasoning-effort`, `--prompt-file` / `--append-prompt-file`, `--issue`, `--repo` only when cwd detection is not enough.

**Capability hard-stop** before `--write-recipe`: check the whole-run agent (and any `--model` / `--reasoning-effort`) against this repo's README agent matrix. Refuse illegal combos. Point at the matrix; do not invent flags.

When filling per-issue rows later, skip knobs that row's agent cannot use (Cursor: no separate effort). Antigravity supports both `model` and `reasoning_effort`. Same-agent defaults: whole-run `model` / `reasoning_effort` apply only while the row agent equals the whole-run `agent`.

**Done when:** default agent, granularity, and model allowlist/pin are explicit, and the combo is capability-legal.

### 2. Resolve execution scope

Infer the repository from `gh` in the current working directory, exactly as `to-nightshift-prd` and `to-nightshift-issues` do. Ask only if `gh` cannot detect a repo or the user wants a different one.

**PRD id** is runtime input: take from conversation or ask.

**Write path:** default `./prd-<prd>-recipe.yaml`. If that file exists, stop and ask for another path (nightshift will not overwrite). Use a user-supplied path when they name one.

**Done when:** `--prd`, `--agent`, write path, and `--repo` (only if needed) are known.

### 3. Stamp the planned set

Run the real binary (prefer `nightshift` on PATH):

```bash
nightshift --prd <prd_id> --agent <agent> --write-recipe <path>
```

Add `--model`, `--reasoning-effort`, `--prompt-file` or `--append-prompt-file`, `--issue`, and `--repo` only when the interview accepted them. Never pass `--pick-*` or `--tui`.

Empty planned set or a missing binary → stop. Install hint from repo README (`cargo install --git …` / `cargo install nightshift-cli`).

The file is already a valid recipe. `--write-recipe` prints the path.

**Done when:** the YAML exists and lists every planned issue (`number`, `title`, `agent`).

### 4. Model character research (per-issue variation only)

If the user chose whole-PRD profile, skip to step 6.

For **each distinct** model in the allowlist/pin (and per candidate agent when agents will vary), research cost and performance character (web search). Do not recommend from name vibes alone.

If research fails for a model: **block** until search works **or** the user supplies character notes for that model.

**Done when:** every candidate model has grounded cost/perf notes.

### 5. Fill per-issue rows

Using PRD, issue bodies, and codebase as needed, assign **best fit** profiles in planned order and edit the YAML in place.

- Keep whole-run `agent` / `model` / `reasoning_effort` at the top of the file.
- Change a row's `agent`, `model`, or `reasoning_effort` only when it should differ. Omit a field to inherit (same-agent rule).
- Cursor: effort lives in the model slug; do not set `reasoning_effort` on that row.
- Antigravity: `model` comes from the user allowlist; `reasoning_effort` accepts `low`, `medium`, `high`, or `max`.
- Other agents: effort from the README matrix; models from the user allowlist.
- `prompt_file` requires `prompt_mode` (`append` or `replace`). Paths must be absolute.
- Do not add `pick_*`, `tui`, `dry_run`, or `append_prompt_file` keys. Do not change `number`. `title` is documentary.

**Done when:** every planned issue has a recommendation and a short **why**.

### 6. Emit artifacts

Print in this order:

1. **Mode summary**: whole-run `agent`, whole-PRD vs per-issue, pin/allowlist, extras. Mention Same-Agent Defaults Inheritance when agents may differ.
2. **Recommendations**: markdown table in planned order, with a **why** column. Use `n/a` when a cell does not apply.
3. **Copy-ready command** (no `--dry-run`):

```bash
nightshift --recipe <path>
```

4. **Soft hint:** `nightshift --recipe <path> --dry-run` validates the recipe against the live planned set without spawning.

**Done when:** summary + recs + `--recipe` command + dry-run hint are present. Skill ends.
