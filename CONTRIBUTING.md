# Contributing to nightshift

nightshift is a small, focused tool and I would like to keep it that way. Contributions are welcome. Please: **prefer simplicity, document what you add, and make it easy to read and review.**

---

## Tier 1: Adding a new agent

Adding a coding-agent CLI requires command wiring, Invocation Profile capability handling, tests, and docs. Most implementation lives in [`src/agent.rs`](src/agent.rs); user-facing behavior belongs in [`README.md`](README.md) and clap help in [`src/cli.rs`](src/cli.rs).

> [!NOTE]
> Before code, verify that the CLI accepts a prompt via stdin and exits non-zero on failure. nightshift relies on both.

### Invocation Profile Preflight

`--pick-agents` lists `Agent::all()` without probing `PATH`; keep that list in sync when adding an agent. `--pick-agents` may stack with `--pick-efforts` or `--pick-models`; rows collect agent, then model when enabled and supported, then effort when enabled and supported. When `--pick-prompts` is on, collect prompt then mode after effort. Enter on path inherits run-wide; Enter on mode is append; skip the mode line when the path is blank. A row that selects another agent does not inherit whole-run `--model` or `--reasoning-effort`; it uses that agent's defaults unless the row supplies a value. This is Same-Agent Defaults Inheritance. Cursor skips separate effort because it is model-encoded; Antigravity skips model and effort.

### Confirm upstream behavior first

Treat the agent CLI as source of truth. Before choosing a model or effort arm, confirm both sources:

1. **Local CLI help:** install the CLI if needed, then inspect `mynewagent --help` and the relevant non-interactive subcommand help. Confirm stdin mode, model flag, effort flag, and its accepted enum.
2. **Official vendor docs:** confirm the same flags and enum in current official docs, including whether values vary by model or provider.

Do not infer flags from a blog post, another wrapper, or a model catalog. If local help and official docs differ, document the observed version behavior and resolve the discrepancy before merging.

### Step-by-step

1. Add an `Agent` variant with a `///` doc comment naming its CLI. If clap `ValueEnum` kebab-case would be the wrong `--agent` string (`OpenCode` → `open-code`), set `#[value(name = "...")]`:

    ```rust
    /// My new agent CLI, invoked as `mynewagent`.
    MyNewAgent,
    ```

2. Add its `get_command()` arm with program name and flags needed for non-interactive stdin mode. Explain why every non-obvious flag is present:

    ```rust
    // mynewagent: -p reads piped stdin in non-interactive mode
    Self::MyNewAgent => ("mynewagent", vec!["-p"]),
    ```

3. Wire model capability:

    - With a documented non-interactive model flag, pass the supplied `--model` string unchanged from `get_command_with_profile()`.
    - Without one, extend `ensure_model_supported()` to reject explicit `--model` before spawn. Include the agent name, flag, and a retry-without-flag hint.

4. Wire reasoning-effort capability:

    - For an agent with a separate effort flag, add its documented native values to `supported_reasoning_efforts()` and an `append_reasoning_effort_args()` arm. Keep the exact argv mechanism visible:

      ```rust
      Self::MyNewAgent => Some(&["low", "high"]),
      // ...
      Self::MyNewAgent => args.extend(["--effort".into(), effort.into()]),
      ```

    - Without a separate effort flag, return `None` from `supported_reasoning_efforts()` and extend `validate_reasoning_effort()` to hard-reject `--reasoning-effort` with an actionable hint. This also makes `--pick-efforts` fail before fetch.
    - For **Model-Encoded Effort** agents (Cursor pattern), keep model support if the CLI has it, but expose no separate effort control. `--pick-models` becomes model-only; users choose a model slug that encodes effort. Document that nightshift never adds `--reasoning-effort`, rewrites model strings, or injects effort syntax.

5. Validate at the capability level only:

    - Validate only whether the agent supports model/effort selection and whether a supplied effort belongs to its documented native enum. Permit pass-through variants only when the agent documents dynamic variants; state that exception explicitly and leave their exact acceptance to the agent.
    - Do not scrape model catalogs, parse or fuzzy-match model names, or pre-validate model strings.
    - Do not own per-model effort matrices. Pass model strings and let the agent reject a model or model-specific effort subset at runtime.

6. Add focused tests in `src/agent.rs` and, when argv execution needs coverage, [`tests/process_agent_runner.rs`](tests/process_agent_runner.rs):

    - Supported model and each effort path produce exact expected argv.
    - Omitted model/effort keeps the agent-default command path.
    - Unsupported model or effort, and invalid native effort values, fail before spawn with a useful hint.
    - Process-runner coverage observes effort argv through the fake agent.

7. Update docs and help:

    - Add the agent to README's Supported Agents table with model and effort support, wiring, accepted values, and caveats.
    - Update clap flag help for agent-specific restrictions users need before a run.
    - For Model-Encoded Effort, state plainly that effort is chosen by model slug, not a fake effort flag or bracket injection.

8. Run verification:

    ```bash
    cargo fmt --check
    cargo test
    cargo clippy --all-targets -- -D warnings
    ```

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
- Testing `main` / clap flag parsing, except pick-flag contracts in [`src/cli.rs`](src/cli.rs) (mutual exclusion of `--pick-efforts` / `--pick-models`, Preflight Dimension mapping, Cursor help text, OpenCode `--agent` value name vs kebab-case `open-code`). Those are workflow contracts, not parser-string tests.
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
