//! `done` vs `idle` against a real tmux server with a real attached client.
//!
//! Two independent paths are pinned here:
//!
//! 1. **Reconciliation** — `perch list` re-evaluates seen from the live client
//!    list, so a `next-window` onto a done pane resolves it with *no perch
//!    hooks installed on that server at all*.
//! 2. **The hook** — sourcing the snippet makes it immediate, proved by
//!    reading the state file directly rather than through `perch list`, which
//!    would have done the reconciliation itself.

#[path = "e2e/mod.rs"]
mod e2e;

use std::time::Duration;

use e2e::{fixture, wait_for, Server};

fn f(name: &str) -> String {
    fixture("claude", name)
}

/// The state file on disk, without going through `perch list`.
fn state_on_disk(s: &Server, pane: &str) -> String {
    let key: String = pane
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let path = s.state_dir.join("panes").join(format!("{key}.json"));
    let Ok(body) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["state"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Whether the harness's pty-attached client carries tmux's `focused` flag.
///
/// It decides which half of the seen rule these tests exercise, so it is
/// asserted rather than assumed: a pty that never sends focus-in would leave
/// no client focused anywhere, and the "no focus info" fallback would be what
/// makes them pass.
fn client_is_focused(s: &Server, client: &str) -> bool {
    s.tmux_try(&["display", "-p", "-c", client, "#{client_flags}"])
        .split(',')
        .any(|f| f.trim() == "focused")
}

#[test]
fn seen_is_evaluated_from_the_live_client_list() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let a = s.pane_of("one:0");
    let b = s.new_window("one", "second");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:0"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == a,
        Duration::from_secs(2)
    ));

    // Which branch of the rule is live here. A pty-attached client on tmux
    // 3.6a does carry `focused`, so these tests exercise the primary branch,
    // not the "no focus info anywhere" fallback.
    assert!(
        client_is_focused(&s, &client.name),
        "the harness client does not report focus, so these tests would only \
         be exercising the no-focus-info fallback"
    );

    // (a) Stop while the client is showing the pane -> idle.
    s.hook(&a, "claude", &f("user_prompt_submit.json"));
    s.hook(&a, "claude", &f("stop.json"));
    assert_eq!(
        s.state_of(&a),
        "idle",
        "a turn that ends under your eyes is idle"
    );

    // (b) Stop while the client is on another window -> done.
    s.tmux(&["select-window", "-t", "one:1"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == b,
        Duration::from_secs(2)
    ));
    s.hook(&a, "claude", &f("user_prompt_submit.json"));
    s.hook(&a, "claude", &f("stop.json"));
    assert_eq!(s.state_of(&a), "done");

    // (c) Reconciliation. No perch hook is installed on this server, so tmux
    // tells perch nothing; `next-window` is not `select-window` either. The
    // next read is what notices.
    s.tmux(&["next-window", "-t", "one"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == a,
        Duration::from_secs(2)
    ));
    assert_eq!(
        state_on_disk(&s, &a),
        "done",
        "nothing has read the board yet, so nothing has reconciled"
    );
    assert_eq!(
        s.state_of(&a),
        "idle",
        "a read re-evaluates seen from the live client list"
    );
    // And it was written through, not merely reported.
    assert_eq!(state_on_disk(&s, &a), "idle");
    assert_eq!(s.pane_option(&a, "@perch_state"), "idle");
}

/// Sourcing the installed snippet makes the same transition immediate: the
/// state file flips within a second of a `next-window`, with nothing reading
/// the board.
#[test]
fn the_tmux_hooks_make_seen_immediate() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let a = s.pane_of("one:0");
    let b = s.new_window("one", "second");
    let client = s.attach("one");

    // The snippet as installed, with `perch` resolved to the test binary.
    let snippet = perch::install::TMUX_SNIPPET.replace("perch ", &format!("{} ", e2e::PERCH_BIN));
    let conf = s.config_dir.join("perch.tmux.conf");
    std::fs::write(&conf, snippet).unwrap();
    let out = s.tmux(&["source-file", conf.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "the snippet must source cleanly: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Park on the other window and finish a turn there: done.
    s.tmux(&["select-window", "-t", "one:1"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == b,
        Duration::from_secs(2)
    ));
    s.hook(&a, "claude", &f("user_prompt_submit.json"));
    s.hook(&a, "claude", &f("stop.json"));
    assert_eq!(state_on_disk(&s, &a), "done");

    // `next-window`, which fires none of the `after-select-*` hooks.
    s.tmux(&["next-window", "-t", "one"]);
    assert!(
        wait_for(|| state_on_disk(&s, &a) == "idle", Duration::from_secs(1)),
        "the tmux hook did not mark it seen; state is {:?}",
        state_on_disk(&s, &a)
    );
}
