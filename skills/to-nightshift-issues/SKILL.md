---
name: to-nightshift-issues
description: Break a Nightshift parent context, plan, or spec into single-session GitHub child issues with native parent and blocked-by links. Use when the user wants implementation tickets for a feature set, bug backlog, migration, MVP, or other long-horizon work; choose bounded chunks suited to the work.
disable-model-invocation: true
---

# To Nightshift Issues

Break a parent context into bounded, independently verifiable issues and publish them as direct GitHub sub-issues with native `blocked-by` links.

The target repository is auto-detected by `gh` from the current working directory, exactly as nightshift itself does. Only pass `-R owner/repo` if the user asks for a different repository.

The **parent number** is a runtime input. If `to-nightshift-context` was just run, use the number it returned. Otherwise use the issue number or URL the user supplied; ask for it only when absent.

## How nightshift selects work

nightshift selects work from native GitHub relationships and one label; it uses issue bodies as agent context, not for selection:

- **Membership**: an issue belongs to the run when its native parent is the requested parent. Direct children only; grandchildren are invisible.
- **Ordering**: an issue is ready when every issue in its native `blockedBy` list is closed. Among ready issues, the lowest number runs first.
- **Eligibility**: only issues labelled `ready-for-agent` are fetched.

Everything below exists to produce exactly that shape.

## Process

### 1. Gather Context
Use the conversation context, then fetch the current parent issue and comments:
```bash
gh issue view <parent_number> --comments
```
Read the full body and comments. If `## Blocking Questions` contains unresolved decisions, stop before drafting AFK issues. Resolve them with the user and update the parent first; a blocking decision must not be silently delegated to an AFK child.

### 2. Explore the Codebase (Optional)
If you have not already explored the codebase, do so to understand the current state of the code. Issue titles and descriptions should use the project's domain glossary vocabulary (such as in `CONTEXT.md`), and respect ADRs in the area you're touching.

### 3. Draft Bounded Issues
Choose a decomposition that matches the work:

- **Features and MVPs**: prefer thin vertical slices through the relevant integration layers, each delivering a complete observable behavior.
- **Bug backlogs**: use bounded fixes with a reproducible failure or observable symptom, a desired correction, and a regression check. Group bugs only when one coherent fix addresses them.
- **Migrations**: use verifiable stages that preserve stated invariants and leave the system in a safe state. Capture sequencing and rollback needs as dependencies and acceptance criteria.

Each AFK issue must fit one fresh agent session, be complete enough to implement without another decision, and be independently checkable. Do not split purely by code layer when that leaves incomplete behavior or an unsafe intermediate state.

Issues may be 'HITL' (Human-In-The-Loop) or 'AFK' (Away-From-Keyboard). HITL issues require human interaction, such as an architectural decision or design review. AFK issues can be implemented and merged without human interaction by nightshift. Prefer AFK where the parent supplies enough decisions.

Every issue must be a direct child of the parent. Do not introduce intermediate grouping issues; nightshift cannot see grandchildren.

### 4. Quiz the User
Present the proposed breakdown as a numbered list. For each issue, show:
- **Title**: short descriptive name
- **Type**: HITL / AFK
- **Blocked by**: which other issues (if any) must complete first
- **User stories covered**: which user stories this addresses (if the source material has them)

Ask the user:
- Does the granularity feel right? (too coarse / too fine)
- Are the dependency relationships correct?
- Should any issues be merged or split further?
- Are the correct issues marked as HITL and AFK?

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
## What to change

The bounded behavior, correction, or migration stage this issue delivers.

## Acceptance criteria

- [ ] Criterion 1
- [ ] Criterion 2

## Notes

Optional: parent decisions and constraints that matter here. The parent governs initiative-wide rules; this issue governs its specific acceptance criteria. Resolve contradictions before publishing.
```

**Never write `## Parent`, `## Blocked by`, or dependency lists into the body.** nightshift does not read them, so they would silently disagree with the real relationships. Relationships are created only through the `gh` flags in Step C.

#### Step C: Create issues in dependency order
Create blockers before the issues they block, so every `--blocked-by` value is a real, existing issue number. Among independent issues, create them in the order you want them executed: nightshift breaks ties by lowest issue number, so creation order is the default run order.

One command per issue creates the issue, the parent link, and the blocked-by links together:

```bash
gh issue create \
  --title "<issue title>" \
  --body-file /tmp/nightshift-issue-<n>.md \
  --label ready-for-agent \
  --parent <parent_number> \
  --blocked-by <blocker_number>,<blocker_number>
```

- Omit `--blocked-by` when the issue has no blockers.
- **HITL issues** use `--label ready-for-human` instead of `ready-for-agent`, never both. Still pass `--parent <parent_number>` so AFK issues can be `--blocked-by` them; nightshift then waits until a human closes the HITL issue.
- Optionally add a category label (`enhancement`, `bug`, `documentation`) with a second `--label`. Category labels never replace the readiness label.
- Blockers outside the parent (existing issues in the repo) are allowed; nightshift honors their state.

`gh` prints the new issue URL. Extract the number from it and use it for later `--blocked-by` values.

### 6. Verify and report
Read the graph back and check it matches the approved breakdown:
```bash
gh issue list --label ready-for-agent --state open --json number,title,parent,blockedBy
```
Every AFK issue must show `parent.number` equal to the parent number and the expected `blockedBy` numbers. If `nightshift` is installed, run `nightshift --parent <parent_number> --agent <agent> --dry-run` and confirm the printed order matches the intended dependency order.

Present a table of the published issues: number, title, type, parent, blocked-by.
