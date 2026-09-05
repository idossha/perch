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
    // whose meaning depends on it, and the answer costs a round trip.
    let focused = matches!(parsed.event, crate::model::Event::Stop { .. })
        && tmux::current().pane_focused(&pane);
    let applied = reducer::apply_with(&mut rec, &parsed, &now, focused);
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

/// Mark a pane as seen: a finished turn you are now looking at is just idle.
///
/// Called from the tmux `after-select-window` / `after-select-pane` / `client-session-changed` hooks, from `perch next` and from the TUI's jump, so
/// `done` means "finished while you were elsewhere" everywhere.
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
    let t = tmux::current();
    t.batch(&[
        opt("-p", pane, "@perch_state", "idle"),
        opt("-w", pane, "@perch_flag", ""),
    ]);
    true
}

/// The instant cue for a parent transition: the pane's `@perch_state`, the
/// window's `@perch_flag`, and a flash on every attached client.
///
/// All of it goes out as one `tmux a ; b ; c` invocation, spawned and not
/// waited on, so the hook stays well inside its budget even with several
/// clients attached.
fn cue(rec: &PaneRecord, pane: &str) {
    let t = tmux::current();
    let mut cmds: Vec<Vec<String>> = vec![
        opt("-p", pane, "@perch_state", rec.state.as_str()),
        opt("-w", pane, "@perch_flag", flag_for(rec.state)),
    ];

    let cfg = config::load();
    if cfg.notify.tmux_message {
        if let Some(msg) = message_for(rec) {
            let ms = cfg.notify.duration_ms.to_string();
            for client in t.list_clients() {
                cmds.push(vec![
                    "display-message".into(),
                    "-c".into(),
                    client,
                    "-d".into(),
                    ms.clone(),
                    msg.clone(),
                ]);
            }
        }
    }
    t.batch(&cmds);
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

/// The window-status marker for a state; every other state clears it.
pub fn flag_for(state: crate::model::State) -> &'static str {
    match state {
        crate::model::State::NeedsInput => "⚑",
        crate::model::State::Done => "✓",
        _ => "",
    }
}

/// The one-line flash, or `None` for a state that is not worth interrupting for.
fn message_for(rec: &PaneRecord) -> Option<String> {
    let project = rec.project.clone().unwrap_or_else(|| rec.pane.clone());
    let harness = rec.harness.as_str();
    match rec.state {
        crate::model::State::NeedsInput => Some(format!(
            "#[fg=red,bold]⚑ {project} ({harness}) needs input — prefix N jumps"
        )),
        crate::model::State::Done => Some(format!("#[fg=green]✓ {project} ({harness}) done")),
        _ => None,
    }
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
    let project = rec.project.clone().unwrap_or_else(|| rec.pane.clone());
    sound::notify(
        &cfg,
        &format!("perch: {}", rec.state.as_str()),
        &format!(
            "{} {}",
            project,
            rec.last_message.clone().unwrap_or_default()
        ),
    );
}
