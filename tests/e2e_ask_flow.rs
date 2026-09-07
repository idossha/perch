//! The question popup's whole contract, on a real tmux server with the real
//! binary: put away, go to the pane, come back on a window switch through the
//! real tmux hooks, two clients, the title and the queue, and a question you
//! are already looking at.

#[path = "e2e/mod.rs"]
mod e2e;

use std::process::Child;
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

/// Start `perch hook claude --ask` for `pane` with a distinct question, and
/// wait until it has recorded it.
fn ask(s: &Server, pane: &str, tag: &str) -> Child {
    let body = fixture("claude", "pre_tool_use_ask_user_question.json")
        .replace("toolu_01ask", &format!("toolu_{tag}"))
        .replace(
            "Which store backend should perch use?",
            &format!("{tag}: which store backend?"),
        );
    let child = s
        .perch_in(pane, &["hook", "claude", "--ask"])
        .stdin(&body)
        .spawn();
    assert!(
        wait_for(
            || question_of(s, pane)["tool_use_id"] == format!("toolu_{tag}"),
            Duration::from_secs(5)
        ),
        "{tag} never recorded: {}",
        s.records()
    );
    child
}

/// The per-client popup claims on disk: one per popup body alive.
fn popups(s: &Server) -> usize {
    std::fs::read_dir(s.state_dir.join("ask-popups"))
        .map(|d| d.flatten().count())
        .unwrap_or(0)
}

fn wait_popups(s: &Server, n: usize) {
    assert!(
        wait_for(|| popups(s) == n, Duration::from_secs(4)),
        "expected {n} popup(s), have {}; bodies: {}",
        popups(s),
        String::from_utf8_lossy(
            &std::process::Command::new("sh")
                .arg("-c")
                .arg("ps -axo pid,etime,command | grep '[a]sk-form'")
                .output()
                .unwrap()
                .stdout
        )
    );
}

/// The installed tmux snippet, with `perch` resolved to the test binary.
fn wire_tmux_hooks(s: &Server) {
    let snippet = perch::install::TMUX_SNIPPET.replace("perch ", &format!("{} ", e2e::PERCH_BIN));
    let conf = s.config_dir.join("perch.tmux.conf");
    std::fs::write(&conf, snippet).unwrap();
    let out = s.tmux(&["source-file", conf.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn stdout_of(child: Child) -> String {
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Esc hands the set to Claude right away: the hook returns empty, the popup
/// closes everywhere, nothing reopens on a window switch, and the board shows
/// the pane as a `question` — with Enter to go there, and no key of its own.
#[test]
fn esc_hands_the_set_to_claude_and_nothing_brings_it_back() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    wire_tmux_hooks(&s);
    let agent = s.new_window("one", "agent");
    let _elsewhere = s.new_window("one", "elsewhere");
    let host = s.new_shell_window("one", "host");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:2"]);

    let hook = ask(&s, &agent, "A");
    wait_popups(&s, 1);
    std::thread::sleep(Duration::from_millis(400));
    client.type_bytes(b"\x1b");
    let out = stdout_of(hook);
    assert_eq!(out.trim(), "", "handed to Claude: no decision");
    wait_popups(&s, 0);
    assert!(question_of(&s, &agent).is_null());
    assert_eq!(
        s.state_of(&agent),
        "needs_input",
        "blocked on Claude's dialog"
    );

    // Look around, come back: the hooks fire, nothing reopens.
    s.tmux(&["select-window", "-t", "one:3"]);
    s.tmux(&["select-window", "-t", "one:1"]);
    s.tmux(&["select-window", "-t", "one:2"]);
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(popups(&s), 0);

    s.send_keys(
        &host,
        &[
            &e2e::perch_in_pane(&format!("tui --client '{}'", client.name)),
            "Enter",
        ],
    );
    assert!(
        wait_for(
            || s.capture(&host).contains("⚑ question"),
            Duration::from_secs(5)
        ),
        "{}",
        s.capture(&host)
    );
    assert!(!s.capture(&host).contains("a answer"));
    s.send_keys(&host, &["q"]);
}

/// `p`: the question goes back to Claude, the client lands on the pane, the
/// popup ends. The other agent's question waits; looking away from the pane
/// again — a real window switch, real tmux hooks — brings its popup up.
#[test]
fn p_goes_to_the_pane_and_the_next_question_returns_when_you_look_away() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    wire_tmux_hooks(&s);
    let a = s.new_window("one", "agent-a");
    let b = s.new_window("one", "agent-b");
    let elsewhere = s.new_window("one", "elsewhere");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:3"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == elsewhere,
        Duration::from_secs(2)
    ));

    let hook_a = ask(&s, &a, "A");
    wait_popups(&s, 1);
    let hook_b = ask(&s, &b, "B");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        popups(&s),
        1,
        "the second question queues behind the first popup"
    );

    client.type_bytes(b"p");
    let out = stdout_of(hook_a);
    assert_eq!(out.trim(), "", "deferred to Claude's own dialog");
    assert!(
        wait_for(|| s.client_pane(&client.name) == a, Duration::from_secs(3)),
        "client should be on agent-a, is on {:?}",
        s.client_pane(&client.name)
    );
    wait_popups(&s, 0);
    assert!(question_of(&s, &a).is_null());
    assert_eq!(
        question_of(&s, &b)["tool_use_id"],
        "toolu_B",
        "B still waits"
    );

    // Done in the pane; look away: B's popup appears through the tmux hook.
    s.tmux(&["select-window", "-t", "one:3"]);
    wait_popups(&s, 1);
    std::thread::sleep(Duration::from_millis(600));
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    client.type_bytes(b"\r");
    let out = stdout_of(hook_b);
    assert!(out.contains("\"SQLite\""), "{out}");
    wait_popups(&s, 0);
}

/// Two attached clients both get the popup; answering on one closes the
/// other, and the hook gets one answer.
#[test]
fn two_clients_each_get_the_popup_and_answering_on_one_closes_the_other() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let agent = s.new_window("one", "agent");
    let _elsewhere = s.new_window("one", "elsewhere");
    let c1 = s.attach("one");
    let c2 = s.attach("one");
    s.tmux(&["select-window", "-t", "one:2"]);

    let hook = ask(&s, &agent, "A");
    wait_popups(&s, 2);
    std::thread::sleep(Duration::from_millis(500));
    c1.type_bytes(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    c1.type_bytes(b"\r");
    let out = stdout_of(hook);
    assert!(out.contains("\"Files\""), "{out}");
    wait_popups(&s, 0);
    drop(c2);
}

/// The popup body, read back from a pane: the title names project, harness
/// and the pane's window and counts the queue; finishing one set shows the
/// next in place, with its own title.
#[test]
fn the_popup_names_its_owner_and_counts_the_queue_then_moves_on() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let a = s.new_window("one", "agent-a");
    let b = s.new_window("one", "agent-b");
    let host = s.new_shell_window("one", "host");
    let _client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:3"]);
    // Card mode: no popup of the hook's own, so the body can run in a pane.
    std::fs::write(
        s.config_dir.join("config.toml"),
        "[ask]\npopup = \"card\"\n",
    )
    .unwrap();

    let hook_a = ask(&s, &a, "A");
    std::thread::sleep(Duration::from_millis(50));
    let hook_b = ask(&s, &b, "B");
    s.send_keys(
        &host,
        &[
            // A body of its own, on a client name of its own: the real
            // client has the hook's popup, and one popup per client.
            &e2e::perch_in_pane(&format!("ask-form --pane '{a}' --client nobody")),
            "Enter",
        ],
    );
    assert!(
        wait_for(
            || {
                let c = s.capture(&host);
                c.contains("perch · claude · agent-a") && c.contains("+1 waiting")
            },
            Duration::from_secs(5)
        ),
        "{}",
        s.capture(&host)
    );
    let c = s.capture(&host);
    assert!(c.contains("A: which store backend?"), "{c}");
    assert!(c.contains("1/2"), "{c}");
    assert!(
        c.contains("Backend") && c.contains("Harnesses"),
        "tabs:\n{c}"
    );

    // Answer A: the body moves on to B, in the same pane, titled for B.
    s.send_keys(&host, &["Enter"]);
    std::thread::sleep(Duration::from_millis(200));
    s.send_keys(&host, &["Enter"]);
    let out = stdout_of(hook_a);
    assert!(out.contains("\"Files\""), "{out}");
    assert!(
        wait_for(
            || {
                let c = s.capture(&host);
                c.contains("perch · claude · agent-b") && c.contains("B: which store backend?")
            },
            Duration::from_secs(5)
        ),
        "{}",
        s.capture(&host)
    );
    assert!(
        !s.capture(&host).contains("waiting"),
        "{}",
        s.capture(&host)
    );
    s.send_keys(&host, &["j", "Enter"]);
    std::thread::sleep(Duration::from_millis(200));
    s.send_keys(&host, &["Enter"]);
    let out = stdout_of(hook_b);
    assert!(out.contains("\"SQLite\""), "{out}");
    // Queue empty: the body exits back to the shell.
    assert!(wait_for(
        || !s.capture(&host).contains("Harnesses"),
        Duration::from_secs(3)
    ));
}

/// A question that arrives while you are looking at the pane pops up right
/// there, over the pane, and is answered there; `p` in that popup is how
/// Claude's own dialog is chosen instead.
#[test]
fn a_question_you_are_looking_at_pops_up_over_the_pane() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let agent = s.new_window("one", "agent");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:1"]);
    assert!(wait_for(
        || s.client_pane(&client.name) == agent,
        Duration::from_secs(2)
    ));

    let hook = ask(&s, &agent, "A");
    wait_popups(&s, 1);
    std::thread::sleep(Duration::from_millis(500));
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    client.type_bytes(b"\r");
    let out = stdout_of(hook);
    assert!(out.contains("\"SQLite\""), "{out}");
    wait_popups(&s, 0);
    assert_eq!(s.state_of(&agent), "working");

    // And `p` from a popup over the pane hands it to Claude, staying put.
    let hook = ask(&s, &agent, "B");
    wait_popups(&s, 1);
    std::thread::sleep(Duration::from_millis(500));
    client.type_bytes(b"p");
    let out = stdout_of(hook);
    assert_eq!(out.trim(), "");
    wait_popups(&s, 0);
    assert_eq!(s.client_pane(&client.name), agent);
    assert_eq!(s.state_of(&agent), "needs_input");
}

/// Every harness that can ask goes through the same popup. Claude's payload
/// is exercised above; pi's `ask_user` and, for the record, Codex's
/// `request_user_input` (detected, not answerable) are exercised here on the
/// same real server: pi's question pops up and is answered through the client
/// pty; Codex's marks the pane as a question and opens nothing.
#[test]
fn pi_questions_pop_up_and_codex_questions_are_flagged() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let pi_pane = s.new_window("one", "pi-agent");
    let codex_pane = s.new_window("one", "codex-agent");
    let host = s.new_shell_window("one", "host");
    let client = s.attach("one");
    s.tmux(&["select-window", "-t", "one:3"]);

    let hook = s
        .perch_in(&pi_pane, &["hook", "pi", "--ask"])
        .stdin(&fixture("pi", "ask_user.json"))
        .spawn();
    assert!(wait_for(
        || question_of(&s, &pi_pane)["tool_use_id"] == "pi-call-1",
        Duration::from_secs(5)
    ));
    wait_popups(&s, 1);
    std::thread::sleep(Duration::from_millis(500));
    client.type_bytes(b"j");
    std::thread::sleep(Duration::from_millis(150));
    client.type_bytes(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    client.type_bytes(b"\r");
    let out = stdout_of(hook);
    let v: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out:?}"));
    assert_eq!(
        v["answers"]["Which store backend should perch use?"],
        "SQLite"
    );
    assert!(wait_for(
        || s.state_of(&pi_pane) == "working",
        Duration::from_secs(3)
    ));
    wait_popups(&s, 0);

    // Codex: a question perch can see but not answer.
    s.hook(
        &codex_pane,
        "codex",
        &fixture("codex", "pre_tool_use_request_user_input.json"),
    );
    assert_eq!(s.state_of(&codex_pane), "needs_input");
    assert_eq!(question_of(&s, &codex_pane), serde_json::Value::Null);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(popups(&s), 0, "nothing to answer through perch for codex");
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
                c.contains("codex")
                    && c.contains("⚑ needs_input")
                    && c.contains("request_user_input")
            },
            Duration::from_secs(5)
        ),
        "{}",
        s.capture(&host)
    );
    s.send_keys(&host, &["q"]);
}
