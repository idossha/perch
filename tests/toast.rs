//! `perch toast` end to end, through the logging tmux: no server, no popup.

use std::process::Command;

fn toast(dir: &std::path::Path, args: &[&str], clients: &str) -> Vec<String> {
    let log = dir.join("tmux.log");
    let ok = Command::new(env!("CARGO_BIN_EXE_perch"))
        .args(args)
        .env("PERCH_NO_TMUX", "1")
        .env("PERCH_TMUX_LOG", &log)
        .env("PERCH_CONFIG_DIR", dir)
        .env("PERCH_FAKE_CLIENTS", clients)
        .status()
        .unwrap()
        .success();
    assert!(ok, "perch {args:?} failed");
    std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// One borderless bottom-right popup per attached client, sized to the text.
#[test]
fn one_launch_per_client_pinned_to_the_bottom_right() {
    let dir = tempfile::tempdir().unwrap();
    let lines = toast(
        dir.path(),
        &["toast", "needs_input", "perch (claude) needs input"],
        "/dev/ttys001,/dev/ttys002",
    );
    assert_eq!(lines.len(), 2, "{lines:?}");
    for (line, client) in lines.iter().zip(["/dev/ttys001", "/dev/ttys002"]) {
        assert!(line.starts_with(&format!("display-popup -c {client} -B -E -x R -y P -w 30 -h 1 -s bg=colour160,fg=colour255,bold -- ")), "{line}");
        assert!(
            line.ends_with(&format!(
                "toast-body needs_input 3000 perch (claude) needs input --client {client}"
            )),
            "{line}"
        );
    }
}

/// A message too wide for the corner is cut, not wrapped: the popup is one
/// line, and `-w` never exceeds the 60-column cap.
#[test]
fn a_long_message_is_ellipsized_and_the_width_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    let long = "some-extremely-long-project-name-that-will-not-fit (claude) needs input";
    let lines = toast(dir.path(), &["toast", "done", long], "/dev/ttys001");
    let line = &lines[0];
    assert!(line.contains(" -w 60 -h 1 "), "{line}");
    assert!(line.contains("-s bg=colour28,fg=colour255,bold"), "{line}");
    assert!(line.ends_with("… --client /dev/ttys001"), "{line}");
}

/// `perch toast test` draws a sample even with toasts turned off, so a user
/// can check the placement.
#[test]
fn the_sample_ignores_the_enabled_switch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[toast]\nenabled = false\n").unwrap();
    let lines = toast(dir.path(), &["toast", "test"], "/dev/ttys001");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("toast-body test 3000"), "{}", lines[0]);
}

/// Styles come from the config.
#[test]
fn styles_are_overridable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[toast]\nneeds_input_style = \"bg=blue,fg=white\"\nduration_ms = 900\n",
    )
    .unwrap();
    let lines = toast(dir.path(), &["toast", "needs_input", "hi"], "/dev/ttys001");
    assert!(lines[0].contains("-s bg=blue,fg=white "), "{}", lines[0]);
    assert!(
        lines[0].contains("toast-body needs_input 900 hi"),
        "{}",
        lines[0]
    );
}

/// An unknown kind is a plain error, not a silent no-op.
#[test]
fn an_unknown_kind_fails() {
    let out = Command::new(env!("CARGO_BIN_EXE_perch"))
        .args(["toast", "nope", "x"])
        .env("PERCH_NO_TMUX", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown toast kind"));
}
