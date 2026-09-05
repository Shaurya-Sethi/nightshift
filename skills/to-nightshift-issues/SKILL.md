---
name: to-nightshift-issues
description: Break a plan, spec, or PRD into nightshift-runnable GitHub issues (sub-issues of the PRD with native blocked-by links) using tracer-bullet vertical slices. Use when user wants to convert a plan into issues, create implementation tickets, or break down work into issues.
disable-model-invocation: true
---

# To Nightshift Issues

Break a plan or PRD into independently-grabbable issues using vertical slices (tracer bullets), and publish them as GitHub sub-issues of the PRD with native `blocked-by` links.

The target repository is auto-detected by `gh` from the current working directory, exactly as nightshift itself does. Only pass `-R owner/repo` if the user asks for a different repository.

The **PRD number** (the GitHub issue number to break down) is a runtime input. If `to-nightshift-prd` was just run in this conversation, use the number it returned. If the number is not already in context, ask the user: *"What is the PRD issue number you'd like to break down?"*

## How nightshift selects work

nightshift ignores issue bodies entirely. It selects work from native GitHub relationships and one label:

- **Membership**: an issue belongs to the PRD when its native parent (sub-issue relationship) is the PRD. Direct children only; grandchildren are invisible.
- **Ordering**: an issue is ready when every issue in its native `blockedBy` list is closed. Among ready issues, the lowest number runs first.
- **Eligibility**: only issues labelled `ready-for-agent` are fetched.

Everything below exists to produce exactly that shape.

## Process

### 1. Gather Context
Work from whatever is already in the conversation context. If the user passes a PRD reference (issue number or URL), fetch it:
```bash
gh issue view <prd_number> --comments
```
Read the full body and comments of the PRD.

### 2. Explore the Codebase (Optional)
If you have not already explored the codebase, do so to understand the current state of the code. Issue titles and descriptions should use the project's domain glossary vocabulary (such as in `CONTEXT.md`), and respect ADRs in the area you're touching.

### 3. Draft Vertical Slices
Break the plan into **tracer bullet** issues. Each issue is a thin vertical slice that cuts through ALL integration layers end-to-end, NOT a horizontal slice of one layer.

Slices may be 'HITL' (Human-In-The-Loop) or 'AFK' (Away-From-Keyboard). HITL slices require human interaction, such as an architectural decision or a design review. AFK slices can be implemented and merged without human interaction by nightshift. Prefer AFK over HITL where possible.

<vertical-slice-rules>
- Each slice delivers a narrow but COMPLETE path through every layer (schema, API, UI, tests)
- A completed slice is demoable or verifiable on its own
- Prefer many thin slices over few thick ones
- Every slice is a direct child of the PRD. Do not introduce intermediate epic or grouping issues; nightshift cannot see grandchildren.
</vertical-slice-rules>

### 4. Quiz the User
Present the proposed breakdown as a numbered list. For each slice, show:
- **Title**: short descriptive name
- **Type**: HITL / AFK
- **Blocked by**: which other slices (if any) must complete first
- **User stories covered**: which user stories this addresses (if the source material has them)

Ask the user:
- Does the granularity feel right? (too coarse / too fine)
- Are the dependency relationships correct?
- Should any slices be merged or split further?
- Are the correct slices marked as HITL and AFK?

Iterate until the user approves the breakdown.

### 5. Publish to GitHub

#### Step A: Ensure labels exist
Idempotent; safe to run every time:
```bash
gh label create ready-for-agent --description "Fully specified, ready for an AFK agent" --color FEF2C0 --force
gh label create ready-for-human --description "Requires human implementation" --color D4C5F9 --force
```

#### Step B: Write each issue body
Write each body to its own temp file with real newlines, using this structure:

```markdown
## What to build

The end-to-end behaviour this slice delivers, from the user's perspective.

## Acceptance criteria

- [ ] Criterion 1
- [ ] Criterion 2

## Notes

Optional: pointers, constraints, decisions from the PRD that matter here.
```

**Never write `## Parent`, `## Blocked by`, or dependency lists into the body.** nightshift does not read them, so they would silently disagree with the real relationships. Relationships are created only through the `gh` flags in Step C.

#### Step C: Create issues in dependency order
Create blockers before the issues they block, so every `--blocked-by` value is a real, existing issue number. Among independent slices, create them in the order you want them executed: nightshift breaks ties by lowest issue number, so creation order is the default run order.

One command per slice creates the issue, the parent link, and the blocked-by links together:

```bash
gh issue create \
  --title "<slice title>" \
  --body-file /tmp/nightshift-issue-<n>.md \
  --label ready-for-agent \
  --parent <prd_number> \
  --blocked-by <blocker_number>,<blocker_number>
```

- Omit `--blocked-by` when the slice has no blockers.
- **HITL slices** use `--label ready-for-human` instead of `ready-for-agent`, never both. Still pass `--parent <prd_number>` so AFK slices can be `--blocked-by` them; nightshift then waits until a human closes the HITL issue.
- Optionally add a category label (`enhancement`, `bug`, `documentation`) with a second `--label`. Category labels never replace the readiness label.
- Blockers outside the PRD (existing issues in the repo) are allowed; nightshift honors their state.

`gh` prints the new issue URL. Extract the number from it and use it for later `--blocked-by` values.

### 6. Verify and report
Read the graph back and check it matches the approved breakdown:
```bash
gh issue list --label ready-for-agent --state open --json number,title,parent,blockedBy
```
Every AFK slice must show `parent.number` equal to the PRD number and the expected `blockedBy` numbers. If `nightshift` is installed, run `nightshift --prd <prd_number> --agent <agent> --dry-run` and confirm the printed order matches the intended dependency order.

Present a table of the published issues: number, title, type, parent, blocked-by.
