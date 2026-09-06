//! The sound half of the cue: which key a state asks for, the per-pane
//! cooldown, and the mute file.
//!
//! Nothing here can make a noise: every configured sound is a path that does
//! not exist, so `afplay` is never reached. What is asserted instead is
//! `last_sound_ms`, the stamp the hook writes in the same atomic save as the
//! transition — set exactly when perch decided to play.

use std::io::Write;
use std::process::{Command, Stdio};

use perch::config::Config;

/// A config whose sounds are absolute paths that do not exist, with a long
/// cooldown so the second chime of a test is always inside it.
const CONFIG: &str = r#"
cooldown_secs = 300

[sounds]
done = "/nonexistent/perch-test-done.aiff"
needs_input = "/nonexistent/perch-test-needs-input.aiff"
error = "/nonexistent/perch-test-error.aiff"
"#;

struct Env {
    dir: tempfile::TempDir,
    muted: bool,
    no_sound: bool,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(dir.path().join("config/config.toml"), CONFIG).unwrap();
        Env {
            dir,
            muted: false,
            no_sound: false,
        }
    }

    fn mute(mut self) -> Self {
        std::fs::write(self.dir.path().join("mute"), b"").unwrap();
        self.muted = true;
        self
    }

    fn no_sound(mut self) -> Self {
        self.no_sound = true;
        self
    }

    fn hook(&self, fixture: &str) {
        let body = std::fs::read_to_string(format!(
            "{}/tests/fixtures/claude/{fixture}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_perch"));
        cmd.args(["hook", "claude"])
            .env("PERCH_STATE_DIR", self.dir.path())
            .env("PERCH_CONFIG_DIR", self.dir.path().join("config"))
            .env("PERCH_NO_TMUX", "1")
            .env("TMUX_PANE", "%777")
            .env_remove("PERCH_NO_SOUND")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if self.no_sound {
            cmd.env("PERCH_NO_SOUND", "1");
        }
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success(), "a hook never fails");
    }

    /// The cooldown stamp on the record, `None` when perch played nothing.
    fn stamp(&self) -> Option<i64> {
        let body = std::fs::read_to_string(self.dir.path().join("panes/_777.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        v["last_sound_ms"].as_i64()
    }
}

/// Every state that chimes has a key, and nothing else does.
#[test]
fn only_done_needs_input_and_error_have_a_sound() {
    let cfg = Config::default();
    assert_eq!(cfg.sound_for("done"), Some("Glass"));
    assert_eq!(cfg.sound_for("needs_input"), Some("Ping"));
    assert_eq!(
        cfg.sound_for("error"),
        Some("Basso"),
        "the error kind is configurable even though no transition asks for it yet"
    );
    assert_eq!(cfg.sound_for("working"), None);
    assert_eq!(cfg.sound_for(""), None);
}

#[test]
fn a_second_chime_inside_the_cooldown_is_suppressed() {
    let e = Env::new();
    e.hook("user_prompt_submit.json");
    e.hook("stop.json");
    let first = e.stamp().expect("the first done chimed");

    // A second turn, finished immediately: a real transition, inside the
    // 300 s cooldown, so the pane stays quiet and the stamp does not move.
    e.hook("user_prompt_submit.json");
    e.hook("stop.json");
    assert_eq!(
        e.stamp(),
        Some(first),
        "the cooldown must swallow the second chime, stamp included"
    );
}

#[test]
fn the_mute_file_silences_the_hook() {
    let e = Env::new().mute();
    e.hook("user_prompt_submit.json");
    e.hook("stop.json");
    assert_eq!(e.stamp(), None, "a muted perch plays nothing at all");
}

#[test]
fn perch_no_sound_silences_the_hook() {
    let e = Env::new().no_sound();
    e.hook("user_prompt_submit.json");
    e.hook("stop.json");
    assert_eq!(e.stamp(), None);
}
