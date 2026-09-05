//! The hot path: raw harness JSON on stdin -> record, log, sound, tmux option.
//!
//! Nothing here may fail the process; a hook that exits non-zero is a hook the
//! user will disable.

use std::io::Read;

use chrono::Utc;
use serde_json::json;

use crate::adapters;
use crate::config;
use crate::model::{Harness, PaneRecord, State};
use crate::reducer;
use crate::sound;
use crate::store;
use crate::tmux;

/// Run the hook. Always returns; errors are reported on stderr by the caller.
pub fn run(harness: Harness) -> anyhow::Result<()> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let raw: serde_json::Value = serde_json::from_str(buf.trim()).unwrap_or(json!({}));

    let Some(parsed) = adapters::parse(harness, &raw)? else {
        return Ok(());
    };

    let pane = match std::env::var("TMUX_PANE") {
        Ok(p) if p.starts_with('%') => p,
        _ => return Ok(()), // not inside tmux: nothing to key on
    };

    let now = store::now_rfc3339();
    let mut rec = store::load(&pane).unwrap_or_else(|| PaneRecord::new(&pane, harness, &now));
    rec.harness = harness;
    let changed = reducer::apply(&mut rec, &parsed, &now);

    // Sound before the write, so the cooldown stamp lands in the same record.
    if changed {
        maybe_sound(&mut rec);
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
        tmux::current().set_pane_option(&pane, "@perch_state", rec.state.as_str());
    }
    Ok(())
}

/// Play the state's sound unless this pane sounded within the cooldown.
fn maybe_sound(rec: &mut PaneRecord) {
    let key = match rec.state {
        State::Done => "done",
        State::NeedsInput => "needs_input",
        _ => return,
    };
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
