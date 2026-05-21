use regex::Regex;

pub fn extract_section_issue_numbers(body: &str, section_name: &str) -> Vec<u32> {
    let mut numbers = Vec::new();
    let mut capturing: bool = false;

    let re_num = Regex::new(r"#([0-9]+)").unwrap();
    let re_header = Regex::new(r"^#{1,6}\s").unwrap();

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

pub fn extract_parent_prd(body: &str) -> Option<u32> {
    extract_section_issue_numbers(body, "parent")
        .into_iter()
        .next()
}

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
