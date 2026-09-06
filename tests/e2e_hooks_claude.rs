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

/// The subagents `perch list --json` reports for one pane.
fn children_of(s: &Server, pane: &str) -> Vec<serde_json::Value> {
    s.records()
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["pane"] == pane)
        .map(|r| r["children"].as_array().cloned().unwrap_or_default())
        .unwrap_or_default()
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

    // A fan-out of two, then the parent's turn ends. Claude runs subagents in
    // the background: the parent's own turn is over, but the pane is still
    // delegating, so everything the user sees says working.
    s.tmux(&["select-window", "-t", "one:1"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == b,
        Duration::from_secs(2)
    ));
    s.hook(&a, "claude", &f("user_prompt_submit.json"));
    s.hook(&a, "claude", &f("subagent_start.json"));
    s.hook(
        &a,
        "claude",
        &f("subagent_start.json").replace("sub-7", "sub-8"),
    );
    let kids = children_of(&s, &a);
    assert_eq!(kids.len(), 2, "{kids:?}");
    assert!(kids.iter().all(|c| c["state"] == "working"), "{kids:?}");

    s.hook(&a, "claude", &f("stop.json"));
    assert_eq!(s.state_of(&a), "done", "the parent's own turn did end");
    assert_eq!(
        s.effective_of(&a),
        "working",
        "its subagents did not: the pane is delegating"
    );
    let kids = children_of(&s, &a);
    assert_eq!(kids.len(), 2, "{kids:?}");
    assert!(
        kids.iter().all(|c| c["state"] == "working"),
        "a parent stop no longer retires its children: {kids:?}"
    );
    assert_pane_state(&s, &a, "working");

    // Both subagents finish: nothing is waiting on them any more, so the pane
    // reads as what it is.
    s.hook(&a, "claude", &f("subagent_stop_event.json"));
    assert_eq!(s.effective_of(&a), "working", "sub-8 is still running");
    s.hook(
        &a,
        "claude",
        &f("subagent_stop_event.json").replace("sub-7", "sub-8"),
    );
    assert_eq!(s.effective_of(&a), s.state_of(&a));
    assert_eq!(s.state_of(&a), "done");
    assert_pane_state(&s, &a, "done");

    // Look at the pane again: it is idle, and finished children have nothing
    // left to say, so they are gone from the record.
    s.tmux(&["select-window", "-t", "one:0"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == a,
        Duration::from_secs(2)
    ));
    assert!(wait_for(
        || s.state_of(&a) == "idle" && children_of(&s, &a).is_empty(),
        Duration::from_secs(3)
    ));
    assert!(children_of(&s, &a).is_empty(), "{:?}", children_of(&s, &a));

    // A resumed background agent: `SendMessage` is the only signal, and the
    // parent legitimately goes done while the agent keeps running.
    s.tmux(&["select-window", "-t", "one:1"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == b,
        Duration::from_secs(2)
    ));
    s.hook(&a, "claude", &f("user_prompt_submit.json"));
    s.hook(&a, "claude", &f("pre_tool_use_send_message.json"));
    let kids = children_of(&s, &a);
    assert_eq!(kids.len(), 1, "{kids:?}");
    assert_eq!(kids[0]["id"], "a41eb56e05dc8146f");
    assert_eq!(kids[0]["state"], "working");

    s.hook(&a, "claude", &f("stop.json"));
    assert_eq!(s.state_of(&a), "done");
    assert_eq!(
        s.effective_of(&a),
        "working",
        "a resumed agent keeps the pane delegating"
    );
    assert_pane_state(&s, &a, "working");
    assert!(
        children_of(&s, &a)[0]["state"] == "working",
        "{:?}",
        children_of(&s, &a)
    );

    // A helper agent's stop is logged and dropped; the resumed agent's is not.
    s.hook(&a, "claude", &f("subagent_stop_helper.json"));
    assert_eq!(children_of(&s, &a).len(), 1, "no child for a helper stop");
    s.hook(
        &a,
        "claude",
        &f("subagent_stop_helper.json")
            .replace("helper-1", "a41eb56e05dc8146f")
            .replace("\"agent_type\":\"\"", "\"agent_type\":\"Explore\""),
    );
    assert_eq!(s.effective_of(&a), "done");
    assert_eq!(s.state_of(&a), "done");
    assert_pane_state(&s, &a, "done");

    // Back to the pane, so the next steps start from a clean record.
    s.tmux(&["select-window", "-t", "one:0"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == a,
        Duration::from_secs(2)
    ));
    assert!(wait_for(
        || s.state_of(&a) == "idle" && children_of(&s, &a).is_empty(),
        Duration::from_secs(3)
    ));

    s.hook(&a, "claude", &f("session_end.json"));
    assert_eq!(s.state_of(&a), "ended");
    assert_pane_state(&s, &a, "ended");

    // The events log carries every step, in order.
    let events = std::fs::read_to_string(s.state_dir.join("events.jsonl")).unwrap();
    assert!(events.contains("\"event\":\"session_start\""), "{events}");
    assert!(events.contains("\"event\":\"stop\""), "{events}");
    assert!(events.contains("\"event\":\"session_end\""), "{events}");
    assert!(
        events.contains("\"event\":\"subagent_resume\""),
        "a resume is logged: {events}"
    );
    assert!(
        events.contains("\"event\":\"subagent_stop\""),
        "so is the helper stop it dropped: {events}"
    );

    // Only the two seeded panes exist; the hook invented nothing.
    let panes = s.tmux_out(&["list-panes", "-a", "-F", "#{pane_id}"]);
    assert_eq!(panes.lines().count(), 2, "{panes}");
}
