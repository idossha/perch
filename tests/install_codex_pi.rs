//! The codex and pi installers, driven through the built binary against temp
//! copies. The user's real `~/.codex` and `~/.pi` are never written to.

use std::path::Path;
use std::process::Command;

/// A copy of the machine's real `~/.codex/hooks.json` (paths anonymised): a
/// herdr SessionStart entry followed by three AXI ones.
const EXISTING: &str = include_str!("fixtures/codex_hooks_existing.json");

fn install(args: &[&str], hooks: &Path) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_perch"))
        .args(args)
        .env("PERCH_CODEX_HOOKS", hooks)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.success(),
    )
}

fn commands(v: &serde_json::Value, event: &str) -> Vec<String> {
    v["hooks"][event]
        .as_array()
        .unwrap_or_else(|| panic!("{event} is not an array in {v}"))
        .iter()
        .map(|g| g["hooks"][0]["command"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn codex_merge_preserves_every_existing_entry_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let hooks = dir.path().join("hooks.json");
    std::fs::write(&hooks, EXISTING).unwrap();

    let (report, ok) = install(&["install", "codex"], &hooks);
    assert!(ok, "{report}");

    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
    let session_start = commands(&after, "SessionStart");
    assert_eq!(session_start.len(), 5, "{session_start:?}");
    assert!(session_start[0].contains("herdr-agent-state.sh"));
    assert_eq!(session_start[1], "gh-axi");
    assert_eq!(session_start[2], "chrome-devtools-axi");
    assert_eq!(session_start[3], "lavish-axi");
    assert_eq!(session_start[4], "perch hook codex");
    assert_eq!(after["hooks"]["SessionStart"][4]["matcher"], "*");
    assert_eq!(after["hooks"]["SessionStart"][4]["hooks"][0]["timeout"], 10);

    // The other four events are created with perch alone.
    for event in [
        "UserPromptSubmit",
        "Stop",
        "PermissionRequest",
        "SessionEnd",
    ] {
        assert_eq!(commands(&after, event), vec!["perch hook codex"], "{event}");
        assert!(
            after["hooks"][event][0].get("matcher").is_none(),
            "{event} should not take a matcher"
        );
    }
}

#[test]
fn codex_install_backs_up_once_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let hooks = dir.path().join("hooks.json");
    std::fs::write(&hooks, EXISTING).unwrap();

    assert!(install(&["install", "codex"], &hooks).1);
    let after_first = std::fs::read_to_string(&hooks).unwrap();

    let backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().to_string_lossy().to_string();
            n.starts_with("hooks.json.bak-").then_some(n)
        })
        .collect();
    assert_eq!(backups.len(), 1, "expected one backup, got {backups:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(&backups[0])).unwrap(),
        EXISTING
    );

    let (report, ok) = install(&["install", "codex"], &hooks);
    assert!(ok);
    assert!(report.contains("already installed"), "{report}");
    assert_eq!(std::fs::read_to_string(&hooks).unwrap(), after_first);
}

#[test]
fn codex_dry_run_and_print_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let hooks = dir.path().join("hooks.json");
    std::fs::write(&hooks, EXISTING).unwrap();

    let (report, ok) = install(&["install", "codex", "--dry-run"], &hooks);
    assert!(ok);
    assert!(report.contains("dry run"), "{report}");
    assert_eq!(std::fs::read_to_string(&hooks).unwrap(), EXISTING);

    let (printed, ok) = install(&["install", "codex", "--print"], &hooks);
    assert!(ok);
    let merged: serde_json::Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(commands(&merged, "SessionStart").len(), 5);
    assert_eq!(std::fs::read_to_string(&hooks).unwrap(), EXISTING);
}

#[test]
fn codex_install_creates_the_file_when_there_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let hooks = dir.path().join("nested/hooks.json");
    assert!(install(&["install", "codex"], &hooks).1);
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
    assert_eq!(commands(&after, "Stop"), vec!["perch hook codex"]);
}

fn install_pi(args: &[&str], ext_dir: &Path) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_perch"))
        .args(args)
        .env("PERCH_PI_EXT_DIR", ext_dir)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.success(),
    )
}

#[test]
fn pi_install_writes_the_extension_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let ext_dir = dir.path().join("extensions");
    // A neighbouring extension must survive untouched.
    std::fs::create_dir_all(&ext_dir).unwrap();
    let neighbour = ext_dir.join("permission-gate.ts");
    std::fs::write(&neighbour, "// someone else's extension\n").unwrap();

    let (report, ok) = install_pi(&["install", "pi"], &ext_dir);
    assert!(ok, "{report}");
    let written = std::fs::read_to_string(ext_dir.join("perch.ts")).unwrap();
    assert!(
        written.contains(r#"spawn("perch", ["hook", "pi"]"#),
        "{written}"
    );
    assert!(written.contains(r#"pi.on("agent_end""#));
    assert_eq!(
        std::fs::read_to_string(&neighbour).unwrap(),
        "// someone else's extension\n"
    );

    let (report, ok) = install_pi(&["install", "pi"], &ext_dir);
    assert!(ok);
    assert!(report.contains("already installed"), "{report}");
    assert_eq!(
        std::fs::read_to_string(ext_dir.join("perch.ts")).unwrap(),
        written
    );
}

#[test]
fn pi_install_replaces_a_stale_copy_after_backing_it_up() {
    let dir = tempfile::tempdir().unwrap();
    let ext_dir = dir.path().join("extensions");
    std::fs::create_dir_all(&ext_dir).unwrap();
    std::fs::write(ext_dir.join("perch.ts"), "// an old perch extension\n").unwrap();

    assert!(install_pi(&["install", "pi"], &ext_dir).1);
    let written = std::fs::read_to_string(ext_dir.join("perch.ts")).unwrap();
    assert!(written.contains("perch hook pi") || written.contains(r#"["hook", "pi"]"#));

    let backups: Vec<_> = std::fs::read_dir(&ext_dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().to_string_lossy().to_string();
            n.starts_with("perch.ts.bak-").then_some(n)
        })
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        std::fs::read_to_string(ext_dir.join(&backups[0])).unwrap(),
        "// an old perch extension\n"
    );
}

#[test]
fn pi_dry_run_and_print_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ext_dir = dir.path().join("extensions");

    let (report, ok) = install_pi(&["install", "pi", "--dry-run"], &ext_dir);
    assert!(ok);
    assert!(report.contains("dry run"), "{report}");
    assert!(!ext_dir.join("perch.ts").exists());

    let (printed, ok) = install_pi(&["install", "pi", "--print"], &ext_dir);
    assert!(ok);
    assert!(printed.contains("ExtensionAPI"), "{printed}");
    assert!(!ext_dir.join("perch.ts").exists());
}

/// Codex clamps a `SessionEnd` hook to three seconds and warns at every
/// start about anything longer, so that one group is installed at three — and
/// a re-run brings an older install's group down to it instead of leaving it.
#[test]
fn codex_session_end_hook_is_installed_within_the_clamp_and_older_installs_are_corrected() {
    use perch::install::merge_codex_hooks;
    let m = merge_codex_hooks(serde_json::json!({}));
    let end = &m.settings["hooks"]["SessionEnd"][0]["hooks"][0];
    assert_eq!(end["command"], "perch hook codex");
    assert_eq!(end["timeout"], 3, "{end}");
    assert_eq!(m.settings["hooks"]["Stop"][0]["hooks"][0]["timeout"], 10);

    let old = serde_json::json!({ "hooks": { "SessionEnd": [
        { "hooks": [{ "type": "command", "command": "perch hook codex", "timeout": 10 }] }
    ]}});
    let m = merge_codex_hooks(old);
    let end = &m.settings["hooks"]["SessionEnd"][0]["hooks"][0];
    assert_eq!(end["timeout"], 3, "corrected in place: {end}");
    assert_eq!(
        m.settings["hooks"]["SessionEnd"].as_array().unwrap().len(),
        1,
        "not duplicated"
    );
    assert!(
        m.added.iter().any(|a| a.contains("SessionEnd")),
        "reported as a change: {:?}",
        m.added
    );
    let again = merge_codex_hooks(m.settings.clone());
    assert!(again.added.is_empty(), "and then idempotent");
}

/// Regression: the codex hooks the user actually had — a `SessionEnd` entry at
/// ten seconds — made codex warn at every start; and a Claude install from
/// 0.3 had one `--ask` group where 0.4 needs two. Both are corrected by one
/// `setup` re-run, and both are then stable.
#[test]
fn a_zero_three_install_is_brought_to_zero_four_by_one_setup_run() {
    use perch::install::{merge_claude_settings, merge_codex_hooks};
    let claude_0_3 = serde_json::json!({ "hooks": {
        "PreToolUse": [
            { "hooks": [{ "type": "command", "command": "perch hook claude", "timeout": 5 }] },
            { "matcher": "AskUserQuestion", "hooks": [{ "type": "command", "command": "perch hook claude --ask", "timeout": 3600 }] }
        ]
    }});
    let m = merge_claude_settings(claude_0_3);
    assert_eq!(
        m.settings["hooks"]["PermissionRequest"][0]["matcher"],
        "AskUserQuestion"
    );
    assert_eq!(
        m.settings["hooks"]["PreToolUse"].as_array().unwrap().len(),
        2,
        "no duplicate ask group"
    );
    let again = merge_claude_settings(m.settings.clone());
    assert!(again.added.is_empty());

    let codex_0_3 = serde_json::json!({ "hooks": {
        "SessionEnd": [{ "hooks": [{ "type": "command", "command": "perch hook codex", "timeout": 10 }] }],
        "Stop": [{ "hooks": [{ "type": "command", "command": "perch hook codex", "timeout": 10 }] }]
    }});
    let m = merge_codex_hooks(codex_0_3);
    assert_eq!(
        m.settings["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"],
        3
    );
    assert_eq!(m.settings["hooks"]["Stop"][0]["hooks"][0]["timeout"], 10);
    let again = merge_codex_hooks(m.settings.clone());
    assert!(again.added.is_empty());
}
