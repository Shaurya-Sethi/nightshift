---
name: to-nightshift-context
description: Author or revise a durable GitHub parent issue that gives Nightshift coding agents the larger context for coherent work spanning multiple fresh sessions. Use for a feature set, MVP, migration, related bug backlog, or other long-horizon initiative before breaking it into Nightshift issues; adapt the document to the work instead of forcing a PRD.
disable-model-invocation: true
---

# To Nightshift Context

Create the parent issue whose body Nightshift will include in every child issue's agent prompt. It preserves the outcome and decisions across fresh sessions. `to-nightshift-issues` turns this context into executable child issues; this skill does not prescribe their boundaries or dependency graph.

Use one parent for one coherent objective. If the request is one session of work, suggest a standalone issue instead. If it combines unrelated objectives, propose separate parents.

The target repository is detected by `gh` from the current working directory. Use `-R owner/repo` only when the user requests a different repository. Create a new issue by default; revise an existing parent only when the user explicitly supplies its number or URL.

## Process

### 1. Establish the work

Use the conversation and supplied material first. Inspect relevant repo docs, current code, tests, public interfaces, domain vocabulary, and ADRs enough to distinguish current behavior from the desired outcome. Research external facts when the work requires them. Do not make the user repeat settled context.

Identify the kind of work and its coherent objective. Ask only for consequential missing decisions that would change scope, constraints, or success. Leave routine ticket and implementation choices to the later stage. Distinguish facts, settled decisions, assumptions, and open questions; never turn a guess into a requirement.

### 2. Draft a fitting parent document

Choose headings and detail suited to the work. The document should let an agent understand:

- the objective and why it matters;
- the relevant current state, including evidence or symptoms where useful;
- what is in scope, what is out of scope, and constraints that apply across child issues;
- observable success signals;
- settled product, architectural, operational, and testing decisions that later agents must preserve;
- the known major work areas, when they help decomposition.

For a bug backlog, preserve the known failures and shared cause or goal without inventing user stories. For a migration, explain the starting state, target state, invariants, and any rollout or rollback requirements. For a feature set or MVP, describe the intended capabilities and user-facing behavior. These are examples, not mandatory templates.

Keep it concise and agent-legible. Include stable references or exact contracts when they carry a decision, but avoid a speculative file-by-file implementation plan. The parent describes the destination and initiative-wide rules; child issues carry bounded work and their own acceptance criteria.

If a consequential unknown remains, add a `## Blocking Questions` section with an unchecked item for each unresolved decision and state that AFK breakdown is blocked. Minor questions may be recorded without blocking. Do not claim the context is ready for `to-nightshift-issues` until blocking questions are resolved.

### 3. Review with the user

Show the full proposed title and body. Ask the user to confirm or correct the draft, including its scope, decisions, success signals, and any blocking questions. Incorporate corrections before publishing. If the user requested a draft only, stop here.

### 4. Publish or revise

Write the approved Markdown body to a temporary file with real newlines. For a new parent, create the issue with a descriptive title:

```bash
gh issue create --title "<descriptive title>" --body-file <temp-file>
```

When the user explicitly supplied an existing parent to revise, read its body and comments first, preserve relevant decisions, show the proposed replacement for review, then use `gh issue edit <number> --title "<title>" --body-file <temp-file>`.

Do not add `ready-for-agent` or `ready-for-human` to the parent. Nightshift runs only eligible direct children; `to-nightshift-issues` creates those labels and relationships.

Report the parent number and URL. If no blocking questions remain, point the user to `to-nightshift-issues` with that number. If blockers remain, state that AFK breakdown must wait for their resolution.
