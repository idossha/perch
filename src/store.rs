use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::model::{PaneRecord, State};
use crate::tmux::Tmux;

const EVENTS_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Records whose pane is gone are pruned once they are this old.
const PRUNE_AFTER_SECS: i64 = 3600;

/// The on-disk state directory, `PERCH_STATE_DIR` when set.
pub fn state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("PERCH_STATE_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    crate::paths::home().join(".local/state/perch")
}

pub fn panes_dir() -> PathBuf {
    state_dir().join("panes")
}

pub fn events_path() -> PathBuf {
    state_dir().join("events.jsonl")
}

pub fn mute_path() -> PathBuf {
    state_dir().join("mute")
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn record_path(pane: &str) -> PathBuf {
    panes_dir().join(format!("{}.json", sanitize(pane)))
}

/// Pane ids are `%NN`; keep the file name free of separators regardless.
fn sanitize(pane: &str) -> String {
    pane.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Write a record atomically (tmp file in the same dir, then rename).
pub fn save(rec: &PaneRecord) -> Result<()> {
    let dir = panes_dir();
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let final_path = record_path(&rec.pane);
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        sanitize(&rec.pane),
        std::process::id()
    ));
    let body = serde_json::to_vec_pretty(rec)?;
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&body)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &final_path)?;
    Ok(())
}

pub fn load(pane: &str) -> Option<PaneRecord> {
    let body = fs::read(record_path(pane)).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Every record on disk, unsorted.
pub fn load_all() -> Vec<PaneRecord> {
    let Ok(entries) = fs::read_dir(panes_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(body) = fs::read(&p) {
            if let Ok(rec) = serde_json::from_slice::<PaneRecord>(&body) {
                out.push(rec);
            }
        }
    }
    out
}

pub fn remove(pane: &str) {
    let _ = fs::remove_file(record_path(pane));
}

/// Load every record, mark dead panes `ended`, prune the stale ones, and sort.
///
/// Sorted by state rank then by `since` ascending (oldest waiting first).
pub fn snapshot(tmux: &dyn Tmux) -> Vec<PaneRecord> {
    let live: Vec<String> = tmux.list_panes().into_iter().map(|p| p.pane).collect();
    let before = load_all();
    let states_before: Vec<(String, State)> =
        before.iter().map(|r| (r.pane.clone(), r.state)).collect();
    let after = reconcile(before, &live, Utc::now());

    // Persist the reconciliation: dead panes flip to `ended`, stale ones go away.
    for (pane, state) in &states_before {
        match after.iter().find(|r| &r.pane == pane) {
            None => remove(pane),
            Some(rec) if rec.state != *state => {
                let _ = save(rec);
            }
            Some(_) => {}
        }
    }
    after
}

/// Pure part of [`snapshot`], so liveness is testable with an injected pane list.
///
/// Does no I/O: a record whose pane is gone becomes `ended`, and one already
/// `ended` for over an hour is dropped from the result (the caller prunes it).
pub fn reconcile(
    records: Vec<PaneRecord>,
    live_ids: &[String],
    now: DateTime<Utc>,
) -> Vec<PaneRecord> {
    let mut out = Vec::new();
    for mut rec in records {
        if !live_ids.iter().any(|p| p == &rec.pane) {
            if rec.state != State::Ended {
                rec.state = State::Ended;
                rec.since = now.to_rfc3339();
            } else if age_secs(&rec.since, now) > PRUNE_AFTER_SECS {
                continue;
            }
        }
        out.push(rec);
    }
    out.sort_by(|a, b| {
        a.state
            .rank()
            .cmp(&b.state.rank())
            .then_with(|| a.since.cmp(&b.since))
            .then_with(|| a.pane.cmp(&b.pane))
    });
    out
}

/// Seconds since an RFC3339 timestamp; 0 when unparseable.
pub fn age_secs(since: &str, now: DateTime<Utc>) -> i64 {
    DateTime::parse_from_rfc3339(since)
        .map(|t| (now - t.with_timezone(&Utc)).num_seconds().max(0))
        .unwrap_or(0)
}

pub fn fmt_age(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// Append one line to `events.jsonl`, rotating past 5 MB.
pub fn append_event(value: &serde_json::Value) -> Result<()> {
    let dir = state_dir();
    fs::create_dir_all(&dir)?;
    let path = events_path();
    rotate_if_large(&path);
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    f.write_all(line.as_bytes())?;
    Ok(())
}

fn rotate_if_large(path: &Path) {
    if let Ok(md) = fs::metadata(path) {
        if md.len() >= EVENTS_MAX_BYTES {
            let _ = fs::rename(path, path.with_extension("jsonl.1"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Harness;

    #[test]
    fn fmt_age_units() {
        assert_eq!(fmt_age(5), "5s");
        assert_eq!(fmt_age(125), "2m");
        assert_eq!(fmt_age(7300), "2h");
        assert_eq!(fmt_age(200_000), "2d");
    }

    #[test]
    fn reconcile_sorts_and_ends_dead_panes() {
        let now = Utc::now();
        let ts = now.to_rfc3339();
        let mut a = PaneRecord::new("%1", Harness::Claude, &ts);
        a.state = State::Idle;
        let mut b = PaneRecord::new("%2", Harness::Claude, &ts);
        b.state = State::NeedsInput;
        let out = reconcile(vec![a, b], &["%1".into(), "%2".into()], now);
        assert_eq!(out[0].pane, "%2");
        assert_eq!(out[1].pane, "%1");
    }

    #[test]
    fn dead_pane_becomes_ended() {
        let now = Utc::now();
        let mut a = PaneRecord::new("%9", Harness::Claude, &now.to_rfc3339());
        a.state = State::Working;
        let out = reconcile(vec![a], &[], now);
        assert_eq!(out[0].state, State::Ended);
    }
}
