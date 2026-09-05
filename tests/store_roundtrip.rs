//! Store tests. Every one redirects PERCH_STATE_DIR at a tempdir; none touches
//! the real home, tmux or the sound player.

use chrono::{Duration, Utc};
use perch::model::{Harness, PaneRecord, State};
use perch::store;

/// Env is process-wide, so store tests share one mutex and one temp dir.
fn with_state_dir<T>(f: impl FnOnce() -> T) -> T {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("PERCH_STATE_DIR", dir.path());
    std::env::set_var("PERCH_NO_SOUND", "1");
    std::env::set_var("PERCH_NO_TMUX", "1");
    let out = f();
    std::env::remove_var("PERCH_STATE_DIR");
    out
}

fn rec(pane: &str, state: State, since: &str) -> PaneRecord {
    let mut r = PaneRecord::new(pane, Harness::Claude, since);
    r.state = state;
    r.project = Some("perch".into());
    r
}

#[test]
fn save_load_round_trip() {
    with_state_dir(|| {
        let mut r = rec("%42", State::Done, &store::now_rfc3339());
        r.last_message = Some("finished".into());
        r.session_id = Some("s-9".into());
        store::save(&r).unwrap();

        let back = store::load("%42").expect("record on disk");
        assert_eq!(back.pane, "%42");
        assert_eq!(back.state, State::Done);
        assert_eq!(back.last_message.as_deref(), Some("finished"));
        assert_eq!(back.session_id.as_deref(), Some("s-9"));
        assert_eq!(back.harness, Harness::Claude);

        assert_eq!(store::load_all().len(), 1);
        store::remove("%42");
        assert!(store::load("%42").is_none());
    });
}

#[test]
fn save_is_atomic_and_leaves_no_temp_files() {
    with_state_dir(|| {
        let r = rec("%1", State::Working, &store::now_rfc3339());
        store::save(&r).unwrap();
        store::save(&r).unwrap();
        let names: Vec<String> = std::fs::read_dir(store::panes_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["_1.json".to_string()]);
    });
}

#[test]
fn events_are_appended_as_jsonl() {
    with_state_dir(|| {
        for i in 0..3 {
            store::append_event(&serde_json::json!({"n": i})).unwrap();
        }
        let body = std::fs::read_to_string(store::events_path()).unwrap();
        assert_eq!(body.lines().count(), 3);
        assert_eq!(body.lines().next().unwrap(), r#"{"n":0}"#);
    });
}

#[test]
fn reconcile_uses_the_injected_pane_list() {
    let now = Utc::now();
    let fresh = now.to_rfc3339();
    let old = (now - Duration::hours(2)).to_rfc3339();

    let records = vec![
        rec("%1", State::Working, &fresh),    // alive, stays working
        rec("%2", State::NeedsInput, &fresh), // alive, sorts first
        rec("%3", State::Working, &fresh),    // dead -> ended
        rec("%4", State::Ended, &old),        // dead and stale -> pruned
    ];
    let out = store::reconcile(records, &["%1".into(), "%2".into()], now);
    let panes: Vec<&str> = out.iter().map(|r| r.pane.as_str()).collect();
    assert_eq!(panes, vec!["%2", "%1", "%3"]);
    assert_eq!(out[2].state, State::Ended);
}

#[test]
fn snapshot_persists_the_reconciliation() {
    with_state_dir(|| {
        let old = (Utc::now() - Duration::hours(2)).to_rfc3339();
        store::save(&rec("%7", State::Working, &store::now_rfc3339())).unwrap();
        store::save(&rec("%8", State::Ended, &old)).unwrap();

        // PERCH_NO_TMUX=1 means the null tmux: no live panes at all.
        let out = store::snapshot(perch::tmux::current().as_ref());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pane, "%7");
        assert_eq!(out[0].state, State::Ended);
        assert_eq!(
            store::load("%7").unwrap().state,
            State::Ended,
            "flip persisted"
        );
        assert!(store::load("%8").is_none(), "stale record pruned from disk");
    });
}

#[test]
fn age_formatting_and_parsing() {
    let now = Utc::now();
    let since = (now - Duration::minutes(5)).to_rfc3339();
    assert_eq!(store::fmt_age(store::age_secs(&since, now)), "5m");
    assert_eq!(store::age_secs("not-a-date", now), 0);
}
