//! `--tui` must fail on a non-TTY before GitHub or git work.

use std::process::{Command, Stdio};

#[test]
fn tui_without_tty_fails_before_github_or_git() {
    let exe = env!("CARGO_BIN_EXE_nightshift");
    let output = Command::new(exe)
        .args(["--tui", "--prd", "1", "--agent", "pi"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("nightshift should spawn");
    assert!(
        !output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--tui requires an interactive TTY"),
        "{stderr}"
    );
    let lower = stderr.to_ascii_lowercase();
    assert!(
        !lower.contains("failed to execute gh") && !lower.contains("not inside a git"),
        "must fail before GitHub or git: {stderr}"
    );
}
