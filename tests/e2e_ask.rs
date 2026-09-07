//! A question answered from the dashboard, end to end: the real `--ask` hook
//! waiting in a real pane's environment, the real TUI in another pane, and
//! the decision the hook prints for Claude.

#[path = "e2e/mod.rs"]
mod e2e;

use std::time::Duration;

use e2e::{fixture, wait_for, Server};

fn question_of(s: &Server, pane: &str) -> serde_json::Value {
    s.records()
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["pane"] == pane)
        .map(|r| r["question"].clone())
        .unwrap_or(serde_json::Value::Null)
}

/// Esc on the popup hands the question to Claude at once — the hook returns
/// with no decision and Claude's dialog takes over — and the board then shows
/// the pane as a `question` you jump to with Enter. No second popup: once a
/// question is Claude's, Claude's dialog is where it is answered.
#[test]
fn esc_hands_the_question_to_claude_and_enter_just_jumps() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let agent = s.pane_of("one:0");
    let host = s.new_shell_window("one", "host");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:1"]);
    let claims = || {
        std::fs::read_dir(s.state_dir.join("ask-popups"))
            .map(|d| d.flatten().count())
            .unwrap_or(0)
    };

    let hook = s
        .perch_in(&agent, &["hook", "claude", "--ask"])
        .stdin(&fixture("claude", "pre_tool_use_ask_user_question.json"))
        .spawn();
    assert!(wait_for(
        || question_of(&s, &agent)["tool_use_id"] == "toolu_01ask",
        Duration::from_secs(5)
    ));
    assert!(wait_for(|| claims() == 1, Duration::from_secs(4)));
    std::thread::sleep(Duration::from_millis(400));
    client.type_bytes(b"\x1b");
    let out = hook.wait_with_output().expect("hook exits on Esc");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "",
        "no decision: Claude's dialog takes over"
    );
    assert!(wait_for(|| claims() == 0, Duration::from_secs(4)));
    assert_eq!(s.state_of(&agent), "needs_input");
    assert!(question_of(&s, &agent).is_null());

    s.send_keys(
        &host,
        &[
            &e2e::perch_in_pane(&format!("tui --client '{}'", client.name)),
            "Enter",
        ],
    );
    assert!(
        wait_for(
            || {
                let c = s.capture(&host);
                c.contains("⚑ question") && c.contains("Which store backend")
            },
            Duration::from_secs(5)
        ),
        "the board should mark it as a question:\n{}",
        s.capture(&host)
    );
    assert!(!s.capture(&host).contains("a answer"));
    s.send_keys(&host, &["Enter"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == agent,
        Duration::from_secs(3)
    ));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(claims(), 0, "Claude's dialog is the dialog now");
}

/// The default: the form pops up on the client you are using, and you answer
/// it there. `send-keys` cannot reach a popup, so the keys go into the
/// client's own pty — the same bytes a keyboard would send.
#[test]
fn the_question_pops_up_on_the_client_and_is_answered_there() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let agent = s.pane_of("one:0");
    let _elsewhere = s.new_window("one", "elsewhere");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:1"]);

    let hook = s
        .perch_in(&agent, &["hook", "claude", "--ask"])
        .stdin(&fixture("claude", "pre_tool_use_ask_user_question.json"))
        .spawn();
    assert!(wait_for(
        || question_of(&s, &agent)["tool_use_id"] == "toolu_01ask",
        Duration::from_secs(5)
    ));
    // Give the detached popup a moment to be drawn before typing into it.
    std::thread::sleep(Duration::from_millis(1200));

    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    client.type_bytes(b" ");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b" ");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"\r");

    let out = hook.wait_with_output().expect("hook exits");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout:?}"));
    let input = &v["hookSpecificOutput"]["updatedInput"];
    assert_eq!(
        input["answers"]["Which store backend should perch use?"],
        "SQLite"
    );
    assert_eq!(
        input["answers"]["Which harnesses need the change?"],
        "claude, codex"
    );
    assert!(wait_for(
        || s.state_of(&agent) == "working",
        Duration::from_secs(3)
    ));
    // The popup closed with the answer: the client is back on its window.
    assert_eq!(s.client_pane(&client.name), _elsewhere);
}

/// The popup body on its own, in a pane, read back: the form fills it.
#[test]
fn the_popup_body_draws_the_form_and_closes_when_the_question_is_answered_elsewhere() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let agent = s.pane_of("one:0");
    let host = s.new_shell_window("one", "host");
    let _client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:1"]);
    // Card mode, so the hook does not open a popup of its own.
    std::fs::write(
        s.config_dir.join("config.toml"),
        "[ask]\npopup = \"card\"\n",
    )
    .unwrap();

    let hook = s
        .perch_in(&agent, &["hook", "claude", "--ask"])
        .stdin(&fixture("claude", "pre_tool_use_ask_user_question.json"))
        .spawn();
    assert!(wait_for(
        || question_of(&s, &agent)["tool_use_id"] == "toolu_01ask",
        Duration::from_secs(5)
    ));
    s.send_keys(
        &host,
        &[
            // A body of its own, on a client name of its own: the real
            // client already has the hook's popup, and one popup per client.
            &e2e::perch_in_pane(&format!("ask-form --pane '{agent}' --client nobody")),
            "Enter",
        ],
    );
    assert!(
        wait_for(
            || {
                let c = s.capture(&host);
                c.contains("Backend") && c.contains("1/2") && c.contains("A single database file")
            },
            Duration::from_secs(5)
        ),
        "the form never drew:\n{}",
        s.capture(&host)
    );
    assert!(!s.capture(&host).contains("last message"), "no board");

    // Answered from the dashboard elsewhere: the body notices and exits.
    let ans = serde_json::json!({
        "pane": agent, "tool_use_id": "toolu_01ask",
        "answers": {"Which store backend should perch use?": "Files"}
    });
    let dir = s.state_dir.join("answers");
    std::fs::create_dir_all(&dir).unwrap();
    let key: String = agent
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    std::fs::write(dir.join(format!("{key}.json")), ans.to_string()).unwrap();
    let out = hook.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("Files"));
    assert!(
        wait_for(
            || !s.capture(&host).contains("A single database file"),
            Duration::from_secs(3)
        ),
        "the body should exit once the question is gone:\n{}",
        s.capture(&host)
    );
}

/// Two agents ask at once. One popup: the first set, then — in the same
/// popup, no second `display-popup` — the second set, and both hooks get
/// their answers. Esc on the second puts it away, and the popup closes.
#[test]
fn simultaneous_questions_queue_into_one_popup() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let first = s.pane_of("one:0");
    let second = s.new_window("one", "second");
    let _elsewhere = s.new_window("one", "elsewhere");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:2"]);

    let hook_a = s
        .perch_in(&first, &["hook", "claude", "--ask"])
        .stdin(&fixture("claude", "pre_tool_use_ask_user_question.json"))
        .spawn();
    assert!(wait_for(
        || question_of(&s, &first)["tool_use_id"] == "toolu_01ask",
        Duration::from_secs(5)
    ));
    std::thread::sleep(Duration::from_millis(800));
    // The second question arrives while the first popup is up.
    let body_b = fixture("claude", "pre_tool_use_ask_user_question.json")
        .replace("toolu_01ask", "toolu_02ask")
        .replace(
            "Which store backend should perch use?",
            "Second agent: which colour?",
        );
    let hook_b = s
        .perch_in(&second, &["hook", "claude", "--ask"])
        .stdin(&body_b)
        .spawn();
    assert!(wait_for(
        || question_of(&s, &second)["tool_use_id"] == "toolu_02ask",
        Duration::from_secs(5)
    ));
    std::thread::sleep(Duration::from_millis(800));

    // Answer the first set: Files, then both harnesses.
    client.type_bytes(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    client.type_bytes(b" ");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b" ");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"\r");
    let out = hook_a.wait_with_output().unwrap();
    let a = String::from_utf8_lossy(&out.stdout);
    assert!(a.contains("\"Files\""), "{a}");

    // The same popup now shows the second set; answer it: Memory only.
    std::thread::sleep(Duration::from_millis(900));
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    client.type_bytes(b"\r");
    let out = hook_b.wait_with_output().unwrap();
    let b = String::from_utf8_lossy(&out.stdout);
    assert!(b.contains("\"Memory\""), "{b}");
    assert!(b.contains("Second agent: which colour?"), "{b}");
    assert!(wait_for(
        || s.state_of(&second) == "working",
        Duration::from_secs(3)
    ));
    assert_eq!(
        s.client_pane(&client.name),
        _elsewhere,
        "popup closed, client where it was"
    );
}
