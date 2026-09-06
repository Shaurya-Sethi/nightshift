//! Pure Invocation Profile data and cascade resolution.
//!
//! A [`crate::invocation_profile::WholeRunInvocationDefaults`] value supplies a
//! required agent and optional run-wide model and reasoning-effort choices. A
//! [`crate::invocation_profile::PerIssueInvocationOverride`] can replace any field
//! for one child issue in a [`crate::invocation_profile::RunEphemeralProfileMap`].
//! [`crate::invocation_profile::resolve`] applies Same-Agent Defaults Inheritance
//! without I/O or agent validation.

use crate::agent::Agent;
use crate::prompt::PerIssuePrompt;

/// Whole-Run Invocation Defaults used when a child issue has no override.
///
/// `agent` is required. Each optional field remains available for the
/// Agent-Default Invocation unless it applies under Same-Agent Defaults
/// Inheritance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WholeRunInvocationDefaults<'a> {
    /// Required coding agent used when no child issue overrides it.
    pub agent: Agent,
    /// Explicit model identifier inherited only by the same resolved agent.
    pub model: Option<&'a str>,
    /// Explicit effort inherited only by the same resolved agent.
    pub reasoning_effort: Option<&'a str>,
}

/// Optional Invocation Profile fields selected for one child issue.
///
/// Values are owned so a Run-Ephemeral Profile Map can retain interactive
/// preflight input. Empty model and effort fields are absent, so blank
/// preflight input cannot replace a Whole-Run Invocation Default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PerIssueInvocationOverride {
    /// Coding agent selected for this child issue.
    pub agent: Option<Agent>,
    /// Model identifier selected for this child issue.
    pub model: Option<String>,
    /// Agent-native reasoning effort selected for this child issue.
    pub reasoning_effort: Option<String>,
    /// Per-issue prompt snapshot. `None` inherits the run-wide [`crate::prompt::DirectivePolicy`].
    pub prompt: Option<PerIssuePrompt>,
}

/// In-memory overrides keyed by child issue number for one run only.
///
/// The Run-Ephemeral Profile Map owns preflight selections and is deliberately
/// not persisted to issues or the repository.
pub type RunEphemeralProfileMap = std::collections::HashMap<u32, PerIssueInvocationOverride>;

/// Optional Invocation Profile Preflight columns enabled for one run.
///
/// `agents` may combine with `--pick-efforts` or `--pick-models` (those two remain
/// mutually exclusive at the CLI / orchestrator boundary) **and** with `prompts`.
/// This type only records requested columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreflightDimensions {
    /// Collect a per-issue coding agent.
    pub agents: bool,
    /// Collect a per-issue reasoning-effort value.
    pub efforts: bool,
    /// Collect a per-issue model and, where supported, reasoning effort.
    pub models: bool,
    /// Collect a per-issue prompt file path and append/replace mode.
    pub prompts: bool,
}

impl PreflightDimensions {
    /// True when any preflight column is requested, including `--pick-prompts` alone.
    pub fn requested(self) -> bool {
        self.agents || self.efforts || self.models || self.prompts
    }
}

/// Resolved agent, model, and reasoning-effort choices for one invocation.
///
/// `None` leaves model or effort to the resolved agent's persisted default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationProfile<'a> {
    /// Coding agent that receives this invocation.
    pub agent: Agent,
    /// Explicit model identifier, passed through to agents that support models.
    pub model: Option<&'a str>,
    /// Explicit agent-native reasoning-effort level.
    pub reasoning_effort: Option<&'a str>,
}

/// Resolves one child issue's Invocation Profile.
///
/// A per-issue agent wins when present. Explicit non-empty model and effort
/// overrides win their fields. Otherwise, Whole-Run model and effort defaults
/// apply only when the resolved agent is the Whole-Run agent; cross-agent rows
/// omit those fields for Agent-Default Invocation.
pub fn resolve<'a>(
    defaults: WholeRunInvocationDefaults<'a>,
    issue_override: Option<&'a PerIssueInvocationOverride>,
) -> InvocationProfile<'a> {
    let agent = issue_override
        .and_then(|override_| override_.agent)
        .unwrap_or(defaults.agent);
    let inherits_defaults = agent == defaults.agent;
    let override_model = issue_override
        .and_then(|override_| override_.model.as_deref())
        .filter(|value| !value.is_empty());
    let override_effort = issue_override
        .and_then(|override_| override_.reasoning_effort.as_deref())
        .filter(|value| !value.is_empty());

    InvocationProfile {
        agent,
        model: override_model.or(inherits_defaults.then_some(defaults.model).flatten()),
        reasoning_effort: override_effort.or(inherits_defaults
            .then_some(defaults.reasoning_effort)
            .flatten()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PerIssueInvocationOverride, PreflightDimensions, RunEphemeralProfileMap,
        WholeRunInvocationDefaults, resolve,
    };
    use crate::agent::Agent;

    fn defaults<'a>(
        model: Option<&'a str>,
        reasoning_effort: Option<&'a str>,
    ) -> WholeRunInvocationDefaults<'a> {
        WholeRunInvocationDefaults {
            agent: Agent::Pi,
            model,
            reasoning_effort,
        }
    }

    fn issue_override(
        agent: Option<Agent>,
        model: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> PerIssueInvocationOverride {
        PerIssueInvocationOverride {
            agent,
            model: model.map(str::to_owned),
            reasoning_effort: reasoning_effort.map(str::to_owned),
            ..PerIssueInvocationOverride::default()
        }
    }

    #[test]
    fn resolve_prefers_non_blank_per_issue_fields() {
        let issue_override = issue_override(Some(Agent::Claude), Some("issue-model"), Some("high"));
        let profile = resolve(
            defaults(Some("run-model"), Some("medium")),
            Some(&issue_override),
        );

        assert_eq!(profile.agent, Agent::Claude);
        assert_eq!(profile.model, Some("issue-model"));
        assert_eq!(profile.reasoning_effort, Some("high"));
    }

    #[test]
    fn resolve_uses_per_issue_model_and_effort_with_whole_run_agent() {
        let issue_override = issue_override(None, Some("issue-model"), Some("high"));
        let profile = resolve(
            defaults(Some("run-model"), Some("medium")),
            Some(&issue_override),
        );

        assert_eq!(profile.agent, Agent::Pi);
        assert_eq!(profile.model, Some("issue-model"));
        assert_eq!(profile.reasoning_effort, Some("high"));
    }

    #[test]
    fn resolve_inherits_defaults_when_resolved_agent_matches_whole_run_agent() {
        let issue_override = issue_override(None, Some("issue-model"), None);
        let profile = resolve(
            defaults(Some("run-model"), Some("medium")),
            Some(&issue_override),
        );

        assert_eq!(profile.agent, Agent::Pi);
        assert_eq!(profile.model, Some("issue-model"));
        assert_eq!(profile.reasoning_effort, Some("medium"));
    }

    #[test]
    fn resolve_inherits_model_when_same_agent_overrides_effort() {
        let issue_override = issue_override(Some(Agent::Pi), None, Some("high"));
        let profile = resolve(
            defaults(Some("run-model"), Some("medium")),
            Some(&issue_override),
        );

        assert_eq!(profile.agent, Agent::Pi);
        assert_eq!(profile.model, Some("run-model"));
        assert_eq!(profile.reasoning_effort, Some("high"));
    }

    #[test]
    fn resolve_does_not_inherit_missing_effort_for_a_cross_agent_model_override() {
        let issue_override = issue_override(Some(Agent::Claude), Some("issue-model"), None);
        let profile = resolve(
            defaults(Some("run-model"), Some("medium")),
            Some(&issue_override),
        );

        assert_eq!(profile.agent, Agent::Claude);
        assert_eq!(profile.model, Some("issue-model"));
        assert_eq!(profile.reasoning_effort, None);
    }

    #[test]
    fn resolve_uses_overridden_agent_without_cross_agent_defaults() {
        let issue_override = issue_override(Some(Agent::Claude), None, None);
        let profile = resolve(
            defaults(Some("run-model"), Some("medium")),
            Some(&issue_override),
        );

        assert_eq!(profile.agent, Agent::Claude);
        assert_eq!(profile.model, None);
        assert_eq!(profile.reasoning_effort, None);
    }

    #[test]
    fn resolve_treats_blank_override_fields_as_missing() {
        let issue_override = issue_override(None, Some(""), Some(""));
        let profile = resolve(
            defaults(Some("run-model"), Some("medium")),
            Some(&issue_override),
        );

        assert_eq!(profile.agent, Agent::Pi);
        assert_eq!(profile.model, Some("run-model"));
        assert_eq!(profile.reasoning_effort, Some("medium"));
    }

    #[test]
    fn resolve_uses_whole_run_defaults_without_an_override() {
        let profile = resolve(defaults(Some("run-model"), Some("medium")), None);

        assert_eq!(profile.agent, Agent::Pi);
        assert_eq!(profile.model, Some("run-model"));
        assert_eq!(profile.reasoning_effort, Some("medium"));
    }

    #[test]
    fn profile_map_keeps_owned_overrides_by_issue_number() {
        let mut profiles = RunEphemeralProfileMap::new();
        profiles.insert(
            42,
            issue_override(Some(Agent::Codex), Some("issue-model"), Some("high")),
        );

        assert_eq!(profiles[&42].agent, Some(Agent::Codex));
        assert_eq!(profiles[&42].model.as_deref(), Some("issue-model"));
        assert!(!profiles.contains_key(&43));
    }

    #[test]
    fn preflight_dimensions_default_to_no_columns() {
        assert_eq!(
            PreflightDimensions::default(),
            PreflightDimensions {
                agents: false,
                efforts: false,
                models: false,
                prompts: false,
            }
        );
    }
}
