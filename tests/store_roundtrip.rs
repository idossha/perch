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

/// Seen is a property of the live client list, evaluated on every read.
///
/// A `done` pane a focused client is showing becomes `idle` inside
/// `snapshot`, with no hook involved; a `done` pane nobody is showing stays
/// `done`; and a pane merely on an *unfocused* client's screen stays `done`
/// as long as the server knows about focus at all.
#[test]
fn snapshot_marks_done_panes_a_focused_client_is_showing_as_idle() {
    with_state_dir(|| {
        let ts = store::now_rfc3339();
        let live = |ids: &[&str]| perch::tmux::NullTmux {
            panes: ids
                .iter()
                .map(|p| {
                    perch::tmux::parse_pane_line(&format!("{p} one 0 0 1 1 claude 1 shell"))
                        .unwrap()
                })
                .collect(),
        };

        for p in ["%1", "%2", "%3"] {
            store::save(&rec(p, State::Done, &ts)).unwrap();
        }
        // %1 is on a focused client, %2 on an unfocused one, %3 on none.
        std::env::set_var("PERCH_FAKE_VIEWERS", "%1:focused,%2");
        let out = store::snapshot(&live(&["%1", "%2", "%3"]));
        let state = |p: &str| out.iter().find(|r| r.pane == p).unwrap().state;
        assert_eq!(state("%1"), State::Idle, "a focused client is showing it");
        assert_eq!(state("%2"), State::Done, "on screen, but not looked at");
        assert_eq!(state("%3"), State::Done, "nobody is showing it");
        // Written through, not just reported.
        assert_eq!(store::load("%1").unwrap().state, State::Idle);
        assert_eq!(store::load("%2").unwrap().state, State::Done);

        // No client anywhere reports focus: the flag carries no information,
        // so any viewer counts and %2 is seen after all.
        std::env::set_var("PERCH_FAKE_VIEWERS", "%2");
        let out = store::snapshot(&live(&["%1", "%2", "%3"]));
        assert_eq!(
            out.iter().find(|r| r.pane == "%2").unwrap().state,
            State::Idle,
            "no focus info anywhere: a viewer is enough"
        );
        std::env::remove_var("PERCH_FAKE_VIEWERS");
    });
}

/// A pane that is not working may perfectly well have running subagents:
/// Claude runs them in the background and the parent's turn ends without them.
/// Read time retires only the stuck and the dead.
#[test]
fn reconcile_keeps_running_children_of_an_idle_or_done_pane() {
    use perch::model::{Harness, PaneRecord, State, Subagent};
    let now = chrono::Utc::now();
    let child = |id: &str, state: State, age_secs: i64| Subagent {
        id: id.into(),
        agent_type: None,
        state,
        since: (now - Duration::seconds(age_secs)).to_rfc3339(),
        last_message: None,
    };
    let pane = |p: &str, state: State, children: Vec<Subagent>| {
        let mut r = PaneRecord::new(p, Harness::Claude, &now.to_rfc3339());
        r.state = state;
        r.children = children;
        r
    };
    let records = vec![
        pane(
            "%1",
            State::Idle,
            vec![child("a", State::Working, 5), child("b", State::Done, 5)],
        ),
        pane("%2", State::Done, vec![child("c", State::Working, 5)]),
        pane("%3", State::Working, vec![child("d", State::Working, 5)]),
        pane(
            "%4",
            State::Done,
            vec![child("stuck", State::Working, 3 * 3600)],
        ),
        pane("%5", State::Ended, vec![child("orphan", State::Working, 5)]),
    ];
    let live: Vec<String> = ["%1", "%2", "%3", "%4", "%5"]
        .iter()
        .map(|p| p.to_string())
        .collect();
    let out = perch::store::reconcile(records, &live, now);
    let by = |pane: &str| out.iter().find(|r| r.pane == pane).expect(pane);
    // An idle pane drops its finished child and keeps the running one, which
    // keeps the pane delegating.
    let ids: Vec<&str> = by("%1").children.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["a"], "{:?}", by("%1").children);
    assert_eq!(by("%1").effective_state(), State::Working);
    // A done pane keeps its running child, and reads as working.
    assert_eq!(by("%2").children[0].state, State::Working);
    assert_eq!(by("%2").effective_state(), State::Working);
    // A working pane's running child is untouched.
    assert_eq!(by("%3").children[0].state, State::Working);
    // A child running for three hours is a SubagentStop that never came.
    assert_eq!(by("%4").children[0].state, State::Done);
    assert_eq!(by("%4").effective_state(), State::Done);
    // A pane that is gone runs nothing.
    assert_eq!(by("%5").children[0].state, State::Done);
}
