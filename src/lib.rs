#![warn(missing_docs)]

//! Nightshift runs a maintainers' loop over GitHub issues that belong to a PRD.
//! It finds child issues by reading structured markdown in each issue body, gates
//! work on any declared blockers, renders a prompt with PRD context, and invokes
//! a coding agent to complete the selected issue. Issue bodies are expected to
//! follow the parser contract described in [`parser`], especially the `Parent`
//! and `Blocked by` sections used by the orchestrator.

/// Agent command selection and process execution.
pub mod agent;
/// Command-line argument parsing for the nightshift binary.
pub mod cli;
/// Monochrome terminal output for orchestrator runs.
pub mod console;
/// Git workspace discovery and hygiene checks.
pub mod git;
/// GitHub issue access through the GitHub CLI.
pub mod github;
/// The PRD child-issue selection and agent execution loop.
pub mod orchestrator;
/// Structured issue-body parsing for PRD parents and blockers.
pub mod parser;
/// Prompt rendering and directive loading for agent runs.
pub mod prompt;
