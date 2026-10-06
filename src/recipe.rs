//! User-owned YAML run recipes.
//!
//! A recipe is a complete run spec: whole-run defaults plus an issue list that
//! replaces TTY preflight. Parse, path, and capability checks run before any
//! GitHub fetch or agent spawn. [`Recipe::from_planned`] stamps a valid skeleton
//! for `--write-recipe`.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use crate::agent::Agent;
use crate::github::GithubIssue;
use crate::invocation_profile::{
    PerIssueInvocationOverride, RunEphemeralProfileMap, WholeRunInvocationDefaults, resolve,
};
use crate::prompt::{DirectivePolicy, PerIssuePrompt, PromptMode, load_directives};

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn is_main(value: &str) -> bool {
    value == "main"
}

fn default_base_branch() -> String {
    "main".to_string()
}

fn deserialize_issue<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    Ok(Option::<u32>::deserialize(deserializer)?.unwrap_or(0))
}

fn deserialize_base_branch<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_base_branch))
}

/// Default `--write-recipe` destination for a parent: `parent-<parent>-recipe.yaml` in cwd.
pub fn default_write_path(parent: u32) -> PathBuf {
    PathBuf::from(format!("parent-{parent}-recipe.yaml"))
}

/// Whole-run defaults plus per-issue rows from a recipe file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    parent: u32,
    agent: Agent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_issue",
        skip_serializing_if = "is_zero"
    )]
    issue: u32,
    #[serde(
        default = "default_base_branch",
        deserialize_with = "deserialize_base_branch",
        skip_serializing_if = "is_main"
    )]
    base_branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_mode: Option<PromptMode>,
    issues: Vec<RecipeIssue>,
}

/// One planned child in a recipe. `title` is documentary and ignored at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeIssue {
    number: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<Agent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_mode: Option<PromptMode>,
}

/// Inputs for [`Recipe::from_planned`].
pub struct GenerateSpec<'a> {
    /// Parent issue number stamped into the recipe.
    pub parent: u32,
    /// Resolved `owner/name` slug.
    pub repo: &'a str,
    /// Whole-run default agent.
    pub agent: Agent,
    /// `--issue` floor; omitted from YAML when `0`.
    pub issue: u32,
    /// Base branch; omitted from YAML when `main`.
    pub base_branch: &'a str,
    /// Whole-run model copied onto same-agent rows when present.
    pub model: Option<&'a str>,
    /// Whole-run effort copied onto same-agent rows when present.
    pub reasoning_effort: Option<&'a str>,
    /// Prompt file from generate CLI; canonicalized to an absolute path.
    pub prompt_file: Option<&'a Path>,
    /// Replace vs append for [`Self::prompt_file`].
    pub prompt_mode: Option<PromptMode>,
    /// Simulated solvable set in planned order.
    pub planned: &'a [GithubIssue],
}

/// Recipe with prompt snapshots and capability-checked profiles.
#[derive(Debug)]
pub struct PreparedRecipe {
    recipe: Recipe,
    run_wide_prompt: Option<(PromptMode, String)>,
    overrides: RunEphemeralProfileMap,
}

impl Recipe {
    /// Parses YAML, rejects unknown keys, and checks prompt pairing, absolute
    /// paths, and duplicate issue numbers.
    ///
    /// Prompt files are not read here; [`Recipe::prepare`] snapshots them.
    ///
    /// # Errors
    ///
    /// Returns a user-facing message when the document is not a valid recipe.
    pub fn from_yaml(text: &str) -> Result<Self, String> {
        let recipe: Self = serde_yaml::from_str(text)
            .map_err(|error| format!("nightshift: invalid recipe: {error}"))?;
        recipe.validate_schema()?;
        Ok(recipe)
    }

    /// Builds a valid recipe from a planned set and generate-time CLI values.
    ///
    /// # Errors
    ///
    /// Returns an error when the planned set is empty, prompt flags are paired
    /// incorrectly, or a prompt file cannot be canonicalized.
    pub fn from_planned(spec: GenerateSpec<'_>) -> Result<Self, String> {
        if spec.planned.is_empty() {
            return Err("nightshift: planned set is empty; nothing to write".to_string());
        }
        let (prompt_file, prompt_mode) = generate_prompt_pair(spec.prompt_file, spec.prompt_mode)?;
        let issues = spec
            .planned
            .iter()
            .map(|issue| RecipeIssue {
                number: issue.number,
                title: Some(issue.title.clone()),
                agent: Some(spec.agent),
                model: spec.model.map(str::to_string),
                reasoning_effort: spec.reasoning_effort.map(str::to_string),
                prompt_file: None,
                prompt_mode: None,
            })
            .collect();
        let recipe = Self {
            parent: spec.parent,
            agent: spec.agent,
            repo: Some(spec.repo.to_string()),
            issue: spec.issue,
            base_branch: spec.base_branch.to_string(),
            model: spec.model.map(str::to_string),
            reasoning_effort: spec.reasoning_effort.map(str::to_string),
            prompt_file,
            prompt_mode,
            issues,
        };
        recipe.validate_schema()?;
        Ok(recipe)
    }

    /// Serializes this recipe to YAML.
    ///
    /// # Errors
    ///
    /// Returns an error when the document cannot be encoded.
    pub fn to_yaml(&self) -> Result<String, String> {
        serde_yaml::to_string(self)
            .map_err(|error| format!("nightshift: failed to write recipe: {error}"))
    }

    /// Writes YAML to `path`. Fails when the path exists or is a directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination cannot be written.
    pub fn write_to(&self, path: &Path) -> Result<(), String> {
        if path.is_dir() {
            return Err(format!(
                "nightshift: --write-recipe path {} is a directory; pass a file path",
                path.display()
            ));
        }
        let yaml = self.to_yaml()?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut file| file.write_all(yaml.as_bytes()))
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    format!(
                        "nightshift: --write-recipe path {} already exists",
                        path.display()
                    )
                } else {
                    format!(
                        "nightshift: failed to write recipe {}: {error}",
                        path.display()
                    )
                }
            })
    }

    /// Loads prompt files, builds per-issue overrides, and capability-checks
    /// every resolved profile.
    ///
    /// # Errors
    ///
    /// Returns an error when a prompt file cannot be read or a resolved profile
    /// is illegal for its agent.
    pub fn prepare(self) -> Result<PreparedRecipe, String> {
        let run_wide_prompt = match (&self.prompt_file, self.prompt_mode) {
            (None, None) => None,
            (Some(path), Some(mode)) => Some((mode, load_directives(path)?)),
            _ => unreachable!("validate_schema rejects unpaired prompt fields"),
        };
        let mut overrides = RunEphemeralProfileMap::new();
        for row in &self.issues {
            let prompt = match (&row.prompt_file, row.prompt_mode) {
                (None, None) => None,
                (Some(path), Some(mode)) => Some(PerIssuePrompt {
                    mode,
                    contents: load_directives(path).map_err(|error| {
                        let detail = error.strip_prefix("nightshift: ").unwrap_or(&error);
                        format!("nightshift: recipe issue #{}: {detail}", row.number)
                    })?,
                }),
                _ => unreachable!("validate_schema rejects unpaired prompt fields"),
            };
            overrides.insert(
                row.number,
                PerIssueInvocationOverride {
                    agent: row.agent,
                    model: row.model.clone(),
                    reasoning_effort: row.reasoning_effort.clone(),
                    prompt,
                },
            );
        }
        let defaults = WholeRunInvocationDefaults {
            agent: self.agent,
            model: self.model.as_deref(),
            reasoning_effort: self.reasoning_effort.as_deref(),
        };
        defaults
            .agent
            .get_command_with_profile(resolve(defaults, None))
            .map_err(|error| {
                let detail = error.strip_prefix("nightshift: ").unwrap_or(&error);
                format!("nightshift: recipe: {detail}")
            })?;
        for row in &self.issues {
            let profile = resolve(defaults, overrides.get(&row.number));
            profile
                .agent
                .get_command_with_profile(profile)
                .map_err(|error| {
                    let detail = error.strip_prefix("nightshift: ").unwrap_or(&error);
                    format!("nightshift: recipe issue #{}: {detail}", row.number)
                })?;
        }
        Ok(PreparedRecipe {
            recipe: self,
            run_wide_prompt,
            overrides,
        })
    }

    fn validate_schema(&self) -> Result<(), String> {
        validate_prompt_pair(self.prompt_file.as_deref(), self.prompt_mode, "whole-run")?;
        let mut seen = HashSet::new();
        for row in &self.issues {
            if !seen.insert(row.number) {
                return Err(format!(
                    "nightshift: recipe has duplicate issue #{}",
                    row.number
                ));
            }
            validate_prompt_pair(
                row.prompt_file.as_deref(),
                row.prompt_mode,
                &format!("issue #{}", row.number),
            )?;
        }
        Ok(())
    }
}

impl PreparedRecipe {
    /// Reads `path`, validates the recipe, and snapshots prompt files.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or the recipe is invalid.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|_| format!("nightshift: failed to read recipe file: {}", path.display()))?;
        Recipe::from_yaml(&text)?.prepare()
    }

    /// Parent issue number from the recipe.
    pub fn parent(&self) -> u32 {
        self.recipe.parent
    }

    /// Issue floor from the recipe (`0` when omitted).
    pub fn issue(&self) -> u32 {
        self.recipe.issue
    }

    /// Optional `owner/name` slug from the recipe.
    pub fn repo(&self) -> Option<&str> {
        self.recipe.repo.as_deref()
    }

    /// Base branch from the recipe (`main` when omitted).
    pub fn base_branch(&self) -> &str {
        &self.recipe.base_branch
    }

    /// Whole-run agent, model, and effort from the recipe.
    pub fn whole_run_defaults(&self) -> WholeRunInvocationDefaults<'_> {
        WholeRunInvocationDefaults {
            agent: self.recipe.agent,
            model: self.recipe.model.as_deref(),
            reasoning_effort: self.recipe.reasoning_effort.as_deref(),
        }
    }

    /// Per-issue overrides keyed by child number, including inherit-only rows.
    pub fn per_issue_profiles(&self) -> &RunEphemeralProfileMap {
        &self.overrides
    }

    /// Run-wide directive policy snapshotted from the recipe prompt file.
    pub fn directive_policy(&self) -> DirectivePolicy<'_> {
        match &self.run_wide_prompt {
            None => DirectivePolicy::BuiltIn,
            Some((PromptMode::Replace, text)) => DirectivePolicy::Replace(text),
            Some((PromptMode::Append, text)) => DirectivePolicy::Append(text),
        }
    }
}

/// Requires recipe map keys to equal the planned-set issue numbers.
///
/// # Errors
///
/// Returns a diff when the sets differ.
pub fn assert_recipe_lock(
    profiles: &RunEphemeralProfileMap,
    planned: &[GithubIssue],
) -> Result<(), String> {
    let recipe: HashSet<u32> = profiles.keys().copied().collect();
    let plan: HashSet<u32> = planned.iter().map(|issue| issue.number).collect();
    if recipe == plan {
        return Ok(());
    }
    let mut missing: Vec<u32> = plan.difference(&recipe).copied().collect();
    let mut extra: Vec<u32> = recipe.difference(&plan).copied().collect();
    missing.sort_unstable();
    extra.sort_unstable();
    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!(
            "planned issues missing from recipe: {}",
            format_issue_list(&missing)
        ));
    }
    if !extra.is_empty() {
        parts.push(format!(
            "recipe issues not in the planned set: {}",
            format_issue_list(&extra)
        ));
    }
    Err(format!(
        "nightshift: recipe issue numbers do not match the planned set ({})",
        parts.join("; ")
    ))
}

/// Error when a live issue is not in a locked recipe map.
pub fn unknown_recipe_issue(number: u32) -> String {
    format!("nightshift: issue #{number} is not in the recipe; aborting")
}

/// Rejects planned issues that are missing from a locked recipe map.
///
/// Closed recipe rows may drop out of the live plan. A new planned number that
/// is not in the map is an error.
///
/// # Errors
///
/// Returns [`unknown_recipe_issue`] for the first planned number absent from
/// `profiles`.
pub fn assert_no_unknown_recipe_issues(
    profiles: &RunEphemeralProfileMap,
    planned: &[GithubIssue],
) -> Result<(), String> {
    match planned
        .iter()
        .find(|issue| !profiles.contains_key(&issue.number))
    {
        Some(issue) => Err(unknown_recipe_issue(issue.number)),
        None => Ok(()),
    }
}

fn format_issue_list(numbers: &[u32]) -> String {
    numbers
        .iter()
        .map(|number| format!("#{number}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_prompt_pair(
    path: Option<&Path>,
    mode: Option<PromptMode>,
    where_: &str,
) -> Result<(), String> {
    match (path, mode) {
        (None, None) => Ok(()),
        (Some(path), Some(_)) => require_absolute(path, where_),
        (Some(_), None) => Err(format!(
            "nightshift: recipe {where_} prompt_file requires prompt_mode"
        )),
        (None, Some(_)) => Err(format!(
            "nightshift: recipe {where_} prompt_mode requires prompt_file"
        )),
    }
}

fn require_absolute(path: &Path, where_: &str) -> Result<(), String> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(format!(
            "nightshift: recipe {where_} path {} must be absolute",
            path.display()
        ))
    }
}

fn generate_prompt_pair(
    path: Option<&Path>,
    mode: Option<PromptMode>,
) -> Result<(Option<PathBuf>, Option<PromptMode>), String> {
    match (path, mode) {
        (None, None) => Ok((None, None)),
        (Some(path), Some(mode)) => {
            load_directives(path)?;
            let absolute = std::path::absolute(path).map_err(|error| {
                format!(
                    "nightshift: failed to resolve prompt file {}: {error}",
                    path.display()
                )
            })?;
            Ok((Some(absolute), Some(mode)))
        }
        (Some(_), None) => {
            Err("nightshift: --write-recipe prompt_file requires prompt_mode".to_string())
        }
        (None, Some(_)) => {
            Err("nightshift: --write-recipe prompt_mode requires prompt_file".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GithubIssue;

    fn issue(number: u32, title: &str) -> GithubIssue {
        GithubIssue {
            number,
            title: title.to_string(),
            body: String::new(),
        }
    }

    fn missing_abs() -> String {
        if cfg!(windows) {
            r"C:\no\such\nightshift-recipe-prompt.md".to_string()
        } else {
            "/no/such/nightshift-recipe-prompt.md".to_string()
        }
    }

    #[test]
    fn from_yaml_accepts_minimal_valid_recipe() {
        let recipe = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
issues:
  - number: 42
    title: Add login
    agent: claude
"#,
        )
        .expect("minimal recipe should parse");
        assert_eq!(recipe.parent, 12);
        assert_eq!(recipe.agent, Agent::Claude);
        assert_eq!(recipe.issue, 0);
        assert_eq!(recipe.base_branch, "main");
        assert_eq!(recipe.issues.len(), 1);
        assert_eq!(recipe.issues[0].number, 42);
        assert_eq!(recipe.issues[0].title.as_deref(), Some("Add login"));
    }

    #[test]
    fn parent_is_the_only_parent_issue_recipe_key() {
        let recipe = Recipe::from_yaml("parent: 12\nagent: claude\nissues: []\n")
            .expect("parent key should parse");
        assert!(recipe.to_yaml().unwrap().contains("parent: 12"));
        assert!(Recipe::from_yaml("prd: 12\nagent: claude\nissues: []\n").is_err());
    }

    #[test]
    fn from_yaml_rejects_unknown_keys() {
        let error = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
tui: true
issues: []
"#,
        )
        .expect_err("unknown keys must fail");
        assert!(error.contains("invalid recipe"), "{error}");
        assert!(error.contains("tui"), "{error}");
    }

    #[test]
    fn from_yaml_requires_parent_agent_and_issues() {
        let error =
            Recipe::from_yaml("agent: claude\nissues: []\n").expect_err("missing parent must fail");
        assert!(error.contains("invalid recipe"), "{error}");

        let error =
            Recipe::from_yaml("parent: 1\nissues: []\n").expect_err("missing agent must fail");
        assert!(error.contains("invalid recipe"), "{error}");

        let error =
            Recipe::from_yaml("parent: 1\nagent: claude\n").expect_err("missing issues must fail");
        assert!(error.contains("invalid recipe"), "{error}");
    }

    #[test]
    fn from_yaml_rejects_open_code_kebab() {
        let error = Recipe::from_yaml(
            r#"
parent: 1
agent: open-code
issues: []
"#,
        )
        .expect_err("open-code is not the clap value name");
        assert!(error.contains("invalid recipe"), "{error}");
    }

    #[test]
    fn from_yaml_accepts_opencode() {
        let recipe = Recipe::from_yaml(
            r#"
parent: 1
agent: opencode
issues: []
"#,
        )
        .expect("opencode is the clap value name");
        assert_eq!(recipe.agent, Agent::OpenCode);
    }

    #[test]
    fn from_yaml_treats_null_like_omit() {
        let recipe = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
model: null
reasoning_effort: null
issues:
  - number: 42
    agent: claude
    model: null
    reasoning_effort: null
"#,
        )
        .expect("null optional fields should parse as omit");
        assert!(recipe.model.is_none());
        assert!(recipe.reasoning_effort.is_none());
        assert!(recipe.issues[0].model.is_none());
        assert!(recipe.issues[0].reasoning_effort.is_none());
        assert_eq!(recipe.issue, 0);
        assert_eq!(recipe.base_branch, "main");
    }

    #[test]
    fn from_yaml_treats_null_issue_and_base_branch_like_omit() {
        let recipe = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
issue: null
base_branch: null
issues: []
"#,
        )
        .expect("null issue and base_branch should parse as omit");
        assert_eq!(recipe.issue, 0);
        assert_eq!(recipe.base_branch, "main");
    }

    #[test]
    fn from_yaml_rejects_prompt_file_without_mode() {
        let path = missing_abs();
        let error = Recipe::from_yaml(&format!(
            "parent: 12\nagent: claude\nprompt_file: '{path}'\nissues: []\n"
        ))
        .expect_err("prompt_file requires prompt_mode");
        assert!(
            error.contains("prompt_file requires prompt_mode"),
            "{error}"
        );
    }

    #[test]
    fn from_yaml_rejects_prompt_mode_without_file() {
        let error = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
prompt_mode: append
issues: []
"#,
        )
        .expect_err("prompt_mode requires prompt_file");
        assert!(
            error.contains("prompt_mode requires prompt_file"),
            "{error}"
        );
    }

    #[test]
    fn from_yaml_rejects_relative_prompt_path() {
        let error = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
prompt_file: ./directives.md
prompt_mode: replace
issues: []
"#,
        )
        .expect_err("relative paths must fail");
        assert!(error.contains("must be absolute"), "{error}");
    }

    #[test]
    fn from_yaml_rejects_tilde_prompt_path() {
        let error = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
prompt_file: ~/directives.md
prompt_mode: replace
issues: []
"#,
        )
        .expect_err("tilde is not absolute");
        assert!(error.contains("must be absolute"), "{error}");
    }

    #[test]
    fn from_yaml_rejects_duplicate_issue_numbers() {
        let error = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
issues:
  - number: 42
  - number: 42
"#,
        )
        .expect_err("duplicate numbers must fail");
        assert!(error.contains("duplicate issue #42"), "{error}");
    }

    #[test]
    fn from_yaml_rejects_per_issue_prompt_mode_without_file() {
        let error = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
issues:
  - number: 42
    prompt_mode: replace
"#,
        )
        .expect_err("per-issue prompt_mode requires prompt_file");
        assert!(error.contains("issue #42"), "{error}");
        assert!(
            error.contains("prompt_mode requires prompt_file"),
            "{error}"
        );
    }

    #[test]
    fn prepare_fails_when_prompt_file_missing() {
        let path = missing_abs();
        let recipe = Recipe::from_yaml(&format!(
            "parent: 12\nagent: claude\nprompt_file: '{path}'\nprompt_mode: replace\nissues: []\n"
        ))
        .expect("absolute missing path is schema-valid");
        let error = recipe
            .prepare()
            .expect_err("missing prompt file must fail before the run");
        assert!(error.contains("failed to read prompt file"), "{error}");
    }

    #[test]
    fn prepare_accepts_antigravity_model_and_effort() {
        let recipe = Recipe::from_yaml(
            r#"
parent: 12
agent: antigravity
model: gemini
reasoning_effort: high
issues:
  - number: 42
    agent: antigravity
"#,
        )
        .expect("schema allows the fields");
        let prepared = recipe
            .prepare()
            .expect("antigravity accepts model and effort");
        let defaults = prepared.whole_run_defaults();
        let profile = resolve(defaults, prepared.per_issue_profiles().get(&42));
        assert_eq!(profile.model, Some("gemini"));
        assert_eq!(profile.reasoning_effort, Some("high"));
    }

    #[test]
    fn prepare_rejects_cursor_effort_on_row() {
        let recipe = Recipe::from_yaml(
            r#"
parent: 12
agent: claude
issues:
  - number: 42
    agent: cursor
    reasoning_effort: high
"#,
        )
        .expect("schema allows the field");
        let error = recipe.prepare().expect_err("cursor + effort must fail");
        assert!(error.contains("issue #42"), "{error}");
        assert!(
            error.contains("does not support --reasoning-effort"),
            "{error}"
        );
    }

    #[test]
    fn prepare_accepts_opencode_passthrough_variant() {
        Recipe::from_yaml(
            r#"
parent: 12
agent: opencode
reasoning_effort: custom-variant
issues:
  - number: 42
    agent: opencode
"#,
        )
        .expect("schema")
        .prepare()
        .expect("OpenCode variants pass through");
    }

    #[test]
    fn from_planned_fails_on_empty_set() {
        let error = Recipe::from_planned(GenerateSpec {
            parent: 12,
            repo: "owner/name",
            agent: Agent::Claude,
            issue: 0,
            base_branch: "main",
            model: None,
            reasoning_effort: None,
            prompt_file: None,
            prompt_mode: None,
            planned: &[],
        })
        .expect_err("empty plan must not write");
        assert!(error.contains("planned set is empty"), "{error}");
    }

    #[test]
    fn from_planned_stamps_repo_and_omits_defaults() {
        let planned = [issue(42, "Add login"), issue(43, "Add logout")];
        let recipe = Recipe::from_planned(GenerateSpec {
            parent: 12,
            repo: "owner/name",
            agent: Agent::Claude,
            issue: 0,
            base_branch: "main",
            model: Some("claude-opus-5"),
            reasoning_effort: Some("high"),
            prompt_file: None,
            prompt_mode: None,
            planned: &planned,
        })
        .expect("non-empty plan");
        let yaml = recipe.to_yaml().expect("encode");
        assert!(yaml.contains("parent: 12"), "{yaml}");
        assert!(yaml.contains("repo: owner/name"), "{yaml}");
        assert!(yaml.contains("agent: claude"), "{yaml}");
        assert!(yaml.contains("model: claude-opus-5"), "{yaml}");
        assert!(yaml.contains("reasoning_effort: high"), "{yaml}");
        assert!(!yaml.contains("issue:"), "{yaml}");
        assert!(!yaml.contains("base_branch:"), "{yaml}");
        assert!(yaml.contains("number: 42"), "{yaml}");
        assert!(yaml.contains("Add login"), "{yaml}");
        assert!(yaml.contains("number: 43"), "{yaml}");
        let roundtrip = Recipe::from_yaml(&yaml).expect("generated YAML must parse");
        assert_eq!(roundtrip.issues[0].agent, Some(Agent::Claude));
        assert_eq!(roundtrip.issues[0].model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn from_planned_omits_unset_model_and_effort() {
        let planned = [issue(1, "One")];
        let yaml = Recipe::from_planned(GenerateSpec {
            parent: 9,
            repo: "o/r",
            agent: Agent::Pi,
            issue: 4,
            base_branch: "develop",
            model: None,
            reasoning_effort: None,
            prompt_file: None,
            prompt_mode: None,
            planned: &planned,
        })
        .expect("plan")
        .to_yaml()
        .expect("encode");
        assert!(yaml.contains("issue: 4"), "{yaml}");
        assert!(yaml.contains("base_branch: develop"), "{yaml}");
        assert!(!yaml.contains("model:"), "{yaml}");
        assert!(!yaml.contains("reasoning_effort:"), "{yaml}");
        assert!(!yaml.contains("prompt_file:"), "{yaml}");
    }

    #[test]
    fn from_planned_stamps_absolute_prompt_file_and_mode() {
        let dir = std::env::temp_dir().join(format!(
            "nightshift-recipe-gen-prompt-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let prompt = dir.join("directives.md");
        std::fs::write(&prompt, "extra\n").expect("prompt file");
        let planned = [issue(1, "One")];
        let recipe = Recipe::from_planned(GenerateSpec {
            parent: 12,
            repo: "o/r",
            agent: Agent::Claude,
            issue: 0,
            base_branch: "main",
            model: None,
            reasoning_effort: None,
            prompt_file: Some(prompt.as_path()),
            prompt_mode: Some(PromptMode::Replace),
            planned: &planned,
        })
        .expect("generate with prompt file");
        let yaml = recipe.to_yaml().expect("encode");
        assert!(yaml.contains("prompt_mode: replace"), "{yaml}");
        assert!(
            recipe
                .prompt_file
                .as_ref()
                .is_some_and(|path| path.is_absolute()),
            "{yaml}"
        );
        assert!(!yaml.contains(r"\\?\"), "{yaml}");
        let _ = std::fs::remove_file(&prompt);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn write_to_fails_when_path_exists() {
        let planned = [issue(1, "One")];
        let recipe = Recipe::from_planned(GenerateSpec {
            parent: 1,
            repo: "o/r",
            agent: Agent::Pi,
            issue: 0,
            base_branch: "main",
            model: None,
            reasoning_effort: None,
            prompt_file: None,
            prompt_mode: None,
            planned: &planned,
        })
        .expect("plan");
        let path = std::env::temp_dir().join(format!(
            "nightshift-recipe-exists-{}-{}",
            std::process::id(),
            "write.yaml"
        ));
        std::fs::write(&path, "already\n").expect("seed existing file");
        let error = recipe.write_to(&path).expect_err("must not overwrite");
        let _ = std::fs::remove_file(&path);
        assert!(error.contains("already exists"), "{error}");
    }

    #[test]
    fn write_to_fails_when_path_is_directory() {
        let planned = [issue(1, "One")];
        let recipe = Recipe::from_planned(GenerateSpec {
            parent: 1,
            repo: "o/r",
            agent: Agent::Pi,
            issue: 0,
            base_branch: "main",
            model: None,
            reasoning_effort: None,
            prompt_file: None,
            prompt_mode: None,
            planned: &planned,
        })
        .expect("plan");
        let error = recipe
            .write_to(std::env::temp_dir().as_path())
            .expect_err("directory must fail");
        assert!(error.contains("directory"), "{error}");
    }

    #[test]
    fn default_write_path_uses_parent_number() {
        assert_eq!(
            default_write_path(12),
            PathBuf::from("parent-12-recipe.yaml")
        );
    }

    #[test]
    fn assert_recipe_lock_requires_exact_number_match() {
        let mut profiles = RunEphemeralProfileMap::new();
        profiles.insert(42, PerIssueInvocationOverride::default());
        profiles.insert(43, PerIssueInvocationOverride::default());
        assert_recipe_lock(&profiles, &[issue(42, "a"), issue(43, "b")]).expect("exact match");

        let error =
            assert_recipe_lock(&profiles, &[issue(42, "a"), issue(43, "b"), issue(44, "c")])
                .expect_err("missing from recipe");
        assert!(error.contains("#44"), "{error}");
        assert!(error.contains("missing from recipe"), "{error}");

        let error = assert_recipe_lock(&profiles, &[issue(42, "a")]).expect_err("extra in recipe");
        assert!(error.contains("#43"), "{error}");
        assert!(error.contains("not in the planned set"), "{error}");
    }

    #[test]
    fn prepare_loads_prompt_file_and_builds_overrides() {
        let dir =
            std::env::temp_dir().join(format!("nightshift-recipe-prompt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let prompt = dir.join("directives.md");
        std::fs::write(&prompt, "  extra instructions  \n").expect("prompt file");
        let yaml = format!(
            "parent: 12\nagent: claude\nprompt_file: '{}'\nprompt_mode: append\nissues:\n  - number: 42\n    agent: claude\n",
            prompt.display()
        );
        let prepared = Recipe::from_yaml(&yaml)
            .expect("schema")
            .prepare()
            .expect("readable prompt");
        assert_eq!(prepared.parent(), 12);
        match prepared.directive_policy() {
            DirectivePolicy::Append(text) => assert_eq!(text, "extra instructions"),
            other => panic!("expected append, got {other:?}"),
        }
        let _ = std::fs::remove_file(&prompt);
        let _ = std::fs::remove_dir(&dir);
    }
}
