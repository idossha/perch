//! `perch setup`, `perch doctor` and `perch uninstall` end to end, always
//! against a temporary `PERCH_HOME`. The real home is never read or written.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};
use tempfile::TempDir;

const CLAUDE_BEFORE: &str = r#"{
  "model": "opus",
  "hooks": {
    "Stop": [
      {"hooks": [{"type": "command", "command": "idosleep hook"}]}
    ]
  }
}
"#;

const CODEX_BEFORE: &str = r#"{
  "hooks": {
    "SessionStart": [
      {"matcher": "*", "hooks": [{"type": "command", "command": "gh-axi"}]}
    ]
  }
}
"#;

const TMUX_BEFORE: &str = "set -g mouse on\nset -g base-index 1\n";

/// A fake home with claude and codex present and pi absent.
fn fake_home() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path();
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    std::fs::write(h.join(".claude/settings.json"), CLAUDE_BEFORE).unwrap();
    std::fs::create_dir_all(h.join(".codex")).unwrap();
    std::fs::write(h.join(".codex/hooks.json"), CODEX_BEFORE).unwrap();
    std::fs::write(h.join(".tmux.conf"), TMUX_BEFORE).unwrap();
    dir
}

fn perch(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_perch"))
        .args(args)
        .env("PERCH_HOME", home)
        .env("PERCH_NO_PATH_PROBE", "1")
        .env("PERCH_NO_SOUND", "1")
        .env("PERCH_NO_TMUX", "1")
        .env_remove("TMUX")
        .env_remove("PERCH_CLAUDE_SETTINGS")
        .env_remove("PERCH_CODEX_HOOKS")
        .env_remove("PERCH_CODEX_CONFIG")
        .env_remove("PERCH_PI_EXT_DIR")
        .env_remove("PERCH_TMUX_CONF")
        .env_remove("PERCH_CONFIG_DIR")
        .env_remove("PERCH_STATE_DIR")
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn json_at(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn setup_wires_present_harnesses_skips_absent_and_is_idempotent() {
    let home = fake_home();
    let h = home.path();

    let out = perch(h, &["setup"]);
    let report = stdout(&out);
    assert!(out.status.success(), "{report}");
    assert!(
        report.contains("component") && report.contains("status"),
        "{report}"
    );
    for line in report.lines() {
        match line.split_whitespace().next() {
            Some("claude") | Some("codex") | Some("tmux") => {
                assert!(line.contains("installed"), "{line}")
            }
            Some("pi") => assert!(line.contains("skipped: not found"), "{line}"),
            _ => {}
        }
    }

    // claude and codex keep what they had and gain perch.
    let claude = json_at(&h.join(".claude/settings.json"));
    assert_eq!(claude["model"], "opus");
    assert_eq!(
        claude["hooks"]["Stop"][0]["hooks"][0]["command"],
        "idosleep hook"
    );
    assert_eq!(
        claude["hooks"]["Stop"][1]["hooks"][0]["command"],
        "perch hook claude"
    );
    let codex = json_at(&h.join(".codex/hooks.json"));
    assert_eq!(
        codex["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "gh-axi"
    );
    assert_eq!(
        codex["hooks"]["Stop"][0]["hooks"][0]["command"],
        "perch hook codex"
    );

    // pi was skipped entirely.
    assert!(!h.join(".pi").exists());

    // tmux: our conf plus the one source line appended.
    let perch_conf = h.join(".config/perch/perch.tmux.conf");
    assert!(std::fs::read_to_string(&perch_conf)
        .unwrap()
        .contains("perch tui"));
    let tmux_conf = std::fs::read_to_string(h.join(".tmux.conf")).unwrap();
    assert!(tmux_conf.starts_with(TMUX_BEFORE), "{tmux_conf}");
    assert!(tmux_conf.contains(&format!("source-file {}", perch_conf.display())));

    // The marker records the run.
    let marker = json_at(&h.join(".config/perch/setup.json"));
    assert_eq!(marker["version"], env!("CARGO_PKG_VERSION"));
    assert!(marker["timestamp"].as_str().unwrap().contains('T'));
    let names: Vec<&str> = marker["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["claude", "codex", "pi", "tmux"]);

    // Second run changes nothing and says so.
    let after_first = std::fs::read_to_string(h.join(".claude/settings.json")).unwrap();
    let report = stdout(&perch(h, &["setup"]));
    for line in report.lines() {
        if matches!(
            line.split_whitespace().next(),
            Some("claude") | Some("codex") | Some("tmux")
        ) {
            assert!(line.contains("already"), "{line}");
        }
    }
    assert_eq!(
        std::fs::read_to_string(h.join(".claude/settings.json")).unwrap(),
        after_first
    );
}

#[test]
fn setup_dry_run_writes_nothing() {
    let home = fake_home();
    let h = home.path();
    let report = stdout(&perch(h, &["setup", "--dry-run", "--yes"]));
    assert!(report.contains("dry run"), "{report}");
    assert_eq!(
        std::fs::read_to_string(h.join(".claude/settings.json")).unwrap(),
        CLAUDE_BEFORE
    );
    assert!(!h.join(".config/perch/setup.json").exists());
}

#[test]
fn setup_only_and_no_tmux_restrict_the_run() {
    let home = fake_home();
    let h = home.path();
    let report = stdout(&perch(h, &["setup", "--only", "claude", "--no-tmux"]));
    assert!(report.contains("claude"), "{report}");
    assert!(!report.contains("codex"), "{report}");
    assert_eq!(
        std::fs::read_to_string(h.join(".codex/hooks.json")).unwrap(),
        CODEX_BEFORE
    );
    assert_eq!(
        std::fs::read_to_string(h.join(".tmux.conf")).unwrap(),
        TMUX_BEFORE
    );
}

#[test]
fn doctor_reports_unwired_then_wired() {
    let home = fake_home();
    let h = home.path();

    let out = perch(h, &["doctor"]);
    let report = stdout(&out);
    assert!(
        !out.status.success(),
        "should fail while unwired:\n{report}"
    );
    assert!(report.contains("unwired: claude, codex"), "{report}");
    assert!(report.contains("pi      not found"), "{report}");

    assert!(perch(h, &["setup"]).status.success());

    let out = perch(h, &["doctor"]);
    let report = stdout(&out);
    assert!(out.status.success(), "{report}");
    assert!(report.contains("ok"), "{report}");

    let out = perch(h, &["doctor", "--json"]);
    let d: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(d["unwired"].as_array().unwrap().len(), 0);
    assert_eq!(d["tmux_binding"], json!(true));
    assert_eq!(d["tmux_sourced"], json!(true));
    assert_eq!(d["setup_marker"], json!(true));
    assert_eq!(d["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["harnesses"][2]["present"], json!(false));
}

#[test]
fn uninstall_restores_the_originals_and_leaves_other_lines() {
    let home = fake_home();
    let h = home.path();
    assert!(perch(h, &["setup"]).status.success());
    // Something in the state dir, so its removal is observable.
    std::fs::create_dir_all(h.join(".local/state/perch/panes")).unwrap();
    std::fs::write(h.join(".local/state/perch/panes/_1.json"), "{}").unwrap();

    let report = stdout(&perch(h, &["uninstall"]));
    assert!(report.contains("perch entries removed"), "{report}");

    // Byte-for-byte modulo key order: compare the parsed documents.
    let claude: Value = serde_json::from_str(CLAUDE_BEFORE).unwrap();
    assert_eq!(json_at(&h.join(".claude/settings.json")), claude);
    let codex: Value = serde_json::from_str(CODEX_BEFORE).unwrap();
    assert_eq!(json_at(&h.join(".codex/hooks.json")), codex);

    // The tmux conf keeps its own lines and loses only the source-file line.
    assert_eq!(
        std::fs::read_to_string(h.join(".tmux.conf")).unwrap(),
        TMUX_BEFORE
    );
    assert!(!h.join(".config/perch/perch.tmux.conf").exists());
    assert!(!h.join(".config/perch/setup.json").exists());
    assert!(!h.join(".local/state/perch").exists());
}

#[test]
fn uninstall_keep_state_and_dry_run() {
    let home = fake_home();
    let h = home.path();
    assert!(perch(h, &["setup"]).status.success());
    std::fs::create_dir_all(h.join(".local/state/perch/panes")).unwrap();

    let report = stdout(&perch(h, &["uninstall", "--dry-run"]));
    assert!(report.contains("dry run"), "{report}");
    assert!(h.join(".config/perch/setup.json").exists());
    assert!(std::fs::read_to_string(h.join(".claude/settings.json"))
        .unwrap()
        .contains("perch hook claude"));

    assert!(perch(h, &["uninstall", "--keep-state"]).status.success());
    assert!(h.join(".local/state/perch").exists());
    assert!(!std::fs::read_to_string(h.join(".claude/settings.json"))
        .unwrap()
        .contains("perch hook"));
}

#[test]
fn list_and_status_nudge_on_stderr_only() {
    let home = fake_home();
    let h = home.path();

    let out = perch(h, &["status"]);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(err.contains("run `perch setup`"), "{err}");
    assert!(!stdout(&out).contains("perch setup"), "{}", stdout(&out));
    assert!(stdout(&out).starts_with('⚑'), "{}", stdout(&out));

    let out = perch(h, &["list"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("run `perch setup`"));

    assert!(perch(h, &["setup"]).status.success());
    let out = perch(h, &["status"]);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

/// `perch setup` records codex's hook trust, so the user never sees the accept
/// prompt; `doctor` reports it and `uninstall` takes only perch's own keys.
#[test]
fn setup_writes_codex_trust_records_and_uninstall_removes_them() {
    let home = fake_home();
    let h = home.path();
    let config = h.join(".codex/config.toml");
    std::fs::write(
        &config,
        "model = \"gpt-5\"\n\n[hooks.state.\"/other/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:aa\"\n",
    )
    .unwrap();

    assert!(perch(h, &["setup"]).status.success());
    let body = std::fs::read_to_string(&config).unwrap();
    let hooks = h.join(".codex/hooks.json");
    let key = format!("{}:session_start:", hooks.display());
    assert!(body.contains(&key), "{body}");
    assert!(body.contains("model = \"gpt-5\""), "{body}");
    assert!(body.contains("/other/hooks.json:stop:0:0"), "{body}");
    // One record per perch handler: five events.
    assert_eq!(body.matches("trusted_hash").count(), 6, "{body}");

    let out = perch(h, &["doctor"]);
    assert!(stdout(&out).contains("trust: yes"), "{}", stdout(&out));

    // A second setup is a no-op on the config.
    assert!(perch(h, &["setup"]).status.success());
    assert_eq!(std::fs::read_to_string(&config).unwrap(), body);

    assert!(perch(h, &["uninstall", "--keep-state"]).status.success());
    let after = std::fs::read_to_string(&config).unwrap();
    assert!(!after.contains(&key), "{after}");
    assert!(after.contains("/other/hooks.json:stop:0:0"), "{after}");
    assert!(after.contains("model = \"gpt-5\""), "{after}");
}

/// The hashes perch writes are the ones codex recomputes from the file, so
/// reordering hooks.json and re-running setup refreshes the indices.
#[test]
fn reordering_hooks_json_refreshes_the_indices() {
    let home = fake_home();
    let h = home.path();
    assert!(perch(h, &["setup"]).status.success());
    let hooks_path = h.join(".codex/hooks.json");
    let mut hooks: Value = json_at(&hooks_path);
    // Put a foreign group in front of perch's Stop handler.
    let stop = hooks["hooks"]["Stop"].as_array_mut().unwrap();
    stop.insert(
        0,
        serde_json::json!({"hooks":[{"type":"command","command":"other","timeout":10}]}),
    );
    std::fs::write(&hooks_path, serde_json::to_string_pretty(&hooks).unwrap()).unwrap();

    // doctor now disagrees, and setup puts the right key back.
    let out = perch(h, &["doctor"]);
    assert!(stdout(&out).contains("trust: no"), "{}", stdout(&out));
    assert!(perch(h, &["setup"]).status.success());
    let body = std::fs::read_to_string(h.join(".codex/config.toml")).unwrap();
    assert!(
        body.contains(&format!("{}:stop:1:0", hooks_path.display())),
        "{body}"
    );
}
