//! `perch hook claude --ask`: the one hook that waits for the human.
//!
//! No tmux, no sound. The hook is driven as a real child process because the
//! whole point is what it prints on stdout and when it returns.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn fixture(name: &str) -> String {
    harness_fixture("claude", name)
}

fn harness_fixture(harness: &str, name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/{harness}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn perch(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_perch"));
    cmd.args(args)
        .env("PERCH_STATE_DIR", dir)
        .env("PERCH_CONFIG_DIR", dir.join("config"))
        .env("PERCH_NO_SOUND", "1")
        .env("PERCH_NO_TMUX", "1")
        .env("TMUX_PANE", "%999")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn spawn_with(mut cmd: Command, stdin: &str) -> Child {
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child
}

fn run(dir: &Path, args: &[&str], stdin: &str) -> (String, bool) {
    let out = spawn_with(perch(dir, args), stdin)
        .wait_with_output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.success(),
    )
}

fn record(dir: &Path) -> Option<serde_json::Value> {
    let body = std::fs::read_to_string(dir.join("panes/_999.json")).ok()?;
    serde_json::from_str(&body).ok()
}

fn wait_for(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    cond()
}

/// The record has a live question on it: the hook has written and is waiting.
fn wait_for_question(dir: &Path) {
    assert!(
        wait_for(
            || record(dir).is_some_and(|r| r["question"]["tool_use_id"] == "toolu_01ask"),
            Duration::from_secs(5)
        ),
        "the ask hook never recorded the question: {:?}",
        record(dir)
    );
}

fn write_answer(dir: &Path, body: serde_json::Value) {
    let d = dir.join("answers");
    std::fs::create_dir_all(&d).unwrap();
    let tmp = d.join(".999.tmp");
    std::fs::write(&tmp, body.to_string()).unwrap();
    std::fs::rename(tmp, d.join("_999.json")).unwrap();
}

fn finish(child: Child) -> String {
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "the hook must exit 0, whatever happened"
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The unmatched `PreToolUse` group also fires for `AskUserQuestion`; that
/// hook must neither block nor touch the record, or the two would race.
#[test]
fn the_plain_hook_ignores_an_ask_user_question_pre_tool_use() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let (out, ok) = run(
        p,
        &["hook", "claude"],
        &fixture("pre_tool_use_ask_user_question.json"),
    );
    assert!(ok);
    assert_eq!(out, "", "no decision from the plain hook");
    assert!(record(p).is_none(), "no record, no state change");
}

/// The contract with Claude: block, then print `updatedInput` carrying the
/// answers keyed by question text, multi-select joined with `, `.
#[test]
fn the_ask_hook_blocks_until_the_answer_file_and_prints_the_decision() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let child = spawn_with(
        perch(p, &["hook", "claude", "--ask"]),
        &fixture("pre_tool_use_ask_user_question.json"),
    );
    wait_for_question(p);
    let rec = record(p).unwrap();
    assert_eq!(rec["state"], "needs_input");
    assert_eq!(rec["last_message"], "Which store backend should perch use?");
    assert_eq!(rec["question"]["questions"][1]["header"], "Harnesses");
    // Still waiting: nothing on stdout yet is proven by the process being alive.
    std::thread::sleep(Duration::from_millis(300));
    assert!(record(p).unwrap()["question"].is_object());

    write_answer(
        p,
        serde_json::json!({
            "tool_use_id": "toolu_01ask",
            "answers": {
                "Which store backend should perch use?": "SQLite",
                "Which harnesses need the change?": "claude, codex"
            }
        }),
    );
    let out = finish(child);
    let v: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out:?}"));
    let hso = &v["hookSpecificOutput"];
    assert_eq!(hso["hookEventName"], "PreToolUse");
    assert_eq!(hso["permissionDecision"], "allow");
    let input = &hso["updatedInput"];
    assert_eq!(
        input["questions"].as_array().unwrap().len(),
        2,
        "the original input is carried through untouched"
    );
    assert_eq!(
        input["answers"]["Which store backend should perch use?"],
        "SQLite"
    );
    assert_eq!(
        input["answers"]["Which harnesses need the change?"],
        "claude, codex"
    );

    let rec = record(p).unwrap();
    assert_eq!(
        rec["state"], "working",
        "unblocked by perch, so working now"
    );
    assert!(rec["question"].is_null(), "answered, nothing pending");
    assert!(
        !p.join("answers/_999.json").exists(),
        "the answer file is consumed"
    );
    let events = std::fs::read_to_string(p.join("events.jsonl")).unwrap();
    assert!(events.contains("\"event\":\"question\""), "{events}");
}

/// "Answer it in the pane": the hook returns with no decision and Claude
/// draws its own dialog. The pane stays `needs_input` — it really is blocked.
#[test]
fn a_defer_releases_the_hook_with_no_decision() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let child = spawn_with(
        perch(p, &["hook", "claude", "--ask"]),
        &fixture("pre_tool_use_ask_user_question.json"),
    );
    wait_for_question(p);
    write_answer(
        p,
        serde_json::json!({ "tool_use_id": "toolu_01ask", "defer": true }),
    );
    let out = finish(child);
    assert_eq!(out.trim(), "", "no decision: the native dialog takes over");
    let rec = record(p).unwrap();
    assert_eq!(rec["state"], "needs_input");
    assert!(rec["question"].is_null(), "no longer perch's to answer");
}

/// An answer written for some other tool use is not this question's answer.
#[test]
fn an_answer_for_another_tool_use_is_ignored_and_the_hook_keeps_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let mut cmd = perch(p, &["hook", "claude", "--ask"]);
    cmd.env("PERCH_ASK_TIMEOUT_SECS", "2");
    let child = spawn_with(cmd, &fixture("pre_tool_use_ask_user_question.json"));
    wait_for_question(p);
    write_answer(
        p,
        serde_json::json!({ "tool_use_id": "toolu_stale", "answers": { "x": "y" } }),
    );
    let out = finish(child);
    assert_eq!(out.trim(), "", "timed out to the native dialog, no answer");
    assert!(
        !p.join("answers/_999.json").exists(),
        "the stale file is cleared"
    );
}

/// Looking at the pane changes nothing: perch holds the question and the
/// popup is the dialog, right there over the pane. Handing it to Claude is a
/// choice (`p`, or Enter to jump), never a side effect of where you look.
#[test]
fn the_ask_hook_holds_the_question_even_when_the_pane_is_seen() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let log = p.join("tmux.log");
    let mut cmd = perch(p, &["hook", "claude", "--ask"]);
    cmd.env("PERCH_FAKE_VIEWERS", "%999:focused")
        .env("PERCH_TMUX_LOG", &log);
    let child = spawn_with(cmd, &fixture("pre_tool_use_ask_user_question.json"));
    wait_for_question(p);
    std::thread::sleep(Duration::from_millis(300));
    assert!(record(p).unwrap()["question"].is_object(), "still held");
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        lines.contains("ask-popup %999"),
        "the popup is the dialog:\n{lines}"
    );
    write_answer(
        p,
        serde_json::json!({ "tool_use_id": "toolu_01ask", "answers": { "Which store backend should perch use?": "Files" } }),
    );
    let out = finish(child);
    assert!(out.contains("\"Files\""), "{out}");
}

/// The hook's own deadline, ahead of the harness's: expire quietly, leaving
/// the native dialog to appear and a record with nothing pending.
#[test]
fn the_ask_hook_times_out_to_the_native_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let mut cmd = perch(p, &["hook", "claude", "--ask"]);
    cmd.env("PERCH_ASK_TIMEOUT_SECS", "1");
    let start = Instant::now();
    let out = finish(spawn_with(
        cmd,
        &fixture("pre_tool_use_ask_user_question.json"),
    ));
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert!(start.elapsed() < Duration::from_secs(4));
    assert_eq!(out.trim(), "");
    let rec = record(p).unwrap();
    assert_eq!(rec["state"], "needs_input");
    assert!(rec["question"].is_null());
}

/// A question kind perch has no control for (a number slider) is never taken
/// from the harness: the hook defers immediately.
#[test]
fn a_number_question_is_left_to_the_native_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let start = Instant::now();
    let out = finish(spawn_with(
        perch(p, &["hook", "claude", "--ask"]),
        &fixture("pre_tool_use_ask_user_question_number.json"),
    ));
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(out.trim(), "");
    let rec = record(p).unwrap();
    assert_eq!(rec["state"], "needs_input");
    assert!(rec["question"].is_null());
}

/// `[ask] enabled = false` restores the old behaviour exactly: the ask hook
/// is a no-op and the native dialog is the only dialog.
#[test]
fn a_disabled_ask_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::create_dir_all(p.join("config")).unwrap();
    std::fs::write(p.join("config/config.toml"), "[ask]\nenabled = false\n").unwrap();
    let out = finish(spawn_with(
        perch(p, &["hook", "claude", "--ask"]),
        &fixture("pre_tool_use_ask_user_question.json"),
    ));
    assert_eq!(out.trim(), "");
    assert!(record(p).is_none(), "nothing recorded, nothing pending");
}

/// Walking over to the pane while the hook waits does not hand the question
/// back: the seen rule moves `done` to `idle` and leaves questions alone, so
/// they stay answerable with `a` (or the popup) until you choose otherwise.
#[test]
fn argument_less_seen_leaves_a_live_question_alone() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let child = spawn_with(
        perch(p, &["hook", "claude", "--ask"]),
        &fixture("pre_tool_use_ask_user_question.json"),
    );
    wait_for_question(p);
    let mut seen = perch(p, &["seen"]);
    seen.env("PERCH_FAKE_VIEWERS", "%999:focused");
    assert!(spawn_with(seen, "").wait().unwrap().success());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        record(p).unwrap()["question"].is_object(),
        "still perch's to answer"
    );
    write_answer(
        p,
        serde_json::json!({ "tool_use_id": "toolu_01ask", "defer": true }),
    );
    finish(child);
}

/// The installer's half of the contract: a second `PreToolUse` group matched
/// to the one tool, running the ask variant, with a timeout long enough to
/// wait for a human — and an existing perch install gains it on re-run.
#[test]
fn install_claude_adds_the_ask_group_with_a_long_timeout() {
    use perch::install::merge_claude_settings;
    let m = merge_claude_settings(serde_json::json!({}));
    let pre = m.settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2, "{pre:#?}");
    assert!(pre[0].get("matcher").is_none());
    assert_eq!(pre[0]["hooks"][0]["command"], "perch hook claude");
    assert_eq!(pre[1]["matcher"], "AskUserQuestion");
    assert_eq!(pre[1]["hooks"][0]["command"], "perch hook claude --ask");
    let t = pre[1]["hooks"][0]["timeout"].as_u64().unwrap();
    assert!(
        t > perch::reducer::ASK_WAIT_SECS as u64,
        "the harness must outwait the hook ({t} vs {})",
        perch::reducer::ASK_WAIT_SECS
    );

    // A 0.3.0 install has the plain group only; setup adds the ask group and
    // touches nothing else.
    let old = serde_json::json!({ "hooks": { "PreToolUse": [
        { "hooks": [{ "type": "command", "command": "perch hook claude", "timeout": 5 }] }
    ]}});
    let m = merge_claude_settings(old);
    let pre = m.settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2, "{pre:#?}");
    assert_eq!(pre[1]["matcher"], "AskUserQuestion");
    assert!(m.settings["hooks"]["PermissionRequest"].is_array());
    let again = merge_claude_settings(m.settings.clone());
    assert_eq!(again.settings, m.settings, "and the second run is a no-op");
    assert!(again.added.is_empty());
}

/// A question perch holds is put in front of you as the form itself, on
/// every attached client, never as the transient card.
#[test]
fn a_question_pops_up_as_a_form_not_a_card() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let log = p.join("tmux.log");
    let mut cmd = perch(p, &["hook", "claude", "--ask"]);
    cmd.env("PERCH_TMUX_LOG", &log)
        .env("PERCH_ASK_TIMEOUT_SECS", "1");
    finish(spawn_with(
        cmd,
        &fixture("pre_tool_use_ask_user_question.json"),
    ));
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(lines.contains("ask-popup %999"), "{lines}");
    assert!(
        !lines.contains("notify needs_input"),
        "no card as well:\n{lines}"
    );
}

/// A question handed straight back (a kind the form cannot take) gets the
/// ordinary card, never a form: Claude's dialog is the form.
#[test]
fn a_deferred_question_gets_the_card_not_the_form() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let log = p.join("tmux.log");
    let mut cmd = perch(p, &["hook", "claude", "--ask"]);
    cmd.env("PERCH_TMUX_LOG", &log);
    finish(spawn_with(
        cmd,
        &fixture("pre_tool_use_ask_user_question_number.json"),
    ));
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(lines.contains("notify needs_input %999"), "{lines}");
    assert!(!lines.contains("ask-popup"), "{lines}");
}

#[test]
fn the_ask_popup_is_a_centered_form_per_client_sized_to_the_question() {
    use perch::ask::{launch_command, popup_size};
    let q = perch::model::PendingQuestion {
        tool_use_id: "t".into(),
        asked_at: String::new(),
        deadline: String::new(),
        questions: vec![
            perch::model::Question {
                question: "one".into(),
                header: "A".into(),
                kind: "choice".into(),
                options: (0..4)
                    .map(|i| perch::model::QuestionOption {
                        label: format!("o{i}"),
                        description: String::new(),
                    })
                    .collect(),
                multi_select: false,
            },
            perch::model::Question {
                question: "two".into(),
                header: "B".into(),
                kind: "text".into(),
                options: vec![],
                multi_select: false,
            },
        ],
    };
    let (w, h) = popup_size(&q, None);
    assert!((60..=110).contains(&w), "{w}");
    // tabs, question, blank, 4 options, Other, blank, keys, plus the border:
    // the tallest question sets the height.
    assert_eq!(h, 4 + 6 + 2, "{h}");

    let argv = launch_command("/bin/perch", "/dev/ttys004", "%7", w, h);
    let s = argv.join(" ");
    assert!(s.starts_with("display-popup -c /dev/ttys004 "), "{s}");
    for want in [
        "-E",
        "-x C",
        "-y C",
        &format!("-w {w}"),
        &format!("-h {h}"),
        "-b rounded",
    ] {
        assert!(s.contains(want), "missing {want}: {s}");
    }
    assert!(
        s.ends_with("-- /bin/perch ask-form --pane %7 --client /dev/ttys004"),
        "{s}"
    );
}

fn pending_for(pane: &str, asked_at: &str, _dismissed: bool) -> perch::model::PaneRecord {
    use perch::model::*;
    let mut r = PaneRecord::new(pane, Harness::Claude, asked_at);
    r.state = State::NeedsInput;
    r.question = Some(PendingQuestion {
        tool_use_id: format!("t-{pane}"),
        asked_at: asked_at.into(),
        deadline: (chrono::Utc::now() + chrono::Duration::seconds(600)).to_rfc3339(),
        questions: vec![Question {
            question: format!("q for {pane}"),
            header: "H".into(),
            kind: "choice".into(),
            options: vec![],
            multi_select: false,
        }],
    });
    r
}

/// The queue: oldest live question first, skipping what this popup already
/// handled and what is stale.
#[test]
fn the_next_pending_question_is_the_oldest_unhandled_one() {
    use perch::ask::next_pending;
    let now = chrono::Utc::now();
    let mut stale = pending_for("%1", "2026-01-01T00:00:00Z", false);
    stale.question.as_mut().unwrap().deadline = "2026-01-01T00:01:00Z".into();
    let recs = vec![
        pending_for("%3", "2026-01-01T00:03:00Z", false),
        pending_for("%2", "2026-01-01T00:02:00Z", false),
        stale,
    ];
    let handled: std::collections::HashSet<String> = std::collections::HashSet::new();
    assert_eq!(
        next_pending(&recs, now, &handled).map(|r| r.pane.clone()),
        Some("%2".into())
    );
    let handled: std::collections::HashSet<String> = ["%2".to_string()].into_iter().collect();
    assert_eq!(
        next_pending(&recs, now, &handled).map(|r| r.pane.clone()),
        Some("%3".into())
    );
    let handled: std::collections::HashSet<String> =
        ["%2".to_string(), "%3".to_string()].into_iter().collect();
    assert!(
        next_pending(&recs, now, &handled).is_none(),
        "stale never comes back"
    );
}

/// One popup per client at a time: a claim is a file with the body's pid, a
/// second claim fails while that pid lives, and a dead pid's claim is stale.
#[test]
fn a_client_popup_claim_is_exclusive_while_its_owner_lives() {
    use perch::ask::{claim_popup, release_popup};
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert!(claim_popup(d, "/dev/ttys004", std::process::id()));
    assert!(
        !claim_popup(d, "/dev/ttys004", std::process::id() + 1),
        "held"
    );
    assert!(claim_popup(d, "/dev/ttys005", 1), "another client is free");
    release_popup(d, "/dev/ttys004");
    assert!(
        claim_popup(d, "/dev/ttys004", std::process::id()),
        "released"
    );
    // A pid that cannot exist any more: the claim is stale and can be taken.
    release_popup(d, "/dev/ttys004");
    assert!(claim_popup(d, "/dev/ttys004", 999_999_999));
    assert!(
        claim_popup(d, "/dev/ttys004", std::process::id()),
        "stale claim taken over"
    );
}

/// A second agent asking while a popup is up opens nothing new: the popup
/// already on that client will pick the question up when the first set is
/// done. `ask-popup` therefore skips a claimed client.
#[test]
fn ask_popup_skips_a_client_that_already_has_a_popup() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let log = p.join("tmux.log");
    // A record with a live question, and a claim on the only fake client.
    std::fs::create_dir_all(p.join("panes")).unwrap();
    let rec = pending_for("%999", &chrono::Utc::now().to_rfc3339(), false);
    std::fs::write(p.join("panes/_999.json"), serde_json::to_vec(&rec).unwrap()).unwrap();
    let mut cmd = perch(p, &["ask-popup", "--pane", "%999"]);
    cmd.env("PERCH_TMUX_LOG", &log)
        .env("PERCH_FAKE_VIEWERS", "%1:focused");
    assert!(spawn_with(cmd, "").wait().unwrap().success());
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        lines.contains("display-popup -c /dev/fake0"),
        "a free client gets the popup:\n{lines}"
    );

    std::fs::remove_file(&log).ok();
    assert!(perch::ask::claim_popup(
        &p.join("ask-popups"),
        "/dev/fake0",
        std::process::id()
    ));
    let mut cmd = perch(p, &["ask-popup", "--pane", "%999"]);
    cmd.env("PERCH_TMUX_LOG", &log)
        .env("PERCH_FAKE_VIEWERS", "%1:focused");
    assert!(spawn_with(cmd, "").wait().unwrap().success());
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !lines.contains("display-popup"),
        "claimed client, nothing opened:\n{lines}"
    );
}

/// A live question with no popup showing it — the client was busy with
/// another dialog, or the body died — gets one again on the next window
/// switch, through `perch seen`.
#[test]
fn argument_less_seen_reopens_the_popup_for_a_waiting_question() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let log = p.join("tmux.log");
    std::fs::create_dir_all(p.join("panes")).unwrap();
    let rec = pending_for("%999", &chrono::Utc::now().to_rfc3339(), false);
    std::fs::write(p.join("panes/_999.json"), serde_json::to_vec(&rec).unwrap()).unwrap();
    let mut seen = perch(p, &["seen"]);
    seen.env("PERCH_TMUX_LOG", &log)
        .env("PERCH_FAKE_VIEWERS", "%1:focused");
    assert!(spawn_with(seen, "").wait().unwrap().success());
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(lines.contains("ask-form --pane %999"), "{lines}");
}

/// A client looking at a pane that is itself blocked on input — Claude's own
/// dialog after `p`, say — never gets a popup over it. The question waits for
/// the next window switch.
#[test]
fn ask_popup_never_covers_a_client_dealing_with_another_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let log = p.join("tmux.log");
    std::fs::create_dir_all(p.join("panes")).unwrap();
    let asking = pending_for("%999", &chrono::Utc::now().to_rfc3339(), false);
    std::fs::write(
        p.join("panes/_999.json"),
        serde_json::to_vec(&asking).unwrap(),
    )
    .unwrap();
    let mut busy = perch::model::PaneRecord::new("%5", perch::model::Harness::Claude, "t0");
    busy.state = perch::model::State::NeedsInput;
    std::fs::write(p.join("panes/_5.json"), serde_json::to_vec(&busy).unwrap()).unwrap();

    // Two clients: one on the blocked pane %5, one elsewhere.
    for cmd in ["ask-popup", "seen"] {
        std::fs::remove_file(&log).ok();
        let mut c = perch(p, &[cmd, "--pane", "%999"]);
        if cmd == "seen" {
            c = perch(p, &["seen"]);
        }
        c.env("PERCH_TMUX_LOG", &log)
            .env("PERCH_FAKE_VIEWERS", "%5:focused,%7");
        assert!(spawn_with(c, "").wait().unwrap().success());
        let lines = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !lines.contains("display-popup -c /dev/fake0"),
            "{cmd}: the client on the blocked pane must be left alone:\n{lines}"
        );
        assert!(
            lines.contains("display-popup -c /dev/fake1"),
            "{cmd}: the other client gets it:\n{lines}"
        );
    }
}

/// The `PermissionRequest` route (auto mode): same wait, and the decision
/// comes back in that event's shape — `decision.behavior` + `updatedInput`.
#[test]
fn the_ask_hook_answers_a_permission_request_in_its_own_shape() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let child = spawn_with(
        perch(p, &["hook", "claude", "--ask"]),
        &fixture("permission_request_ask_user_question.json"),
    );
    assert!(
        wait_for(
            || record(p).is_some_and(|r| r["question"].is_object()),
            Duration::from_secs(5)
        ),
        "{:?}",
        record(p)
    );
    let id = record(p).unwrap()["question"]["tool_use_id"]
        .as_str()
        .unwrap()
        .to_string();
    write_answer(
        p,
        serde_json::json!({ "tool_use_id": id, "answers": { "Which store backend should perch use?": "Memory" } }),
    );
    let out = finish(child);
    let v: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out:?}"));
    let hso = &v["hookSpecificOutput"];
    assert_eq!(hso["hookEventName"], "PermissionRequest");
    assert_eq!(hso["decision"]["behavior"], "allow");
    assert_eq!(
        hso["decision"]["updatedInput"]["answers"]["Which store backend should perch use?"],
        "Memory"
    );
    assert!(
        hso.get("permissionDecision").is_none(),
        "not the PreToolUse shape"
    );
    let log = std::fs::read_to_string(p.join("ask.log")).unwrap_or_default();
    assert!(
        log.contains("PermissionRequest") && log.contains("answered"),
        "{log}"
    );
}

/// A question handed back once is not taken again for the same call: after
/// `PreToolUse` deferred (you were looking), the `PermissionRequest` that
/// follows for the very same questions returns at once — otherwise `p`
/// would jump you to the pane and a second popup would take the dialog away.
#[test]
fn a_deferred_question_is_not_taken_again_by_the_following_permission_request() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let child = spawn_with(
        perch(p, &["hook", "claude", "--ask"]),
        &fixture("pre_tool_use_ask_user_question.json"),
    );
    wait_for_question(p);
    write_answer(
        p,
        serde_json::json!({ "tool_use_id": "toolu_01ask", "defer": true }),
    );
    let out = finish(child);
    assert_eq!(out.trim(), "");
    assert_eq!(record(p).unwrap()["state"], "needs_input");

    // Now nobody is looking, but it is the same question: still handed back.
    let start = Instant::now();
    let log = p.join("tmux.log");
    let mut cmd = perch(p, &["hook", "claude", "--ask"]);
    cmd.env("PERCH_TMUX_LOG", &log);
    let out = finish(spawn_with(
        cmd,
        &fixture("permission_request_ask_user_question.json"),
    ));
    assert!(start.elapsed() < Duration::from_secs(3), "it must not wait");
    assert_eq!(out.trim(), "");
    assert!(record(p).unwrap()["question"].is_null());
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(!lines.contains("ask-popup"), "no popup either:\n{lines}");

    // A different question is a new question.
    let other = fixture("permission_request_ask_user_question.json").replace(
        "Which store backend should perch use?",
        "Something else entirely?",
    );
    let mut cmd = perch(p, &["hook", "claude", "--ask"]);
    cmd.env("PERCH_ASK_TIMEOUT_SECS", "1")
        .env("PERCH_TMUX_LOG", &log);
    finish(spawn_with(cmd, &other));
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        lines.contains("ask-popup"),
        "a new question is taken:\n{lines}"
    );
    let log = std::fs::read_to_string(p.join("ask.log")).unwrap_or_default();
    assert!(log.contains("deferred"), "{log}");
}

/// Every `--ask` invocation leaves one line in `ask.log`, whatever happened,
/// so a question that never reached the board can be explained afterwards.
#[test]
fn every_ask_invocation_is_traced() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let (out, ok) = run(p, &["hook", "claude", "--ask"], &fixture("stop.json"));
    assert!(ok && out.is_empty());
    let log = std::fs::read_to_string(p.join("ask.log")).unwrap_or_default();
    assert!(log.contains("Stop") && log.contains("ignored"), "{log}");
}

#[test]
fn install_claude_adds_the_permission_request_ask_group_too() {
    use perch::install::merge_claude_settings;
    let m = merge_claude_settings(serde_json::json!({}));
    let pr = m.settings["hooks"]["PermissionRequest"].as_array().unwrap();
    assert_eq!(pr.len(), 1, "{pr:#?}");
    assert_eq!(pr[0]["matcher"], "AskUserQuestion");
    assert_eq!(pr[0]["hooks"][0]["command"], "perch hook claude --ask");
    assert!(pr[0]["hooks"][0]["timeout"].as_u64().unwrap() > perch::reducer::ASK_WAIT_SECS as u64);
}

/// pi's route: the extension's `ask_user` tool sends its payload to
/// `perch hook pi --ask`, which waits like Claude's and answers in pi's
/// shape — a bare `{"answers": …}` the extension returns to the model.
#[test]
fn the_ask_hook_answers_a_pi_ask_user_call_in_its_own_shape() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let log = p.join("tmux.log");
    let mut cmd = perch(p, &["hook", "pi", "--ask"]);
    cmd.env("PERCH_TMUX_LOG", &log);
    let child = spawn_with(cmd, &harness_fixture("pi", "ask_user.json"));
    assert!(
        wait_for(
            || record(p).is_some_and(|r| r["question"]["tool_use_id"] == "pi-call-1"),
            Duration::from_secs(5)
        ),
        "{:?}",
        record(p)
    );
    assert_eq!(record(p).unwrap()["harness"], "pi");
    assert_eq!(record(p).unwrap()["state"], "needs_input");
    assert!(
        wait_for(
            || std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("ask-popup %999"),
            Duration::from_secs(3)
        ),
        "the popup, as for Claude:\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    write_answer(
        p,
        serde_json::json!({ "tool_use_id": "pi-call-1", "answers": { "Which store backend should perch use?": "Memory" } }),
    );
    let out = finish(child);
    let v: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out:?}"));
    assert_eq!(
        v["answers"]["Which store backend should perch use?"],
        "Memory"
    );
    assert!(
        v.get("hookSpecificOutput").is_none(),
        "pi's shape, not Claude's"
    );
    assert_eq!(record(p).unwrap()["state"], "working");

    // A defer prints nothing, and the extension falls back to pi's own dialog.
    let child = spawn_with(
        perch(p, &["hook", "pi", "--ask"]),
        &harness_fixture("pi", "ask_user.json").replace("pi-call-1", "pi-call-2"),
    );
    assert!(wait_for(
        || record(p).is_some_and(|r| r["question"]["tool_use_id"] == "pi-call-2"),
        Duration::from_secs(5)
    ));
    write_answer(
        p,
        serde_json::json!({ "tool_use_id": "pi-call-2", "defer": true }),
    );
    assert_eq!(finish(child).trim(), "");
}

/// The plain pi hook ignores an `ask_user` payload, exactly like Claude's.
#[test]
fn the_plain_pi_hook_ignores_an_ask_user_payload() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let (out, ok) = run(p, &["hook", "pi"], &harness_fixture("pi", "ask_user.json"));
    assert!(ok);
    assert_eq!(out, "");
    assert!(record(p).is_none());
}
