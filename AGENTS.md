# Repo guidelines

- this repo implements `nightshift`: A rust cli tool to orchestrate fully autonomous issue completion for GitHub repos with ANY coding agent.

- discover repo structure dynamically with cli search tools.
- use `cargo` for building, testing, and linting.
- follow normal rust style, keep codebase `rustfmt` -clean.
- treat documentation as part of the code: add or update concise Rust docs (`//!`/`///`) for modules and significant items, explaining purpose, invariants, side effects, and errors without restating signatures.

- minimize software-entropy by designing with intent.
- favor deep modules: relatively few, large modules with simple interfaces that hide complexity, rather than shallow modules with complex, fragmented interfaces.
- codebase should be easy to change and testable.

- follow pragmatic test driven development inline with the testing philosophy documented in `CONTRIBUTING.md`
- always read documentation before changing code or implementing anything new.
  - official upstream docs — use web search or browse local source.
  - current repo code, tests, public interfaces.
