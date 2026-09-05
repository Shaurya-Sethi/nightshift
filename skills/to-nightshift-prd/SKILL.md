---
name: to-nightshift-prd
description: Turn the current conversation context into a PRD and publish it as a GitHub issue that nightshift can work through. Use when user wants to create a PRD from the current context.
disable-model-invocation: true
---

# To Nightshift PRD

This skill takes the current conversation context and codebase understanding and produces a Product Requirements Document (PRD), publishing it directly as a GitHub issue in the current repository.

Do NOT interview the user — just synthesize what you already know.

The target repository is auto-detected by `gh` from the current working directory, exactly as nightshift itself does. Only pass `-R owner/repo` if the user asks for a different repository.

## Process

### 1. Explore Codebase
Explore the repo to understand the current state of the codebase, if you haven't already. Use the project's domain glossary vocabulary (such as in `CONTEXT.md`) throughout the PRD, and respect any ADRs in the area you're touching.

### 2. Sketch Major Modules
Sketch out the major modules you will need to build or modify to complete the implementation. Actively look for opportunities to extract deep modules that can be tested in isolation.

A deep module (as opposed to a shallow module) is one which encapsulates a lot of functionality in a simple, testable interface which rarely changes.

Check with the user that these modules match their expectations. Check with the user which modules they want tests written for.

### 3. Write PRD
Draft the PRD using the template below. Use the template headers and structure exactly.

<prd-template>

# PRD: [Feature Title]

## Problem Statement

The problem that the user is facing, from the user's perspective.

## Solution

The solution to the problem, from the user's perspective.

## User Stories

A LONG, numbered list of user stories. Each user story should be in the format of:

1. As an <actor>, I want a <feature>, so that <benefit>

Example:
1. As a developer, I want to list open issues from the terminal, so that I can pick the next one to work on.

This list of user stories should be extremely extensive and cover all aspects of the feature.

## Implementation Decisions

A list of implementation decisions that were made. This can include:

- The modules that will be built/modified
- The interfaces of those modules that will be modified
- Technical clarifications from the developer
- Architectural decisions
- Schema changes
- API contracts
- Specific interactions

Do NOT include specific file paths or code snippets. They may end up being outdated very quickly.

Exception: if a prototype produced a snippet that encodes a decision more precisely than prose can (state machine, reducer, schema, type shape), inline it within the relevant decision and note briefly that it came from a prototype. Trim to the decision-rich parts — not a working demo, just the important bits.

## Testing Decisions

A list of testing decisions that were made. Include:

- A description of what makes a good test (only test external behavior, not implementation details)
- Which modules will be tested
- Prior art for the tests (i.e. similar types of tests in the codebase)

## Out of Scope

A description of the things that are out of scope for this PRD.

## Further Notes

Any further notes about the feature.

</prd-template>

### 4. Publish to GitHub

Write the full PRD markdown (real newlines) to a temp file, then create the issue:

```bash
gh issue create --title "PRD: [Feature Title]" --body-file /tmp/nightshift-prd.md
```

Do not add labels to the PRD. nightshift reads the PRD body directly by number; only its child issues need the `ready-for-agent` label, and `to-nightshift-issues` applies that.

`gh` prints the URL of the new issue. Extract the issue number from it (the last path segment), display the number and URL to the user, and tell them to run `to-nightshift-issues` with that number to break the PRD into child issues.
