//! Test-only fake agent CLI used by process-runner integration tests.
//!
//! Mode is selected with `NIGHTSHIFT_FAKE_MODE`:
//! - `passthrough` (default): write argv (except program name) to `NIGHTSHIFT_FAKE_ARGS_FILE`, drain stdin
//! - `stderr_fail`: print a fixed message to stderr and exit 23
//! - `early_close`: print model-rejection stderr, exit 2 quickly so a large stdin write can fail

use std::env;
use std::fs;
use std::io::{self, Write, copy, stderr};
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

fn main() {
    match env::var("NIGHTSHIFT_FAKE_MODE").as_deref() {
        Ok("passthrough") | Err(_) => passthrough(),
        Ok("stderr_fail") => stderr_fail(),
        Ok("early_close") => early_close(),
        Ok(other) => {
            let _ = writeln!(stderr(), "unknown NIGHTSHIFT_FAKE_MODE: {other}");
            exit(1);
        }
    }
}
