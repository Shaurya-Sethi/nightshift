#![warn(missing_docs)]

//! Nightshift runs a maintainers' loop over GitHub issues that belong to a PRD.
//! It finds child issues from native GitHub `parent` links, gates work on
//! `blockedBy` relationships, renders a prompt with PRD context, and invokes
//! a coding agent to complete the selected issue. Selection is described in
//! [`parser`].

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
/// Per-invocation model and reasoning-effort settings.
pub mod invocation_profile;
/// The PRD child-issue selection and agent execution loop.
pub mod orchestrator;
/// Next-issue selection from native GitHub relationship JSON.
pub mod parser;
/// Prompt rendering and directive loading for agent runs.
pub mod prompt;
