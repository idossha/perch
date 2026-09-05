//! pi's hook payloads (written by perch's own extension) against a real
//! private tmux server.

#[path = "e2e/mod.rs"]
mod e2e;

use std::time::Duration;

use e2e::{fixture, wait_for, Server};

fn f(name: &str) -> String {
    fixture("pi", name)
}

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
fn pi_hooks_drive_state_through_a_real_server() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let a = s.pane_of("one:0");
    let b = s.new_window("one", "second");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:0"]);

    s.hook(&a, "pi", &f("session_start.json"));
    assert_eq!(s.state_of(&a), "idle");
    assert_pane_state(&s, &a, "idle");

    s.hook(&a, "pi", &f("agent_start.json"));
    assert_eq!(s.state_of(&a), "working");
    assert_pane_state(&s, &a, "working");

    // A tool call is not a state change and must not disturb the record.
    s.hook(&a, "pi", &f("tool_call.json"));
    assert_eq!(s.state_of(&a), "working");

    assert_eq!(s.client_pane(&client.name), a);
    s.hook(&a, "pi", &f("agent_end.json"));
    assert_eq!(s.state_of(&a), "idle", "watched turns end idle");

    s.tmux(&["select-window", "-t", "one:1"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == b,
        Duration::from_secs(2)
    ));
    s.hook(&a, "pi", &f("agent_start.json"));
    s.hook(&a, "pi", &f("agent_end.json"));
    assert_eq!(s.state_of(&a), "done", "unwatched turns end done");
    assert_pane_state(&s, &a, "done");

    // `perch seen` is what says "I have looked at it".
    assert!(s.perch(&["seen", &a]).run().ok);
    assert_eq!(s.state_of(&a), "idle");
    assert_pane_state(&s, &a, "idle");

    s.hook(&a, "pi", &f("session_shutdown.json"));
    assert_eq!(s.state_of(&a), "ended");
    assert_pane_state(&s, &a, "ended");
}
