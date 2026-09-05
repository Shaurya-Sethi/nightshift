# Contributing to nightshift

nightshift is a small, focused tool and I would like to keep it that way. Contributions are welcome. Please: **prefer simplicity, document what you add, and make it easy to read and review.**

---

## Tier 1: Adding a new agent

Adding support for a new coding agent CLI requires command wiring, model handling, tests, and docs. Agent support lives mostly in [`src/agent.rs`](src/agent.rs), with user-facing docs in [`README.md`](README.md).

**Step-by-step:**

1. Add a new variant to the `Agent` enum with a `///` doc comment naming the CLI:

    ```rust
    /// My new agent CLI, invoked as `mynewagent`.
    MyNewAgent,
    ```

2. Add an arm to `get_command()` with the program name and any flags needed for non-interactive stdin mode. Follow the comment style of existing arms to explain _why_ each flag is used:

    ```rust
    // mynewagent: -p reads piped stdin in non-interactive mode
    Self::MyNewAgent => ("mynewagent", vec!["-p"]),
    ```

3. Decide how the agent handles `--model`:

    - If the CLI has a documented non-interactive model flag, pass the supplied `--model` string unchanged from `get_command_with_profile()`.
    - If the CLI has no documented non-interactive model flag, reject explicit models from `ensure_model_supported()` with a clear error. The error should tell users to retry without `--model`.

4. Keep validation at the capability level only:

    - Do not scrape model catalogs, parse provider/model strings, or fuzzy-match requested models.
    - Do not pre-validate specific model names in nightshift.
    - Let the agent CLI accept or reject the exact `--model` value.

5. Add or extend focused tests in `src/agent.rs`:

    - Explicit `--model` is included in the command for supported agents and passed through unchanged.
    - Explicit `--model` is rejected for unsupported agents.
    - Omitted `--model` preserves the persisted-default command path.
    - Agent-process failures surface exit status, and `--model` failures add only a soft retry hint.

6. Update [`README.md`](README.md):

    - Add the agent to the Supported Agents table.
    - Mark whether `--model` is supported.
    - Mention any important caveat in plain user-facing language.

7. Run the verification commands:

    ```bash
    cargo fmt --check
    cargo test
    cargo clippy --all-targets -- -D warnings
    ```

> [!NOTE]
> Before adding an agent, please verify that its CLI accepts a prompt via stdin (not only via argv), and that it exits with a non-zero status code on failure. nightshift relies on both behaviours.

---

## Tier 2: Everything else

If your change touches the parser, orchestrator logic, CLI flags, GitHub adapter, or prompt rendering: **open an issue first** and describe what you want to change and why. This keeps things simple and avoids wasted effort.

Changes in this tier should follow the existing style: one responsibility per module, trait-based adapters for anything that shells out, and clear rustdoc on every public item.

---

## Code style

- Follow the patterns already in the codebase.
- Keep things simple. A solution you can read easily is better than a clever one.
- Run `cargo fmt` and `cargo clippy` before opening a PR.

---

## Rustdoc

Every `pub` item needs a `///` doc comment:

- If the function can fail, document it under `# Errors`.
- If usage isn't obvious, add a `# Examples` block; doctests are welcome.
- Module-level `//!` comments should explain the module's responsibility in one short paragraph.
- `pub(crate)` helpers don't need full doc comments; inline `//` comments are fine there.

---

## Tests

nightshift tests protect **workflow contracts** and should not be focused on coverage percentages.

Tests live next to the code they guard: `#[cfg(test)]` modules at the bottom of `src/*.rs`, so they can exercise pure logic and `pub(crate)` helpers without shelling out to `gh`, `git`, or any agent CLI.

**Highest priority:** native GitHub relationship selection (`parent.number` for PRD membership, `blockedBy.nodes[].state` for ordering). Regressions there break candidate filtering and blocker gating. These tests should use JSON fixtures shaped like `gh issue list --json number,title,body,parent,blockedBy` output, with issue bodies that contain no `## Parent` / `## Blocked by` text, and be named after workflow behaviour, not function names.

**Secondary:** pure string helpers (e.g. GitHub remote slug parsing) and orchestrator loop tests via trait mocks (`GithubIssues`, `GitOps`, `AgentRunner`).

**Intentionally out of scope:**
- Testing `main` / clap flag parsing
- Adapter subprocess wiring
- Network or auth-dependent flows
- Snapshotting every function
- Redundant "parser again inside orchestrator" cases (selection tests live in `parser.rs`)
- Filesystem or temp-dir tests unless they catch a real regression

New tests should stay deterministic: `cargo test` should be fast and pass without `gh` being installed or the user being logged in. If a test would require a real clone, remote, or agent binary, extend the pure logic tests instead, or document manual verification.

Process-runner integration tests live in [`tests/process_agent_runner.rs`](tests/process_agent_runner.rs). They fake agent CLIs via `PATH` and a small cross-platform `nightshift-fake-agent` binary (not shell scripts). CI runs `cargo test --all` on Linux and Windows.

---

## Parser internals: read before touching `parser.rs`

[`src/parser.rs`](src/parser.rs) is a pure function over `gh issue list` JSON. It does not read issue bodies.

- **Membership:** `parent.number == prd`. Direct children only; grandchildren are ignored.
- **Ordering:** an issue is ready when every `blockedBy.nodes[].state` is closed (case-insensitive). Empty `nodes` means ready. A blocker outside the PRD set still counts; honor the node's `state`.
- **Pick:** lowest ready issue number at or above `--issue`.
- **No fallback:** `## Parent` / `## Blocked by` in the body is ignored.

The JSON fields are `{"nodes": [...], "totalCount": N}` objects, not flat arrays. Extra fields on `parent` / blocker nodes are ignored. `blockedBy.nodes` is capped by `gh` at 50; nightshift uses the nodes it gets.

If you want to change this contract, open an issue first.
