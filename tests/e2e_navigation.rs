//! Moving a real attached client: the TUI's Enter and `perch next`.
//!
//! The dashboard is driven inside a normal pane rather than a `display-popup`
//! because a popup is not a pane and `send-keys` cannot address it. The command
//! is byte-for-byte the one the popup runs (`perch tui --client <name>`), and
//! `perch open --client <name>` is what wraps it in the popup.

#[path = "e2e/mod.rs"]
mod e2e;

use std::time::Duration;

use e2e::{seed_record, wait_for, Server, PERCH_BIN};

/// Start the dashboard in `host` and wait until it has drawn.
fn start_tui(s: &Server, host: &str, client: &str) {
    s.send_keys(
        host,
        &[&format!("{PERCH_BIN} tui --client '{client}'"), "Enter"],
    );
    assert!(
        wait_for(
            || s.capture(host).contains("last message"),
            Duration::from_secs(5)
        ),
        "TUI never drew:\n{}",
        s.capture(host)
    );
}

#[test]
fn enter_moves_the_client_to_the_selected_pane_and_marks_it_seen() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let b = s.new_window("one", "work");
    let host = s.new_shell_window("one", "host");
    let client = s.attach("one");

    seed_record(&s.state_dir, &b, "work", "done", "2026-01-01T00:00:00Z");

    // The client is looking at the host window, not at B.
    s.tmux(&["select-window", "-t", "one:2"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == host,
        Duration::from_secs(2)
    ));

    start_tui(&s, &host, &client.name);
    assert!(wait_for(
        || s.capture(&host).contains("done"),
        Duration::from_secs(3)
    ));

    s.send_keys(&host, &["Enter"]);
    assert!(
        wait_for(|| s.client_pane(&client.name) == b, Duration::from_secs(2)),
        "the client never moved to {b}; it is on {}",
        s.client_pane(&client.name)
    );
    assert!(
        wait_for(|| s.state_of(&b) == "idle", Duration::from_secs(2)),
        "jumping to a done pane marks it seen, got {:?}",
        s.state_of(&b)
    );
}

#[test]
fn next_lands_on_the_oldest_waiting_pane_across_sessions() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let here = s.pane_of("one:0");
    let far = s.new_session("two");
    let client = s.attach("one");

    // The older wait lives in the other session.
    seed_record(
        &s.state_dir,
        &far,
        "far",
        "needs_input",
        "2026-01-01T00:00:00Z",
    );
    seed_record(
        &s.state_dir,
        &here,
        "here",
        "needs_input",
        "2026-01-01T00:05:00Z",
    );

    let out = s.perch(&["next", "--client", &client.name]).run();
    assert!(out.ok, "perch next failed: {}", out.stderr);
    assert!(
        out.stdout.contains(&far),
        "next chose {out:?}",
        out = out.stdout
    );
    assert!(
        wait_for(
            || s.client_pane(&client.name) == far,
            Duration::from_secs(2)
        ),
        "the client is on {}, not the oldest wait {far}",
        s.client_pane(&client.name)
    );
}

#[test]
fn a_pane_that_died_before_enter_is_reported_gone_and_moves_nobody() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let b = s.new_window("one", "work");
    let host = s.new_shell_window("one", "host");
    let client = s.attach("one");

    seed_record(&s.state_dir, &b, "work", "done", "2026-01-01T00:00:00Z");
    s.tmux(&["select-window", "-t", "one:2"]);
    start_tui(&s, &host, &client.name);
    assert!(wait_for(
        || s.capture(&host).contains("work"),
        Duration::from_secs(3)
    ));

    // The pane dies after the row was drawn, before the jump.
    s.tmux(&["kill-window", "-t", "one:1"]);
    s.send_keys(&host, &["Enter"]);

    assert!(
        wait_for(
            || s.capture(&host).contains(&format!("pane {b} is gone")),
            Duration::from_secs(3)
        ),
        "the TUI did not report the pane gone:\n{}",
        s.capture(&host)
    );
    assert_eq!(
        s.client_pane(&client.name),
        host,
        "a dead target must not move the client"
    );
}
