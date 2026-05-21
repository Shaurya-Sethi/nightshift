//! Parses the structured markdown contract used to connect PRDs, child issues,
//! and blockers.
//!
//! Child issue bodies declare their PRD parent in a `Parent` section and their
//! dependencies in a `Blocked by` section. Section matching is case-insensitive
//! and currently substring-based, so a line containing `transparent` can start a
//! `parent` capture, and a line containing `unblocked by` can start a
//! `blocked by` capture. Once a section is found, issue numbers of the form
//! `#123` are captured from that line and following lines until the next
//! markdown header that starts with one to six `#` characters followed by a
//! space.
//!
//! [`crate::parser::extract_parent_prd`] returns the first captured parent
//! number because the orchestrator treats a child issue as belonging to a single
//! PRD. [`crate::parser::extract_blockers`] returns every blocker number in
//! encounter order, preserving order across multiple numbers on one line and
//! across multiple lines. Fenced code blocks are not special to the parser, so
//! `#123` inside a captured section is treated the same as any other issue
//! reference until the next markdown header.

use regex::Regex;

/// Extracts issue numbers from a named issue-body section.
///
/// Use this when adding parser behavior that needs the same section-capture
/// rules as the orchestrator: case-insensitive substring section matching,
/// capture from the matching line, and stop at the next markdown header.
/// Numbers are returned in the order their `#123` references appear.
///
/// # Examples
///
/// ```rust
/// let body = "## Blocked by\n#10 and #11\n\n## Notes\n#99";
///
/// assert_eq!(
///     nightshift::parser::extract_section_issue_numbers(body, "blocked by"),
///     vec![10, 11]
/// );
/// ```
pub fn extract_section_issue_numbers(body: &str, section_name: &str) -> Vec<u32> {
    let mut numbers = Vec::new();
    let mut capturing: bool = false;

    let re_num = Regex::new(r"#([0-9]+)").unwrap();
    let re_header = Regex::new(r"^#{1,6}\s").unwrap();

    // Keep number scanning shared so the matching header line and captured body
    // lines follow identical `#123` extraction rules.
    let mut scan_line = |line: &str| {
        for cap in re_num.captures_iter(line) {
            if let Ok(num) = cap[1].parse::<u32>() {
                numbers.push(num);
            }
        }
    };

    for line in body.lines() {
        if line.to_lowercase().contains(&section_name.to_lowercase()) {
            capturing = true;
            scan_line(line);
            continue;
        }

        if capturing && re_header.is_match(line) {
            capturing = false;
        }

        if capturing {
            scan_line(line);
        }
    }
    numbers
}

/// Returns the PRD issue number declared by a child issue body.
///
/// Nightshift uses this to decide whether an open issue belongs under the target
/// PRD. If multiple numbers are captured from the `Parent` section, the first
/// number wins. Returns [`None`] when no matching section or number is found.
///
/// # Examples
///
/// ```rust
/// let body = "## Parent\n#42\n#99\n";
///
/// assert_eq!(nightshift::parser::extract_parent_prd(body), Some(42));
/// ```
pub fn extract_parent_prd(body: &str) -> Option<u32> {
    extract_section_issue_numbers(body, "parent")
        .into_iter()
        .next()
}

/// Returns blocker issue numbers declared by a child issue body.
///
/// Nightshift checks these issue numbers before selecting a candidate for agent
/// work. The order of `#123` references is preserved, including multiple
/// references on one line.
///
/// # Examples
///
/// ```rust
/// let body = "## Blocked by\n#3\n#1, #2\n";
///
/// assert_eq!(nightshift::parser::extract_blockers(body), vec![3, 1, 2]);
/// ```
pub fn extract_blockers(body: &str) -> Vec<u32> {
    extract_section_issue_numbers(body, "blocked by")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_h2_header_first_number_wins() {
        let body = r#"## Parent
#42
#99
"#;
        assert_eq!(extract_parent_prd(body), Some(42));
    }

    #[test]
    fn parent_h3_lowercase_header() {
        let body = r#"### parent
#7
"#;
        assert_eq!(extract_parent_prd(body), Some(7));
    }

    #[test]
    fn parent_inline_on_same_line() {
        let body = "Parent: #42\n\nSome description.\n";
        assert_eq!(extract_parent_prd(body), Some(42));
    }

    #[test]
    fn parent_case_insensitive_section_name() {
        let body = r#"## PARENT
#42
"#;
        assert_eq!(extract_parent_prd(body), Some(42));
    }

    #[test]
    fn parent_stops_at_next_markdown_header() {
        let body = r#"## Parent
#42

## Notes
#99 is unrelated
"#;
        assert_eq!(extract_parent_prd(body), Some(42));
        assert_eq!(extract_section_issue_numbers(body, "parent"), vec![42]);
    }

    #[test]
    fn parent_missing_section_returns_none() {
        let body = "## Task\n\nImplement the feature.\n";
        assert_eq!(extract_parent_prd(body), None);
    }

    #[test]
    fn parent_empty_body_returns_none() {
        assert_eq!(extract_parent_prd(""), None);
        assert!(extract_blockers("").is_empty());
    }

    #[test]
    fn parent_wrong_prd_still_parsed_for_orchestrator_filter() {
        let body = r#"## Parent
#99
"#;
        assert_eq!(extract_parent_prd(body), Some(99));
    }

    #[test]
    fn blockers_h2_multiple_on_one_line_order_preserved() {
        let body = r#"## Parent
#42

## Blocked by
#10 and #11
"#;
        assert_eq!(extract_blockers(body), vec![10, 11]);
    }

    #[test]
    fn blockers_multiple_lines_order_preserved() {
        let body = r#"## Parent
#42

## Blocked by
#3
#1
#2
"#;
        assert_eq!(extract_blockers(body), vec![3, 1, 2]);
    }

    #[test]
    fn blockers_case_insensitive_section_name() {
        let body = r#"## BLOCKED BY
#5
"#;
        assert_eq!(extract_blockers(body), vec![5]);
    }

    #[test]
    fn blockers_stops_at_next_header() {
        let body = r#"## Blocked by
#10

## Acceptance criteria
#999
"#;
        assert_eq!(extract_blockers(body), vec![10]);
    }

    #[test]
    fn blockers_missing_section_empty() {
        let body = r#"## Parent
#42
"#;
        assert!(extract_blockers(body).is_empty());
    }

    #[test]
    fn blockers_substring_in_unrelated_line_starts_capture() {
        // Current matcher is substring-based: "unblocked by design" contains "blocked by".
        let body = r#"Discussion: unblocked by design after #77

## Parent
#42
"#;
        assert_eq!(extract_blockers(body), vec![77]);
    }

    #[test]
    fn parent_substring_in_unrelated_word_starts_capture() {
        // "transparent" contains "parent"; numbers on that line are captured.
        let body = r#"transparent layer tracks #55

## Parent
#42
"#;
        assert_eq!(extract_parent_prd(body), Some(55));
    }

    #[test]
    fn hash_in_fenced_code_inside_parent_section_before_next_header() {
        let body = r#"## Parent
#42

```
issue #999 in code fence
```

## Blocked by
#10
"#;
        assert_eq!(extract_parent_prd(body), Some(42));
        assert_eq!(extract_section_issue_numbers(body, "parent"), vec![42, 999]);
        assert_eq!(extract_blockers(body), vec![10]);
    }

    #[test]
    fn realistic_child_issue_body() {
        let body = r#"# Add login API

## Parent
#42

## Blocked by
#10, #11

## Description
Depends on auth schema from #10.

## Acceptance criteria
- [ ] Endpoint returns 401 without token
"#;
        assert_eq!(extract_parent_prd(body), Some(42));
        assert_eq!(extract_blockers(body), vec![10, 11]);
    }
}
