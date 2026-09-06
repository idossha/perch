//! The notification card against a real tmux server with a real client.
//!
//! A popup cannot be listed — tmux exposes no `list-popups` — so the card is
//! observed through the process that draws it: `perch notify-body`, found by a
//! token unique to this test in its `--line` arguments. That it is running
//! means a popup is up; that it is gone means the popup closed.
//!
//! The client is attached here rather than through the shared harness because
//! these tests must *type* at it, which needs the pty master.

#[path = "e2e/mod.rs"]
mod e2e;

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use e2e::{wait_for, Server};
use portable_pty::{CommandBuilder, PtySize};

/// A client attached in a pty, keeping the writer so keys can be typed at it.
struct Typist {
    name: String,
    writer: Box<dyn Write + Send>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Drop for Typist {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn attach(server: &Server, session: &str) -> Typist {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new("tmux");
    cmd.args(["-L", &server.label, "attach", "-t", session]);
    cmd.env("TMUX_TMPDIR", server.tmux_tmpdir.display().to_string());
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("TMUX");
    let portable_pty::PtyPair { slave, master } = pair;
    let child = slave.spawn_command(cmd).expect("attach");
    drop(slave);

    let mut reader = master.try_clone_reader().expect("reader");
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    });
    let writer = master.take_writer().expect("writer");

    let mut name = String::new();
    wait_for(
        || {
            name = server.tmux_try(&["list-clients", "-F", "#{client_name}"]);
            !name.trim().is_empty()
        },
        Duration::from_secs(3),
    );
    let name = name.lines().next().unwrap_or("").trim().to_string();
    assert!(!name.is_empty(), "no client attached within 3s");
    Typist {
        name,
        writer,
        _master: master,
        child,
    }
}

/// Seed a record whose last message carries `token`, so the popup process is
/// identifiable in the process table.
fn seed(s: &Server, pane: &str, token: &str) {
    let dir = s.state_dir.join("panes");
    std::fs::create_dir_all(&dir).unwrap();
    let key: String = pane
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let body = serde_json::json!({
        "pane": pane,
        "harness": "claude",
        "state": "done",
        "since": "2026-01-01T00:00:00Z",
        "project": "perch",
        "branch": "main",
        "location": "editor",
        "last_message": token,
        "children": [],
    });
    std::fs::write(
        dir.join(format!("{key}.json")),
        serde_json::to_vec_pretty(&body).unwrap(),
    )
    .unwrap();
}

/// A short card, so the tests wait on seconds rather than the 3.5s default.
fn short_duration(s: &Server, ms: u64) {
    std::fs::write(
        s.config_dir.join("config.toml"),
        format!("[notify]\nduration_ms = {ms}\n"),
    )
    .unwrap();
}

/// `true` while a `perch notify-body` carrying `token` is running.
fn card_is_up(token: &str) -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", &format!("notify-body.*{token}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|st| st.success())
        .unwrap_or(false)
}

fn unique(tag: &str) -> String {
    format!("perchcard{tag}{}", std::process::id())
}

#[test]
fn a_card_goes_up_on_the_client_and_comes_down_on_its_own() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let pane = s.pane_of("one:0");
    let token = unique("auto");
    short_duration(&s, 1500);
    seed(&s, &pane, &token);
    let _client = attach(&s, "one");

    let out = s.perch(&["notify", "done", "--pane", &pane]).run();
    assert!(out.ok, "notify failed: {}", out.stderr);

    assert!(
        wait_for(|| card_is_up(&token), Duration::from_secs(1)),
        "no popup body running within 1s of `perch notify done`"
    );
    let up = Instant::now();
    assert!(
        wait_for(|| !card_is_up(&token), Duration::from_millis(1500 + 1000)),
        "the card outlived its duration"
    );
    assert!(
        up.elapsed() >= Duration::from_millis(500),
        "it vanished instantly"
    );
}

/// The passthrough guarantee, end to end: a key typed at a client showing the
/// card closes it *and* arrives in that client's active pane.
#[test]
fn a_key_typed_at_the_card_lands_in_the_pane_underneath() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let shell = s.new_shell_window("one", "shell");
    let token = unique("key");
    short_duration(&s, 8000);
    seed(&s, &shell, &token);
    let mut client = attach(&s, "one");
    s.tmux(&["select-window", "-t", "one:shell"]);
    assert!(
        wait_for(
            || s.client_pane(&client.name) == shell,
            Duration::from_secs(2)
        ),
        "the client never got to the shell pane"
    );

    let out = s.perch(&["notify", "done", "--pane", &shell]).run();
    assert!(out.ok, "notify failed: {}", out.stderr);
    assert!(
        wait_for(|| card_is_up(&token), Duration::from_secs(2)),
        "no popup body running"
    );

    client.writer.write_all(b"Z").unwrap();
    client.writer.flush().unwrap();

    assert!(
        wait_for(|| s.capture(&shell).contains('Z'), Duration::from_secs(3)),
        "the keystroke never reached the pane: {:?}",
        s.capture(&shell)
    );
    assert!(
        wait_for(|| !card_is_up(&token), Duration::from_secs(3)),
        "the card did not close on the first key (its duration was 8s)"
    );
}
