//! Claude's hook payloads, driven through the real binary against a real
//! private tmux server with a real attached client.

#[path = "e2e/mod.rs"]
mod e2e;

use std::time::Duration;

use e2e::{fixture, wait_for, Server};

fn f(name: &str) -> String {
    fixture("claude", name)
}

/// The pane option is written by a spawned tmux, so it is polled, not read once.
fn assert_pane_state(s: &Server, pane: &str, want: &str) {
    let ok = wait_for(
        || s.pane_option(pane, "@perch_state") == want,
        Duration::from_secs(2),
    );
    assert!(
        ok,
        "@perch_state on {pane} is {:?}, wanted {want:?}",
        s.pane_option(pane, "@perch_state")
    );
}

#[test]
fn claude_hooks_drive_state_through_a_real_server() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let a = s.pane_of("one:0");
    let b = s.new_window("one", "second");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:0"]);

    s.hook(&a, "claude", &f("session_start.json"));
    assert_eq!(s.state_of(&a), "idle");
    assert_pane_state(&s, &a, "idle");

    // The hook records where the pane was, so an ended row can still say so.
    let loc = s.records().as_array().unwrap()[0]["location"].clone();
    assert_eq!(
        loc.as_str(),
        Some(
            s.tmux_out(&["display", "-p", "-t", &a, "#{window_name}"])
                .as_str()
        )
    );

    s.hook(&a, "claude", &f("user_prompt_submit.json"));
    assert_eq!(s.state_of(&a), "working");
    assert_pane_state(&s, &a, "working");

    // The client is sitting on this very pane: a finished turn is just idle.
    assert_eq!(s.client_pane(&client.name), a);
    s.hook(&a, "claude", &f("stop.json"));
    assert_eq!(
        s.state_of(&a),
        "idle",
        "a turn that ends under your eyes is idle"
    );

    // Now look somewhere else: the same Stop means done.
    s.tmux(&["select-window", "-t", "one:1"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == b,
        Duration::from_secs(2)
    ));
    s.hook(&a, "claude", &f("user_prompt_submit.json"));
    s.hook(&a, "claude", &f("stop.json"));
    assert_eq!(
        s.state_of(&a),
        "done",
        "a turn that ends while you are elsewhere is done"
    );
    assert_pane_state(&s, &a, "done");

    s.hook(&a, "claude", &f("notification_permission_prompt.json"));
    assert_eq!(s.state_of(&a), "needs_input");
    assert_pane_state(&s, &a, "needs_input");

    // Subagents live under the parent and never move it.
    s.hook(&a, "claude", &f("subagent_start.json"));
    assert_eq!(s.state_of(&a), "needs_input");
    s.hook(&a, "claude", &f("subagent_stop_event.json"));
    assert_eq!(s.state_of(&a), "needs_input");
    let recs = s.records();
    let rec = recs
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["pane"] == a.as_str())
        .unwrap()
        .clone();
    assert_eq!(rec["children"].as_array().unwrap().len(), 1);
    assert_eq!(rec["project"], "perch");

    s.hook(&a, "claude", &f("session_end.json"));
    assert_eq!(s.state_of(&a), "ended");
    assert_pane_state(&s, &a, "ended");

    // The events log carries every step, in order.
    let events = std::fs::read_to_string(s.state_dir.join("events.jsonl")).unwrap();
    assert!(events.contains("\"event\":\"session_start\""), "{events}");
    assert!(events.contains("\"event\":\"stop\""), "{events}");
    assert!(events.contains("\"event\":\"session_end\""), "{events}");

    // Only the two seeded panes exist; the hook invented nothing.
    let panes = s.tmux_out(&["list-panes", "-a", "-F", "#{pane_id}"]);
    assert_eq!(panes.lines().count(), 2, "{panes}");
}
