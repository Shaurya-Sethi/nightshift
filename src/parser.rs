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
