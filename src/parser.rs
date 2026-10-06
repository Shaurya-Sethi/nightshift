//! Selects the next child issue of a requested parent from native GitHub relationship JSON.
//!
//! Membership requires `parent.number` to equal the requested issue number
//! (direct children only). Ordering uses `blockedBy.nodes[].state`: an issue is
//! ready when every blocker node is closed. Issue bodies are not parsed.

use std::collections::HashSet;
use std::error::Error;

use serde::Deserialize;

use crate::github::GithubIssue;

#[derive(Debug, Deserialize)]
struct ListedIssue {
    number: u32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    parent: Option<ParentRef>,
    #[serde(rename = "blockedBy", default)]
    blocked_by: BlockedBy,
}

#[derive(Debug, Deserialize)]
struct ParentRef {
    number: u32,
}

#[derive(Debug, Default, Deserialize)]
struct BlockedBy {
    #[serde(default)]
    nodes: Vec<BlockerNode>,
}

#[derive(Debug, Deserialize)]
struct BlockerNode {
    number: u32,
    #[serde(default)]
    state: String,
}

/// Planned vs leftover issues for a dry-run of the live selection loop.
pub struct IssuePlan {
    /// Issues that would be solved, in loop order.
    pub planned: Vec<GithubIssue>,
    /// Open candidates that simulation never reaches because blockers stay open.
    pub blocked: Vec<GithubIssue>,
    /// True when any direct child of the requested parent exists, including those below `--issue`.
    pub has_open_children: bool,
}

fn parse_issues(json: &str) -> Result<Vec<ListedIssue>, Box<dyn Error>> {
    serde_json::from_str(json)
        .map_err(|err| format!("nightshift: failed to parse issue list: {err}").into())
}

fn to_github_issue(issue: &ListedIssue) -> GithubIssue {
    GithubIssue {
        number: issue.number,
        title: issue.title.clone(),
        body: issue.body.clone().unwrap_or_default(),
    }
}

fn is_parent_child(issue: &ListedIssue, parent_number: u32) -> bool {
    issue
        .parent
        .as_ref()
        .is_some_and(|parent| parent.number == parent_number)
}

fn is_ready(issue: &ListedIssue, simulated_closed: &HashSet<u32>) -> bool {
    issue.blocked_by.nodes.iter().all(|blocker| {
        simulated_closed.contains(&blocker.number) || blocker.state.eq_ignore_ascii_case("closed")
    })
}

fn parent_slice(issues: &[ListedIssue], parent: u32, min_issue: u32) -> (Vec<&ListedIssue>, bool) {
    let mut candidates = Vec::new();
    let mut has_open_children = false;
    for issue in issues {
        if is_parent_child(issue, parent) {
            has_open_children = true;
            if issue.number >= min_issue {
                candidates.push(issue);
            }
        }
    }
    (candidates, has_open_children)
}

/// Simulates the live loop: repeatedly pick the lowest ready child until none remain.
///
/// Issues left in [`IssuePlan::blocked`] have blockers that never close during
/// simulation (open external blockers or cycles among remaining candidates).
///
/// # Errors
///
/// Returns an error when `json` is not a GitHub issue-list array.
///
/// # Examples
///
/// ```rust
/// let json = r#"[{"number":11,"title":"Second","parent":{"number":42},"blockedBy":{"nodes":[{"number":10,"state":"OPEN"}]}},{"number":10,"title":"First","parent":{"number":42},"blockedBy":{"nodes":[]}}]"#;
/// let plan = nightshift::parser::plan_order(json, 42, 0).unwrap();
/// assert_eq!(plan.planned[0].number, 10);
/// assert_eq!(plan.planned[1].number, 11);
/// assert!(plan.blocked.is_empty());
/// ```
pub fn plan_order(json: &str, parent: u32, min_issue: u32) -> Result<IssuePlan, Box<dyn Error>> {
    let issues = parse_issues(json)?;
    let (mut remaining, has_open_children) = parent_slice(&issues, parent, min_issue);
    remaining.sort_by_key(|issue| issue.number);

    let mut planned = Vec::new();
    let mut simulated_closed = HashSet::new();

    loop {
        let picked_idx = remaining
            .iter()
            .position(|issue| is_ready(issue, &simulated_closed));
        let Some(idx) = picked_idx else {
            break;
        };
        let issue = remaining.remove(idx);
        simulated_closed.insert(issue.number);
        planned.push(to_github_issue(issue));
    }

    Ok(IssuePlan {
        planned,
        blocked: remaining
            .iter()
            .map(|issue| to_github_issue(issue))
            .collect(),
        has_open_children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph(issues: &[serde_json::Value]) -> String {
        serde_json::Value::Array(issues.to_vec()).to_string()
    }

    fn first_planned(json: &str, parent: u32, min_issue: u32) -> Option<u32> {
        plan_order(json, parent, min_issue)
            .unwrap()
            .planned
            .into_iter()
            .next()
            .map(|issue| issue.number)
    }

    fn child(
        number: u32,
        parent: Option<u32>,
        blockers: &[(u32, &str)],
        body: &str,
    ) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> = blockers
            .iter()
            .map(|(blocker, state)| {
                json!({
                    "number": blocker,
                    "state": state,
                    "title": format!("Issue {blocker}"),
                    "url": "https://example.invalid"
                })
            })
            .collect();
        let total = nodes.len();
        json!({
            "number": number,
            "title": format!("Child {number}"),
            "body": body,
            "parent": parent.map(|parent| json!({ "number": parent, "title": "Parent" })),
            "blockedBy": { "nodes": nodes, "totalCount": total }
        })
    }

    #[test]
    fn blocker_closed_issue_is_selectable() {
        let json = graph(&[child(10, Some(42), &[(7, "CLOSED")], "Do the work.")]);
        let plan = plan_order(&json, 42, 0).unwrap();
        assert_eq!(plan.planned[0].number, 10);
        assert_eq!(plan.planned[0].body, "Do the work.");
        assert!(!json.contains("## Parent"));
        assert!(!json.contains("## Blocked by"));
    }

    #[test]
    fn blocker_open_issue_is_not_selectable() {
        let json = graph(&[child(10, Some(42), &[(7, "OPEN")], "Do the work.")]);
        assert_eq!(first_planned(&json, 42, 0), None);
    }

    #[test]
    fn two_ready_candidates_lower_number_wins() {
        let json = graph(&[
            child(20, Some(42), &[], "Later."),
            child(15, Some(42), &[], "Earlier."),
        ]);
        assert_eq!(first_planned(&json, 42, 0), Some(15));
    }

    #[test]
    fn external_open_blocker_is_not_selectable() {
        let json = graph(&[child(
            10,
            Some(42),
            &[(99, "OPEN")],
            "Depends on outside work.",
        )]);
        let plan = plan_order(&json, 42, 0).unwrap();
        assert!(plan.planned.is_empty());
        assert_eq!(plan.blocked[0].number, 10);
    }

    #[test]
    fn external_closed_blocker_is_selectable() {
        let json = graph(&[child(
            10,
            Some(42),
            &[(99, "CLOSED")],
            "Outside work is done.",
        )]);
        assert_eq!(first_planned(&json, 42, 0), Some(10));
    }

    #[test]
    fn ready_set_empty_open_children_remain_does_not_spin() {
        let json = graph(&[
            child(10, Some(42), &[(11, "OPEN")], "A"),
            child(11, Some(42), &[(10, "OPEN")], "B"),
        ]);
        let plan = plan_order(&json, 42, 0).unwrap();
        assert!(plan.planned.is_empty());
        assert_eq!(plan.blocked.len(), 2);
    }

    #[test]
    fn parent_with_no_children_returns_empty_plan() {
        let json = graph(&[child(10, Some(99), &[], "Other Parent.")]);
        let plan = plan_order(&json, 42, 0).unwrap();
        assert!(!plan.has_open_children);
        assert!(plan.planned.is_empty());
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn empty_list_is_no_children() {
        let plan = plan_order("[]", 42, 0).unwrap();
        assert!(!plan.has_open_children);
        assert!(plan.planned.is_empty());
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn issue_floor_excludes_child_but_records_open_work() {
        let json = graph(&[
            child(5, Some(42), &[], "Below floor."),
            child(10, Some(42), &[], "At floor."),
            child(11, Some(99), &[], "Other Parent."),
            child(12, Some(42), &[], "Above floor."),
        ]);
        let plan = plan_order(&json, 42, 10).unwrap();
        assert!(plan.has_open_children);
        assert_eq!(
            plan.planned
                .iter()
                .map(|issue| issue.number)
                .collect::<Vec<_>>(),
            vec![10, 12]
        );
    }

    #[test]
    fn plan_order_respects_blocker_chain() {
        let json = graph(&[
            child(11, Some(42), &[(10, "OPEN")], "Second."),
            child(10, Some(42), &[], "First."),
        ]);
        let plan = plan_order(&json, 42, 0).unwrap();
        assert_eq!(
            plan.planned
                .iter()
                .map(|issue| issue.number)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn grandchild_is_not_a_member() {
        let json = graph(&[child(10, Some(7), &[], "This issue has another parent.")]);
        let plan = plan_order(&json, 42, 0).unwrap();
        assert!(!plan.has_open_children);
        assert!(plan.planned.is_empty());
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn body_parent_and_blockers_are_ignored() {
        let claiming_other_parent = graph(&[child(
            10,
            Some(42),
            &[],
            "## Parent\n#99\n\n## Blocked by\n#7\n",
        )]);
        assert_eq!(first_planned(&claiming_other_parent, 42, 0), Some(10));
        assert_eq!(first_planned(&claiming_other_parent, 99, 0), None);

        let body_only_member = graph(&[child(11, None, &[], "## Parent\n#42\n")]);
        let plan = plan_order(&body_only_member, 42, 0).unwrap();
        assert!(!plan.has_open_children);
        assert!(plan.planned.is_empty());
    }

    #[test]
    fn malformed_json_is_an_error() {
        let err = plan_order("not-json", 42, 0)
            .err()
            .expect("malformed json should fail");
        assert!(err.to_string().contains("failed to parse issue list"));
    }

    #[test]
    fn closed_state_is_case_insensitive() {
        let json = graph(&[child(10, Some(42), &[(7, "closed")], "Done blocker.")]);
        assert_eq!(first_planned(&json, 42, 0), Some(10));
    }
}
