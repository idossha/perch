//! The real thing: a real interactive `claude` in a pane of the private tmux
//! server, asked to use its question tool, with the hooks exactly as installed
//! on this machine — so `perch hook claude --ask` is the installed binary, the
//! popup is the real popup on the test client, and the answer typed into it
//! is what Claude reports back.
//!
//! Uses the user's real Claude install and costs tokens: off unless
//! `PERCH_E2E_REAL=1`. The pane inherits this test's `PERCH_STATE_DIR`, so
//! the real hooks write here and never into the user's state.

#[path = "e2e/mod.rs"]
mod e2e;

use std::process::{Command, Stdio};
use std::time::Duration;

use e2e::{wait_for, Server};

fn enabled() -> bool {
    if std::env::var("PERCH_E2E_REAL").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("skipping: set PERCH_E2E_REAL=1 to run against the real Claude install");
    false
}

fn on_path(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn question_of(s: &Server, pane: &str) -> serde_json::Value {
    s.records()
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["pane"] == pane)
        .map(|r| r["question"].clone())
        .unwrap_or(serde_json::Value::Null)
}

/// Start `claude` in a fresh window and wait until its SessionStart hook has
/// written a record: the harness is up and its hooks reach this state dir.
fn start_claude(s: &Server, name: &str) -> String {
    let pane = s.new_shell_window("one", name);
    s.send_keys(&pane, &["claude", "Enter"]);
    assert!(
        wait_for(|| s.state_of(&pane) == "idle", Duration::from_secs(60)),
        "claude never reported SessionStart from {pane}:\n{}",
        s.capture(&pane)
    );
    // Let the prompt settle before typing into it.
    std::thread::sleep(Duration::from_millis(1500));
    pane
}

/// Type a prompt and submit it. A long burst followed at once by Enter reads
/// as a paste to Claude, which then waits; the pause makes it a typed line.
fn submit(s: &Server, pane: &str, text: &str) {
    s.send_keys(pane, &[text]);
    std::thread::sleep(Duration::from_millis(800));
    s.send_keys(pane, &["Enter"]);
}

/// A prompt that makes Claude ask exactly one known question and then echo
/// the answer in a form the test can grep for.
fn ask_prompt(tag: &str) -> String {
    format!(
        "Use the AskUserQuestion tool right now, once, with a single question: header 'Colour', question 'Which colour for {tag}?', options 'Red' and 'Blue' (single select). Do nothing else first. After I answer, reply with exactly one line: {tag}=<the label I chose> and stop."
    )
}

#[test]
fn a_real_claude_question_is_answered_through_the_popup() {
    if e2e::no_tmux() || !enabled() {
        return;
    }
    if !on_path("claude") {
        eprintln!("skipping: claude is not on PATH");
        return;
    }
    let s = Server::start();
    let _elsewhere = s.new_window("one", "elsewhere");
    let client = s.attach("one");
    let agent = start_claude(&s, "agent");
    // The user is elsewhere: perch takes the question.
    s.tmux(&["select-window", "-t", "one:1"]);

    submit(&s, &agent, &ask_prompt("COLOUR"));
    assert!(
        wait_for(
            || question_of(&s, &agent)["questions"][0]["question"]
                .as_str()
                .is_some_and(|q| q.contains("COLOUR")),
            Duration::from_secs(120)
        ),
        "the --ask hook never recorded the question; records: {}\npane:\n{}",
        s.records(),
        s.capture(&agent)
    );
    assert_eq!(s.state_of(&agent), "needs_input");
    // The popup is up on the client: pick Blue (second option) and send.
    std::thread::sleep(Duration::from_millis(1500));
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(200));
    client.type_bytes(b"\r");

    assert!(
        wait_for(
            || question_of(&s, &agent).is_null(),
            Duration::from_secs(10)
        ),
        "the question never cleared: {}",
        s.records()
    );
    assert!(
        wait_for(
            || s.capture(&agent).contains("COLOUR=Blue"),
            Duration::from_secs(120)
        ),
        "Claude did not report the answer perch sent:\n{}",
        s.capture(&agent)
    );
    assert!(
        !s.capture(&agent).contains("Which colour for COLOUR?\n"),
        "Claude should not have drawn its own dialog"
    );
    s.send_keys(&agent, &["/exit", "Enter"]);
}

/// Two real Claude sessions ask at once: one popup, both answered in turn.
#[test]
fn two_real_claude_questions_queue_into_one_popup() {
    if e2e::no_tmux() || !enabled() {
        return;
    }
    if !on_path("claude") {
        eprintln!("skipping: claude is not on PATH");
        return;
    }
    let s = Server::start();
    let _elsewhere = s.new_window("one", "elsewhere");
    let client = s.attach("one");
    let a = start_claude(&s, "agent-a");
    let b = start_claude(&s, "agent-b");
    s.tmux(&["select-window", "-t", "one:1"]);

    submit(&s, &a, &ask_prompt("FIRST"));
    submit(&s, &b, &ask_prompt("SECOND"));
    let asked = |pane: &str| {
        question_of(&s, pane)["questions"][0]["question"]
            .as_str()
            .is_some()
    };
    assert!(
        wait_for(|| asked(&a) && asked(&b), Duration::from_secs(120)),
        "both questions should be recorded: {}",
        s.records()
    );
    std::thread::sleep(Duration::from_millis(1500));

    // Whichever came first is up. Answer Blue; the other set follows in the
    // same popup; answer Red there.
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(200));
    client.type_bytes(b"\r");
    assert!(
        wait_for(
            || question_of(&s, &a).is_null() || question_of(&s, &b).is_null(),
            Duration::from_secs(10)
        ),
        "first answer never landed: {}",
        s.records()
    );
    std::thread::sleep(Duration::from_millis(1500));
    client.type_bytes(b"\r");
    assert!(
        wait_for(
            || question_of(&s, &a).is_null() && question_of(&s, &b).is_null(),
            Duration::from_secs(10)
        ),
        "second answer never landed: {}",
        s.records()
    );
    let both = wait_for(
        || {
            let ca = s.capture(&a);
            let cb = s.capture(&b);
            (ca.contains("FIRST=Blue") || ca.contains("FIRST=Red"))
                && (cb.contains("SECOND=Blue") || cb.contains("SECOND=Red"))
        },
        Duration::from_secs(120),
    );
    assert!(both, "a:\n{}\nb:\n{}", s.capture(&a), s.capture(&b));
    let ca = s.capture(&a);
    let cb = s.capture(&b);
    // Exactly one Blue and one Red across the two, in arrival order.
    let blues = usize::from(ca.contains("FIRST=Blue")) + usize::from(cb.contains("SECOND=Blue"));
    assert_eq!(blues, 1, "one Blue, one Red:\na:\n{ca}\nb:\n{cb}");
    s.send_keys(&a, &["/exit", "Enter"]);
    s.send_keys(&b, &["/exit", "Enter"]);
}
