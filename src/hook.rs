//! The hot path: raw harness JSON on stdin -> record, log, sound, tmux option.
//!
//! Nothing here may fail the process; a hook that exits non-zero is a hook the
//! user will disable.

use std::io::Read;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::json;

use crate::adapters;
use crate::config;
use crate::model::{Answer, Event, Harness, PaneRecord};
use crate::notify;
use crate::reducer;
use crate::sound;
use crate::store;
use crate::tmux;

/// Run the hook. Always returns; errors are reported on stderr by the caller.
///
/// `ask` is the `AskUserQuestion` variant, installed on its own matcher: it
/// records the question, waits for the dashboard to answer it, and prints
/// the decision for the harness. The plain hook fires for that payload too
/// (an unmatched `PreToolUse` group sees every tool) and must ignore it, or
/// the two would race over the same record.
pub fn run(harness: Harness, ask: bool) -> anyhow::Result<()> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let raw: serde_json::Value = serde_json::from_str(buf.trim()).unwrap_or(json!({}));
    dump_payload(harness, &buf);

    let Some(parsed) = adapters::parse(harness, &raw)? else {
        return Ok(());
    };
    let is_question = matches!(parsed.event, Event::Question { .. });
    let event_name = raw
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    if is_question != ask {
        if ask {
            trace_ask("-", &event_name, &raw, "ignored: not a question");
        }
        return Ok(());
    }
    if ask && !config::load().ask.enabled {
        trace_ask("-", &event_name, &raw, "ignored: [ask] disabled");
        return Ok(());
    }

    let pane = match std::env::var("TMUX_PANE") {
        Ok(p) if p.starts_with('%') => p,
        _ => {
            if ask {
                trace_ask("-", &event_name, &raw, "ignored: no TMUX_PANE");
            }
            return Ok(()); // not inside tmux: nothing to key on
        }
    };

    let now = store::now_rfc3339();
    let existing = store::load(&pane);
    let is_new = existing.is_none();
    let mut rec = existing.unwrap_or_else(|| PaneRecord::new(&pane, harness, &now));
    rec.harness = harness;
    // Claude asks twice for one dialog: `PreToolUse`, then `PermissionRequest`
    // once the first was handed back. A set perch already handed back for
    // this pane goes back again at once, or `p` would land you on the pane
    // and a second popup would take the dialog away from you.
    if let Event::Question { questions, .. } = &parsed.event {
        let key = crate::model::question_key(questions);
        if rec.deferred_question.as_deref() == Some(key.as_str()) {
            trace_ask(&pane, &event_name, &raw, "deferred: already handed back");
            return Ok(());
        }
    }
    // Only a Stop asks tmux where the user is looking; it is the one event
    // whose meaning depends on it, and the answer costs a round trip. That
    // same round trip also returns the pane's tmux location, so the record
    // remembers where it was once the pane is gone. A pane perch has not
    // located yet pays for one lookup of its own, and never again.
    let t = tmux::current();
    let is_stop = matches!(parsed.event, Event::Stop { .. });
    // Only a Stop asks who is looking (`list-clients`, once); every other
    // event pays for at most the location lookup, and only the first time.
    let seen = is_stop && t.pane_seen_now(&pane);
    if is_stop || rec.location.is_none() {
        if let Some(loc) = t.pane_location(&pane) {
            rec.location = Some(loc);
        }
    }
    let eff_before = rec.effective_state();
    let applied = reducer::apply_with(&mut rec, &parsed, &now, seen);
    let changed = applied.parent_changed;
    // A subagent event moves no pane state, but it can move the state the user
    // is *shown*: the last running child finishing ends the pane's delegating
    // spell. Worth the pane option, never worth a sound or a card.
    let eff_changed = rec.effective_state() != eff_before;

    // A tool call or an observed notice on a pane perch has never seen is not
    // worth inventing a record for; the next real event will make one.
    if is_new && !changed {
        return Ok(());
    }

    // Sound before the write, so the cooldown stamp lands in the same record.
    if let Some(key) = applied.sound {
        maybe_sound(&mut rec, key);
    }
    store::save(&rec)?;

    let _ = store::append_event(&json!({
        "ts": now,
        "pane": pane,
        "harness": harness.as_str(),
        "event": parsed.event.kind(),
        "state": rec.state.as_str(),
        "session_id": rec.session_id,
    }));

    // A question perch is about to hold gets the form itself as the popup on
    // every client; one handed straight back — a kind the form cannot take —
    // gets the ordinary card, because Claude's dialog is the form then. Where
    // you are looking plays no part: the popup is the dialog wherever you
    // are, the asking pane included, and it is the only place perch holds a
    // question. Closing it hands the question to Claude.
    let question_form = match &parsed.event {
        Event::Question { questions, .. } => !unanswerable(questions),
        _ => false,
    };
    if changed {
        cue(&rec, &pane, !question_form);
    } else if eff_changed {
        tmux::current().batch(&[opt(
            "-p",
            &pane,
            "@perch_state",
            rec.effective_state().as_str(),
        )]);
    }

    if let Event::Question {
        tool_use_id,
        questions,
    } = &parsed.event
    {
        // The popup is not tied to a state change: a question on a pane that
        // was already `needs_input` (a permission prompt, an earlier question
        // handed back) is still a new thing to answer.
        if question_form {
            crate::ask::spawn_detached(&pane);
        }
        // A number question is a slider perch has no control for: it goes
        // back at once.
        let why = if unanswerable(questions) {
            Some("deferred: unanswerable question kind")
        } else {
            None
        };
        let answer = match why {
            Some(_) => None,
            None => wait_for_answer(&pane, tool_use_id),
        };
        let outcome = match (&answer, why) {
            (Some(_), _) => "answered",
            (None, Some(w)) => w,
            (None, None) => "deferred: no answer (defer, timeout or gone)",
        };
        settle_question(&pane, &raw, answer, &crate::model::question_key(questions));
        trace_ask(&pane, &event_name, &raw, outcome);
    }
    Ok(())
}

/// One line per `--ask` invocation in `<state>/ask.log`: when, which pane,
/// which hook event, which tool, and what perch did with it. This is how a
/// question that never reached the board gets explained.
fn trace_ask(pane: &str, event_name: &str, raw: &serde_json::Value, outcome: &str) {
    let tool = raw.get("tool_name").and_then(|v| v.as_str()).unwrap_or("-");
    let mode = raw
        .get("permission_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let line = format!(
        "{} {} {} {} mode={} {}\n",
        store::now_rfc3339(),
        pane,
        event_name,
        tool,
        mode,
        outcome
    );
    let path = store::state_dir().join("ask.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

/// A question set the form has no control for: anything but a choice list or
/// a free-text line (Claude's `number` slider).
fn unanswerable(questions: &[crate::model::Question]) -> bool {
    questions
        .iter()
        .any(|q| q.kind != "choice" && q.kind != "text")
}

/// Block until the dashboard leaves an answer for this question, the record
/// stops carrying it (a `SessionEnd` or a new prompt arrived), or the wait
/// runs out. `None` means "no decision": the harness draws its own dialog.
fn wait_for_answer(pane: &str, tool_use_id: &str) -> Option<Answer> {
    let deadline = Instant::now() + Duration::from_secs(reducer::ask_wait_secs() as u64);
    loop {
        if let Some(ans) = store::take_answer(pane) {
            if ans.tool_use_id == tool_use_id {
                return (!ans.defer).then_some(ans);
            }
            // An answer to some other question: dropped, keep waiting.
        }
        let still_asked = store::load(pane)
            .and_then(|r| r.question)
            .is_some_and(|q| q.tool_use_id == tool_use_id);
        if !still_asked || Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Close the question on the record and, when it was answered, print the
/// decision the harness is waiting for. Nothing on stdout means "ask the
/// human yourself".
fn settle_question(pane: &str, raw: &serde_json::Value, answer: Option<Answer>, key: &str) {
    let now = store::now_rfc3339();
    let Some(mut rec) = store::load(pane) else {
        return;
    };
    match answer {
        Some(ans) => {
            reducer::answered(&mut rec, &now);
            let _ = store::save(&rec);
            let _ = store::append_event(&json!({
                "ts": now,
                "pane": pane,
                "harness": rec.harness.as_str(),
                "event": "answered",
                "state": rec.state.as_str(),
                "session_id": rec.session_id,
            }));
            tmux::current().batch(&[opt(
                "-p",
                pane,
                "@perch_state",
                rec.effective_state().as_str(),
            )]);
            println!("{}", ask_output(raw, &ans));
        }
        None => {
            rec.question = None;
            rec.deferred_question = Some(key.to_string());
            let _ = store::save(&rec);
        }
    }
}

/// The decision that satisfies an `AskUserQuestion`: allow, with the original
/// input plus `answers` keyed by question text — in the shape of whichever
/// hook event asked (`PreToolUse` in default mode, `PermissionRequest` in
/// auto mode).
pub fn ask_output(raw: &serde_json::Value, ans: &Answer) -> serde_json::Value {
    let mut input = raw.get("tool_input").cloned().unwrap_or_else(|| json!({}));
    if !input.is_object() {
        input = json!({});
    }
    input["answers"] = json!(ans.answers);
    // pi's extension sends `event: ask_user` and wants the answers alone.
    if raw.get("event").and_then(|v| v.as_str()) == Some("ask_user") {
        return json!({ "answers": ans.answers });
    }
    let event = raw
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("PreToolUse");
    if event == "PermissionRequest" {
        return json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {
                    "behavior": "allow",
                    "updatedInput": input,
                }
            }
        });
    }
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "answered in perch",
            "updatedInput": input,
        }
    })
}

/// Mark every `done` pane a focused client is now showing as `idle`.
///
/// This is what the tmux hooks call. It is the *immediate* path only: the same
/// rule runs inside [`store::snapshot`] on every read, so a switch tmux never
/// told perch about still resolves the next time anything looks at the board.
pub fn seen_all() -> usize {
    let t = tmux::current();
    let n = store::mark_seen(t.as_ref(), &store::load_all());
    // Looking away from a pane again: a question still waiting for a popup
    // gets one now (on a client that has none), so the queue moves on after
    // you answered one in its pane.
    if config::load().ask.enabled {
        crate::ask::reopen(t.as_ref());
    }
    n
}

/// Mark one named pane as seen, unconditionally: a finished turn you were just
/// sent to is just idle.
///
/// Called from `perch seen <pane>`, from `perch next` and from the TUI's jump,
/// which move the client there themselves and so need no evidence.
pub fn seen(pane: &str) -> bool {
    let Some(mut rec) = store::load(pane) else {
        return false;
    };
    if rec.state != crate::model::State::Done {
        return false;
    }
    rec.state = crate::model::State::Idle;
    rec.since = store::now_rfc3339();
    let shown = rec.effective_state().as_str().to_string();
    let _ = store::save(&rec);
    // Synchronous: `seen` is called from the TUI's popup and from `perch
    // next`, both of which exit immediately afterwards.
    tmux::current().run(&[opt("-p", pane, "@perch_state", &shown)]);
    true
}

/// The instant cue for a parent transition: the sound, the pane option, and
/// the card.
///
/// The pane's `@perch_state` lets a user put perch in a status line they wrote
/// themselves. The card is a *detached* `perch notify`, never waited on: the
/// popups it draws are its problem, and the hook is back inside its budget
/// whatever tmux does with them.
fn cue(rec: &PaneRecord, pane: &str, card: bool) {
    // Both the option and the card speak the *effective* state: a pane whose
    // turn ended while its subagents run on is still working, so it gets no
    // done card and reads `working` in a hand-written status line.
    let shown = rec.effective_state();
    tmux::current().batch(&[opt("-p", pane, "@perch_state", shown.as_str())]);
    if !card {
        return;
    }
    if let Some(kind) = notify::kind_for(shown) {
        if config::load().notify.enabled {
            notify::spawn_detached(kind, pane);
        }
    }
}

fn opt(scope: &str, pane: &str, name: &str, value: &str) -> Vec<String> {
    vec![
        "set-option".into(),
        scope.into(),
        "-t".into(),
        pane.into(),
        name.into(),
        value.into(),
    ]
}

/// Debug aid: with `PERCH_DUMP_HOOK_INPUT=<dir>`, drop every raw payload as
/// `<dir>/<harness>-<event>-<ts>.json`, so field mappings can be checked
/// against what a harness really sends. Best-effort and silent on failure.
fn dump_payload(harness: Harness, body: &str) {
    let Ok(dir) = std::env::var("PERCH_DUMP_HOOK_INPUT") else {
        return;
    };
    if dir.is_empty() {
        return;
    }
    let raw: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(json!({}));
    let event = ["hook_event_name", "event"]
        .iter()
        .find_map(|k| raw.get(*k).and_then(|v| v.as_str()))
        .unwrap_or("unknown")
        .replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let ts = Utc::now().format("%Y%m%dT%H%M%S%.3f");
    let dir = std::path::PathBuf::from(dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(
        dir.join(format!("{}-{}-{}.json", harness.as_str(), event, ts)),
        body,
    );
}

/// Play the state's sound unless this pane sounded within the cooldown.
fn maybe_sound(rec: &mut PaneRecord, key: &str) {
    let cfg = config::load();
    let now_ms = Utc::now().timestamp_millis();
    if let Some(last) = rec.last_sound_ms {
        if now_ms - last < cfg.cooldown_secs * 1000 {
            return;
        }
    }
    if !sound::enabled() {
        return;
    }
    rec.last_sound_ms = Some(now_ms);
    sound::play(&cfg, key);
}
