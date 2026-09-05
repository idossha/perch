//! `perch setup` against a fake home, with the generated snippet sourced into
//! a real private tmux server.

#[path = "e2e/mod.rs"]
mod e2e;

use std::path::Path;

use e2e::Server;

/// A fake home with an existing claude settings file and codex hooks, so setup
/// has to merge rather than create.
fn seed_home(home: &Path) {
    let claude = home.join(".claude");
    let codex = home.join(".codex");
    let pi = home.join(".pi/agent/extensions");
    for d in [&claude, &codex, &pi] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        claude.join("settings.json"),
        "{\n  \"model\": \"opus\"\n}\n",
    )
    .unwrap();
    let existing = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex_hooks_existing.json"
    ))
    .unwrap();
    std::fs::write(codex.join("hooks.json"), existing).unwrap();
    std::fs::write(codex.join("config.toml"), "").unwrap();
    std::fs::write(home.join(".tmux.conf"), "set -g mouse on\n").unwrap();
}

fn setup_env(s: &Server) -> Vec<(String, String)> {
    let h = &s.home;
    vec![
        (
            "PERCH_CLAUDE_SETTINGS".into(),
            h.join(".claude/settings.json").display().to_string(),
        ),
        (
            "PERCH_CODEX_HOOKS".into(),
            h.join(".codex/hooks.json").display().to_string(),
        ),
        (
            "PERCH_CODEX_CONFIG".into(),
            h.join(".codex/config.toml").display().to_string(),
        ),
        (
            "PERCH_PI_EXT_DIR".into(),
            h.join(".pi/agent/extensions").display().to_string(),
        ),
        (
            "PERCH_TMUX_CONF".into(),
            h.join(".tmux.conf").display().to_string(),
        ),
        // A harness installed on the developer's machine must not leak into
        // the fake home's detection.
        ("PERCH_NO_PATH_PROBE".into(), "1".into()),
    ]
}

#[test]
fn setup_wires_a_fake_home_and_a_real_tmux_server() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    seed_home(&s.home);
    assert!(
        !s.tmux_out(&["list-keys", "-T", "prefix"]).contains("perch"),
        "the bare server already knows perch"
    );

    let run = |args: &[&str]| {
        let mut r = s.perch(args);
        for (k, v) in setup_env(&s) {
            r = r.env(&k, &v);
        }
        r.run()
    };

    let out = run(&["setup"]);
    assert!(out.ok, "setup failed: {}\n{}", out.stdout, out.stderr);

    // The claude settings kept their own keys and gained the perch hooks.
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(s.home.join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["model"], "opus");
    assert!(
        settings["hooks"].is_object(),
        "claude hooks not merged: {settings}"
    );

    // The pre-existing codex hook survived the merge.
    let hooks = std::fs::read_to_string(s.home.join(".codex/hooks.json")).unwrap();
    assert!(hooks.contains("perch"), "{hooks}");

    // The tmux snippet exists and ~/.tmux.conf sources it, exactly once.
    let snippet = s.config_dir.join("perch.tmux.conf");
    assert!(snippet.exists(), "no {}", snippet.display());
    let conf = std::fs::read_to_string(s.home.join(".tmux.conf")).unwrap();
    assert!(conf.contains("set -g mouse on"), "{conf}");
    let source_lines = conf
        .lines()
        .filter(|l| l.contains("perch.tmux.conf"))
        .count();
    assert_eq!(source_lines, 1, "{conf}");

    // Running setup again changes nothing: still one source line.
    assert!(run(&["setup"]).ok);
    let conf2 = std::fs::read_to_string(s.home.join(".tmux.conf")).unwrap();
    assert_eq!(conf2, conf, "setup is not idempotent");

    // Source the generated snippet into the private server, twice.
    let path = snippet.display().to_string();
    s.tmux_out(&["source-file", &path]);
    let keys = s.tmux_out(&["list-keys", "-T", "prefix"]);
    let perch_keys: Vec<&str> = keys.lines().filter(|l| l.contains("perch")).collect();
    assert!(
        perch_keys.iter().any(|l| l.contains(" g ")),
        "no g binding: {perch_keys:?}"
    );
    assert!(
        perch_keys.iter().any(|l| l.contains(" N ")),
        "no N binding: {perch_keys:?}"
    );
    s.tmux_out(&["source-file", &path]);
    let keys2 = s.tmux_out(&["list-keys", "-T", "prefix"]);
    assert_eq!(keys, keys2, "sourcing twice changed the key table");

    // doctor reports everything wired.
    let d = run(&["doctor", "--json"]);
    assert!(d.ok, "doctor is unhappy: {}\n{}", d.stdout, d.stderr);
    let v: serde_json::Value = serde_json::from_str(&d.stdout).unwrap();
    assert_eq!(v["unwired"].as_array().unwrap().len(), 0, "{}", d.stdout);
    assert_eq!(v["tmux_binding"], true, "{}", d.stdout);
    assert_eq!(v["tmux_sourced"], true, "{}", d.stdout);
    assert_eq!(v["setup_marker"], true, "{}", d.stdout);

    // uninstall puts the home back.
    let u = run(&["uninstall"]);
    assert!(u.ok, "uninstall failed: {}\n{}", u.stdout, u.stderr);
    let conf3 = std::fs::read_to_string(s.home.join(".tmux.conf")).unwrap();
    assert!(conf3.contains("set -g mouse on"), "{conf3}");
    assert!(
        !conf3.contains("perch.tmux.conf"),
        "the source line survived uninstall: {conf3}"
    );
    let hooks = std::fs::read_to_string(s.home.join(".codex/hooks.json")).unwrap();
    assert!(!hooks.contains("perch"), "perch hooks survived: {hooks}");
    let settings = std::fs::read_to_string(s.home.join(".claude/settings.json")).unwrap();
    assert!(settings.contains("opus"), "{settings}");
    assert!(!settings.contains("perch"), "{settings}");
}
