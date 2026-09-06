//! The hot path: raw harness JSON on stdin -> record, log, sound, tmux option.
//!
//! Nothing here may fail the process; a hook that exits non-zero is a hook the
//! user will disable.

use std::io::Read;

use chrono::Utc;
use serde_json::json;

use crate::adapters;
use crate::config;
use crate::model::{Harness, PaneRecord};
use crate::reducer;
use crate::sound;
use crate::store;
use crate::tmux;

/// Run the hook. Always returns; errors are reported on stderr by the caller.
pub fn run(harness: Harness) -> anyhow::Result<()> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let raw: serde_json::Value = serde_json::from_str(buf.trim()).unwrap_or(json!({}));
    dump_payload(harness, &buf);

    let Some(parsed) = adapters::parse(harness, &raw)? else {
        return Ok(());
    };

    let pane = match std::env::var("TMUX_PANE") {
        Ok(p) if p.starts_with('%') => p,
        _ => return Ok(()), // not inside tmux: nothing to key on
    };

    let now = store::now_rfc3339();
    let existing = store::load(&pane);
    let is_new = existing.is_none();
    let mut rec = existing.unwrap_or_else(|| PaneRecord::new(&pane, harness, &now));
    rec.harness = harness;
    // Only a Stop asks tmux where the user is looking; it is the one event
    // whose meaning depends on it, and the answer costs a round trip. That
    // same round trip also returns the pane's tmux location, so the record
    // remembers where it was once the pane is gone. A pane perch has not
    // located yet pays for one lookup of its own, and never again.
    let t = tmux::current();
    let is_stop = matches!(parsed.event, crate::model::Event::Stop { .. });
    // Only a Stop asks who is looking (`list-clients`, once); every other
    // event pays for at most the location lookup, and only the first time.
    let seen = is_stop && t.pane_seen_now(&pane);
    if is_stop || rec.location.is_none() {
        if let Some(loc) = t.pane_location(&pane) {
            rec.location = Some(loc);
        }
    }
    let applied = reducer::apply_with(&mut rec, &parsed, &now, seen);
    let changed = applied.parent_changed;

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

    if changed {
        cue(&rec, &pane);
    }
    Ok(())
}

/// Mark every `done` pane a focused client is now showing as `idle`.
///
/// This is what the tmux hooks call. It is the *immediate* path only: the same
/// rule runs inside [`store::snapshot`] on every read, so a switch tmux never
/// told perch about still resolves the next time anything looks at the board.
pub fn seen_all() -> usize {
    let t = tmux::current();
    store::mark_seen(t.as_ref(), &store::load_all())
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
    let _ = store::save(&rec);
    // Synchronous: `seen` is called from the TUI's popup and from `perch
    // next`, both of which exit immediately afterwards.
    tmux::current().run(&[opt("-p", pane, "@perch_state", "idle")]);
    true
}

/// The instant cue for a parent transition.
///
/// The sound is the notification; this writes the pane's `@perch_state` so a
/// user's own status line can show it. One spawned `tmux`, never waited on, so
/// the hook stays well inside its budget.
fn cue(rec: &PaneRecord, pane: &str) {
    tmux::current().batch(&[opt("-p", pane, "@perch_state", rec.state.as_str())]);
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
