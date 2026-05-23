//! Test-only fake agent CLI used by process-runner integration tests.
//!
//! Mode is selected with `NIGHTSHIFT_FAKE_MODE`:
//! - `passthrough` (default): write argv (except program name) to `NIGHTSHIFT_FAKE_ARGS_FILE`, drain stdin
//! - `stderr_fail`: write to stderr and exit 23
//! - `early_close`: exit 2 quickly so a large stdin write can fail
//! - `noisy_success`: write heavily to stdout/stderr, drain stdin, exit 0

use std::env;
use std::fs;
use std::io::{self, Write, copy, stderr, stdout};
use std::process::exit;

fn drain_stdin() {
    let _ = copy(&mut io::stdin(), &mut io::sink());
}

fn passthrough() {
    let args_path = env::var("NIGHTSHIFT_FAKE_ARGS_FILE").unwrap_or_else(|_| {
        eprintln!("NIGHTSHIFT_FAKE_ARGS_FILE is required for passthrough mode");
        exit(1);
    });
    let args: Vec<_> = env::args().skip(1).collect();
    let contents = args.join("\n");
    if let Err(err) = fs::write(&args_path, format!("{contents}\n")) {
        eprintln!("failed to write args file: {err}");
        exit(1);
    }
    drain_stdin();
}

fn stderr_fail() {
    drain_stdin();
    let _ = writeln!(stderr(), "cli said no");
    exit(23);
}

fn early_close() {
    let _ = writeln!(stderr(), "unknown option: --model sonnet");
    exit(2);
}

const NOISY_MARKER: &str = "NIGHTSHIFT_FAKE_NOISY_OUTPUT";

fn noisy_success() {
    drain_stdin();
    for _ in 0..512 {
        let _ = writeln!(stdout(), "{NOISY_MARKER}");
        let _ = writeln!(stderr(), "{NOISY_MARKER}");
    }
}

fn main() {
    match env::var("NIGHTSHIFT_FAKE_MODE").as_deref() {
        Ok("passthrough") | Err(_) => passthrough(),
        Ok("stderr_fail") => stderr_fail(),
        Ok("early_close") => early_close(),
        Ok("noisy_success") => noisy_success(),
        Ok(other) => {
            let _ = writeln!(stderr(), "unknown NIGHTSHIFT_FAKE_MODE: {other}");
            exit(1);
        }
    }
}
