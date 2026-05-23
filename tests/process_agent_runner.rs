//! Cross-platform integration tests for [`nightshift::agent::ProcessAgentRunner`].

use nightshift::agent::{Agent, AgentRunner, ProcessAgentRunner};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct TestCommandEnv {
    original_path: OsString,
    saved_fake_mode: Option<OsString>,
    saved_fake_args_file: Option<OsString>,
    temp_dir: PathBuf,
}

impl Drop for TestCommandEnv {
    fn drop(&mut self) {
        restore_var("PATH", &Some(self.original_path.clone()));
        restore_var("NIGHTSHIFT_FAKE_MODE", &self.saved_fake_mode);
        restore_var("NIGHTSHIFT_FAKE_ARGS_FILE", &self.saved_fake_args_file);
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn restore_var(name: &str, previous: &Option<OsString>) {
    match previous {
        Some(value) => unsafe {
            env::set_var(name, value);
        },
        None => unsafe {
            env::remove_var(name);
        },
    }
}

fn save_var(name: &str) -> Option<OsString> {
    env::var_os(name)
}

fn fake_agent_source() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_nightshift-fake-agent"))
}

fn agent_command_name() -> String {
    format!("claude{}", env::consts::EXE_SUFFIX)
}

fn install_fake_claude(mode: &str, configure: impl FnOnce(&Path)) -> (TestCommandEnv, PathBuf) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let temp_dir = env::temp_dir().join(format!(
        "nightshift-agent-test-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");

    let command_path = temp_dir.join(agent_command_name());
    fs::copy(fake_agent_source(), &command_path).expect("fake agent should be copied");

    let saved_fake_mode = save_var("NIGHTSHIFT_FAKE_MODE");
    let saved_fake_args_file = save_var("NIGHTSHIFT_FAKE_ARGS_FILE");
    unsafe {
        env::set_var("NIGHTSHIFT_FAKE_MODE", mode);
    }
    configure(&temp_dir);

    let original_path = env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![temp_dir.clone()];
    paths.extend(env::split_paths(&original_path));
    let joined_path = env::join_paths(paths).expect("PATH should be joinable");
    unsafe {
        env::set_var("PATH", joined_path);
    }

    (
        TestCommandEnv {
            original_path,
            saved_fake_mode,
            saved_fake_args_file,
            temp_dir: temp_dir.clone(),
        },
        temp_dir,
    )
}

#[test]
fn process_runner_passes_supported_model_through_unchanged() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_env, temp_dir) = install_fake_claude("passthrough", |temp_dir| {
        let args_file = temp_dir.join("args.txt");
        unsafe {
            env::set_var("NIGHTSHIFT_FAKE_ARGS_FILE", &args_file);
        }
    });
    let args_file = temp_dir.join("args.txt");

    ProcessAgentRunner
        .run(
            Agent::Claude,
            Some("claude-sonnet-does-not-exist"),
            "prompt",
        )
        .expect("supported agents should pass --model through unchanged");

    let args = fs::read_to_string(args_file).expect("args file should be written");
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        vec![
            "-p",
            "--dangerously-skip-permissions",
            "--model",
            "claude-sonnet-does-not-exist"
        ]
    );
}

#[test]
fn process_runner_surfaces_exit_status_and_model_hint() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_env, _temp_dir) = install_fake_claude("stderr_fail", |_| {});

    let err = ProcessAgentRunner
        .run(Agent::Claude, Some("sonnet"), "prompt")
        .expect_err("agent failure should bubble up")
        .to_string();
    assert!(err.contains("exited with status"));
    assert!(err.contains("The agent may have rejected --model sonnet"));
    assert!(!err.contains("cli said no"));
}

#[test]
fn process_runner_does_not_mask_early_model_rejection_as_stdin_failure() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_env, _temp_dir) = install_fake_claude("early_close", |_| {});

    let err = ProcessAgentRunner
        .run(Agent::Claude, Some("sonnet"), &"x".repeat(1_000_000))
        .expect_err("early rejection should bubble up")
        .to_string();
    assert!(err.contains("exited with status"));
    assert!(err.contains("The agent may have rejected --model sonnet"));
    assert!(!err.contains("failed to write prompt to agent's stdin"));
    assert!(!err.contains("unknown option"));
}

#[test]
fn process_runner_completes_with_silent_noisy_agent() {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_env, _temp_dir) = install_fake_claude("noisy_success", |_| {});

    ProcessAgentRunner
        .run(Agent::Claude, None, &"x".repeat(64 * 1024))
        .expect("noisy agent stdout/stderr on null should not hang the runner");
}
