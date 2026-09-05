//! The one test that talks to a real coding agent.
//!
//! **This test uses the user's real harness installation.** It does not install
//! or modify anything: it runs `codex` / `claude` exactly as they are set up on
//! this machine, so the hooks that fire are the ones in the real
//! `~/.codex/hooks.json` and `~/.claude/settings.json`. What keeps it hermetic
//! is that the pane it runs in carries this test's `PERCH_STATE_DIR`, so the
//! records those real hooks write land in a temp directory and never in the
//! user's own state.
//!
//! It costs tokens and needs network, so it is off unless `PERCH_E2E_REAL=1`.
//! Without that it prints why it skipped and passes.

#[path = "e2e/mod.rs"]
mod e2e;

use std::process::{Command, Stdio};
use std::time::Duration;

use e2e::{wait_for, Server};

fn enabled() -> bool {
    if std::env::var("PERCH_E2E_REAL").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("skipping: set PERCH_E2E_REAL=1 to run against the real harness install");
    false
}

fn on_path(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `cmd` in a pane of the private server and wait for perch to record it.
fn records_land_in_the_temp_state_dir(s: &Server, harness: &str, cmd: &str) {
    let pane = s.new_shell_window("one", harness);
    // The pane inherits PERCH_STATE_DIR / TMUX from the server's environment,
    // so the real hooks write here, not into the user's state directory.
    s.send_keys(&pane, &[cmd, "Enter"]);

    let events = s.state_dir.join("events.jsonl");
    let landed = wait_for(
        || {
            std::fs::read_to_string(&events)
                .map(|b| b.contains(&format!("\"harness\":\"{harness}\"")))
                .unwrap_or(false)
        },
        Duration::from_secs(180),
    );
    assert!(
        landed,
        "no {harness} events in {}; pane said:\n{}",
        events.display(),
        s.capture(&pane)
    );
    assert!(
        !s.state_of(&pane).is_empty(),
        "no record for the pane {harness} ran in"
    );
}

#[test]
fn a_real_codex_run_is_recorded_in_this_tests_state_dir() {
    if e2e::no_tmux() || !enabled() {
        return;
    }
    if !on_path("codex") {
        eprintln!("skipping: codex is not on PATH");
        return;
    }
    let s = Server::start();
    records_land_in_the_temp_state_dir(
        &s,
        "codex",
        "codex exec --skip-git-repo-check 'reply with the single word pong'",
    );
}

#[test]
fn a_real_claude_run_is_recorded_in_this_tests_state_dir() {
    if e2e::no_tmux() || !enabled() {
        return;
    }
    if !on_path("claude") {
        eprintln!("skipping: claude is not on PATH");
        return;
    }
    let s = Server::start();
    records_land_in_the_temp_state_dir(&s, "claude", "claude -p 'reply with the single word pong'");
}
