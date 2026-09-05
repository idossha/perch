//! Drives the built binary with fixture payloads. No sound, no tmux, no home.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(dir: &std::path::Path, args: &[&str], stdin: Option<&str>) -> (String, String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_perch"));
    cmd.args(args)
        .env("PERCH_STATE_DIR", dir)
        .env("PERCH_CONFIG_DIR", dir.join("config"))
        .env("PERCH_NO_SOUND", "1")
        .env("PERCH_NO_TMUX", "1")
        .env("TMUX_PANE", "%999")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.unwrap_or("").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.success(),
    )
}

fn fixture(name: &str) -> String {
    harness_fixture("claude", name)
}

fn harness_fixture(harness: &str, name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/{harness}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn a_prompt_then_stop_leaves_a_done_record() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();

    let (_, _, ok) = run(
        p,
        &["hook", "claude"],
        Some(&fixture("user_prompt_submit.json")),
    );
    assert!(ok);
    let (_, _, ok) = run(p, &["hook", "claude"], Some(&fixture("stop.json")));
    assert!(ok);

    let (out, _, ok) = run(p, &["list", "--json"], None);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let rec = &v.as_array().unwrap()[0];
    assert_eq!(rec["pane"], "%999");
    assert_eq!(rec["harness"], "claude");
    assert_eq!(rec["project"], "perch");
    // The pane is not in the (empty) tmux pane list, so liveness ends it.
    assert_eq!(rec["state"], "ended");
    assert_eq!(
        rec["last_message"],
        "Done: added the reducer and its tests."
    );

    let events = std::fs::read_to_string(p.join("events.jsonl")).unwrap();
    assert_eq!(events.lines().count(), 2);
    assert!(events.contains("\"event\":\"stop\""));
}

#[test]
fn malformed_and_untracked_payloads_still_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    for body in ["", "not json", "{}", &fixture("precompact.json")] {
        let (_, _, ok) = run(p, &["hook", "claude"], Some(body));
        assert!(ok, "hook must never fail its harness: {body:?}");
    }
    assert!(!p.join("panes").exists());
}

#[test]
fn codex_and_pi_hooks_record_the_same_way_claude_does() {
    for (harness, prompt, stop, message) in [
        (
            "codex",
            "user_prompt_submit.json",
            "stop.json",
            "Added the codex adapter.",
        ),
        (
            "pi",
            "agent_start.json",
            "agent_end.json",
            "Wrote the pi extension.",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let args = ["hook", harness];
        assert!(run(p, &args, Some(&harness_fixture(harness, prompt))).2);
        assert!(run(p, &args, Some(&harness_fixture(harness, stop))).2);

        let (out, _, ok) = run(p, &["list", "--json"], None);
        assert!(ok);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let rec = &v.as_array().unwrap()[0];
        assert_eq!(rec["pane"], "%999", "{harness}");
        assert_eq!(rec["harness"], harness);
        assert_eq!(rec["project"], "perch", "{harness}");
        assert_eq!(rec["last_message"], message, "{harness}");

        let events = std::fs::read_to_string(p.join("events.jsonl")).unwrap();
        assert_eq!(events.lines().count(), 2, "{harness}: {events}");
        assert!(events.contains("\"event\":\"stop\""), "{harness}");
    }
}

#[test]
fn codex_and_pi_hooks_never_fail_their_harness() {
    for harness in ["codex", "pi"] {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        for body in ["", "not json", "{}", r#"{"hook_event_name":"PreToolUse"}"#] {
            let (_, _, ok) = run(p, &["hook", harness], Some(body));
            assert!(ok, "{harness} must never fail its harness: {body:?}");
        }
        assert!(!p.join("panes").exists(), "{harness}");
    }
}

#[test]
fn status_counts_states() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    run(
        p,
        &["hook", "claude"],
        Some(&fixture("notification_permission_prompt.json")),
    );
    // Liveness ends the pane (no tmux), so the counters read zero — the point is
    // that the command runs and formats.
    let (out, _, ok) = run(p, &["status"], None);
    assert!(ok);
    assert!(out.contains('⚑'), "{out}");
    let (out, _, ok) = run(p, &["status", "--format", "tmux"], None);
    assert!(ok, "{out}");
}

#[test]
fn install_claude_dry_run_writes_nothing_and_merges_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    std::fs::write(
        &settings,
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"idosleep hook stop"}]}]},"model":"opus"}"#,
    )
    .unwrap();
    let before = std::fs::read_to_string(&settings).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_perch"));
    let out = cmd
        .args(["install", "claude", "--dry-run"])
        .env("PERCH_CLAUDE_SETTINGS", &settings)
        .output()
        .unwrap();
    assert!(out.status.success());
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(report.contains("dry run"), "{report}");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), before);

    // --print shows the merged document without touching the file.
    let out = Command::new(env!("CARGO_BIN_EXE_perch"))
        .args(["install", "claude", "--print"])
        .env("PERCH_CLAUDE_SETTINGS", &settings)
        .output()
        .unwrap();
    let merged: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let stop = merged["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["hooks"][0]["command"], "idosleep hook stop");
    assert_eq!(stop[1]["hooks"][0]["command"], "perch hook claude");
    assert_eq!(merged["model"], "opus");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), before);
}

#[test]
fn install_claude_writes_a_backup_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    std::fs::write(&settings, r#"{"model":"opus"}"#).unwrap();

    let install = || {
        Command::new(env!("CARGO_BIN_EXE_perch"))
            .args(["install", "claude"])
            .env("PERCH_CLAUDE_SETTINGS", &settings)
            .output()
            .unwrap()
    };
    assert!(install().status.success());
    let after_first = std::fs::read_to_string(&settings).unwrap();
    assert!(after_first.contains("perch hook claude"));

    let backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().to_string_lossy().to_string();
            n.starts_with("settings.json.bak-").then_some(n)
        })
        .collect();
    assert_eq!(backups.len(), 1, "expected one backup, got {backups:?}");

    let out = install();
    assert!(String::from_utf8_lossy(&out.stdout).contains("already installed"));
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), after_first);
}

#[test]
fn install_tmux_writes_the_popup_binding_and_prints_the_source_line() {
    let dir = tempfile::tempdir().unwrap();
    let tmux_conf = dir.path().join("tmux.conf");
    std::fs::write(&tmux_conf, "set -g mouse on\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_perch"))
        .args(["install", "tmux"])
        .env("PERCH_CONFIG_DIR", dir.path().join("config"))
        .env("PERCH_TMUX_CONF", &tmux_conf)
        .output()
        .unwrap();
    assert!(out.status.success());
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(report.contains("source-file"), "{report}");

    let written = std::fs::read_to_string(dir.path().join("config/perch.tmux.conf")).unwrap();
    assert!(written.contains("bind g display-popup -E -w 85% -h 75% 'perch tui'"));
    // Without --apply the user's tmux.conf is untouched.
    assert_eq!(
        std::fs::read_to_string(&tmux_conf).unwrap(),
        "set -g mouse on\n"
    );
}
