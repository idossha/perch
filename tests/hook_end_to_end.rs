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

/// A turn that ends while you are watching the pane is `idle`, not `done`,
/// and `perch seen` is what says "I have looked at it".
#[test]
fn done_means_finished_while_you_were_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let state = |args: &[&str], stdin: Option<&str>, focused: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_perch"));
        cmd.args(args)
            .env("PERCH_STATE_DIR", p)
            .env("PERCH_CONFIG_DIR", p.join("config"))
            .env("PERCH_NO_SOUND", "1")
            .env("PERCH_NO_TMUX", "1")
            .env("TMUX_PANE", "%999")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if focused {
            // One client, showing this pane, with tmux's focus flag.
            cmd.env("PERCH_FAKE_VIEWERS", "%999:focused");
        }
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.unwrap_or("").as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        let body = std::fs::read_to_string(p.join("panes/_999.json")).unwrap_or_default();
        serde_json::from_str::<serde_json::Value>(&body)
            .map(|v| v["state"].as_str().unwrap_or("").to_string())
            .unwrap_or_default()
    };

    state(
        &["hook", "claude"],
        Some(&fixture("user_prompt_submit.json")),
        false,
    );
    assert_eq!(
        state(&["hook", "claude"], Some(&fixture("stop.json")), true),
        "idle",
        "a turn you watched end is idle"
    );

    state(
        &["hook", "claude"],
        Some(&fixture("user_prompt_submit.json")),
        false,
    );
    assert_eq!(
        state(&["hook", "claude"], Some(&fixture("stop.json")), false),
        "done"
    );
    assert_eq!(state(&["seen", "%999"], None, false), "idle");
    // Seeing an idle pane again is a no-op.
    assert_eq!(state(&["seen", "%999"], None, false), "idle");
}

/// A tool call clears a `needs_input` that was answered outside perch's view.
#[test]
fn a_tool_call_unblocks_a_stale_needs_input() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    assert!(
        run(
            p,
            &["hook", "claude"],
            Some(&fixture("notification_permission_prompt.json"))
        )
        .2
    );
    assert!(run(p, &["hook", "claude"], Some(&fixture("pre_tool_use.json"))).2);
    let body = std::fs::read_to_string(p.join("panes/_999.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["state"], "working");

    // idle_prompt is logged and changes nothing.
    assert!(
        run(
            p,
            &["hook", "claude"],
            Some(&fixture("notification_idle_prompt.json"))
        )
        .2
    );
    let body = std::fs::read_to_string(p.join("panes/_999.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["state"], "working");
    let events = std::fs::read_to_string(p.join("events.jsonl")).unwrap();
    assert!(events.contains("\"event\":\"idle_prompt\""), "{events}");
}

/// The instant cue is the sound; the only tmux write is the pane option.
#[test]
fn a_parent_transition_sets_the_pane_state_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let log = p.join("tmux.log");

    let hook = |body: &str| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_perch"));
        cmd.args(["hook", "claude"])
            .env("PERCH_STATE_DIR", p)
            .env("PERCH_CONFIG_DIR", p.join("config"))
            .env("PERCH_NO_SOUND", "1")
            .env("PERCH_NO_TMUX", "1")
            .env("PERCH_TMUX_LOG", &log)
            .env("PERCH_FAKE_CLIENTS", "/dev/ttys001,/dev/ttys002")
            .env("TMUX_PANE", "%999")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
    };

    hook(&fixture("notification_permission_prompt.json"));
    let lines: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines.len(), 1, "one option write and no more: {lines:?}");
    assert_eq!(lines[0], "set-option -p -t %999 @perch_state needs_input");
    // Nothing draws on the user's screen: no toast, no window flag, no flash.
    let all = lines.join("\n");
    for gone in ["toast", "@perch_flag", "display-message", "display-popup"] {
        assert!(!all.contains(gone), "{gone} survived: {all}");
    }

    // A subagent event is not a parent transition: no cue at all.
    hook(&fixture("subagent_start.json"));
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);

    // Working sets the state and says nothing else.
    hook(&fixture("user_prompt_submit.json"));
    let body = std::fs::read_to_string(&log).unwrap();
    let last = body.lines().last().unwrap().to_string();
    assert_eq!(last, "set-option -p -t %999 @perch_state working");
    assert!(!body.contains("toast"), "{body}");
}

/// Subagent events land under the parent pane's record, never as panes of
/// their own, and `list --json` carries them.
#[test]
fn subagent_events_become_children_of_the_parent_pane() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();

    for f in [
        "user_prompt_submit.json",
        "subagent_start.json",
        "notification_agent_needs_input_child.json",
        "subagent_stop_event.json",
    ] {
        assert!(run(p, &["hook", "claude"], Some(&fixture(f))).2, "{f}");
    }

    let (out, _, ok) = run(p, &["list", "--json"], None);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let recs = v.as_array().unwrap();
    assert_eq!(recs.len(), 1, "one pane, not one per subagent: {out}");
    let children = recs[0]["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["id"], "sub-7");
    assert_eq!(children[0]["agent_type"], "Explore");
    assert_eq!(children[0]["state"], "done");
    assert_eq!(children[0]["last_message"], "Found it in src/reducer.rs.");

    // A new turn retires the finished child.
    assert!(
        run(
            p,
            &["hook", "claude"],
            Some(&fixture("user_prompt_submit.json"))
        )
        .2
    );
    let (out, _, _) = run(p, &["list", "--json"], None);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v[0]["children"].as_array().unwrap().is_empty(), "{out}");
}

/// Codex reports subagents the same way, and the parent's own state is
/// untouched by them.
#[test]
fn codex_subagents_do_not_move_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let args = ["hook", "codex"];
    assert!(
        run(
            p,
            &args,
            Some(&harness_fixture("codex", "user_prompt_submit.json"))
        )
        .2
    );
    assert!(
        run(
            p,
            &args,
            Some(&harness_fixture("codex", "subagent_start.json"))
        )
        .2
    );

    let events = std::fs::read_to_string(p.join("events.jsonl")).unwrap();
    assert!(events.contains("\"event\":\"subagent_start\""), "{events}");
    // The pane stayed `working`: the subagent never changed its state.
    assert!(
        events.lines().all(|l| l.contains("\"state\":\"working\"")),
        "{events}"
    );
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
    assert!(written.contains(r##"bind g run-shell 'perch open --client "#{client_name}"'"##));
    // Without --apply the user's tmux.conf is untouched.
    assert_eq!(
        std::fs::read_to_string(&tmux_conf).unwrap(),
        "set -g mouse on\n"
    );
}
