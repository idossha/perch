//! Rules of the reducer that nothing else pins: which transition asks for
//! which sound, and which asks for none at all.
//!
//! The state table itself is unit-tested in `src/reducer.rs`; this is the cue
//! half of `Applied`, which is what the user actually hears.

use perch::model::{Event, Harness, PaneRecord, ParsedEvent, State};
use perch::reducer::apply_with;

fn rec() -> PaneRecord {
    PaneRecord::new("%1", Harness::Claude, "t0")
}

fn ev(event: Event) -> ParsedEvent {
    ParsedEvent::top_level(event, None, None)
}

fn stop() -> ParsedEvent {
    ev(Event::Stop {
        last_message: Some("finished".into()),
    })
}

#[test]
fn a_stop_you_did_not_watch_chimes_done_and_one_you_watched_is_silent() {
    let mut r = rec();
    let out = apply_with(&mut r, &stop(), "t1", false);
    assert_eq!(r.state, State::Done);
    assert_eq!(out.sound, Some("done"));

    let mut r = rec();
    let out = apply_with(&mut r, &stop(), "t1", true);
    assert_eq!(r.state, State::Idle);
    assert_eq!(
        out.sound, None,
        "a turn that ended under your eyes is not news"
    );
}

#[test]
fn a_needs_input_chimes_its_own_sound() {
    let mut r = rec();
    let out = apply_with(
        &mut r,
        &ev(Event::NeedsInput {
            reason: "permission_prompt".into(),
        }),
        "t1",
        false,
    );
    assert_eq!(r.state, State::NeedsInput);
    assert_eq!(out.sound, Some("needs_input"));
}

#[test]
fn a_completed_notification_chimes_done() {
    let mut r = rec();
    let out = apply_with(&mut r, &ev(Event::Completed), "t1", false);
    assert_eq!(r.state, State::Done);
    assert_eq!(out.sound, Some("done"));
}

/// Everything that is not `done` or `needs_input` is silent, whether or not
/// it moved the pane.
#[test]
fn no_other_transition_makes_a_sound() {
    for (event, want) in [
        (Event::SessionStart, State::Idle),
        (Event::UserPromptSubmit, State::Working),
        (Event::SessionEnd, State::Ended),
    ] {
        let mut r = rec();
        let out = apply_with(&mut r, &ev(event), "t1", false);
        assert_eq!(r.state, want);
        assert!(out.parent_changed);
        assert_eq!(out.sound, None, "{want:?} must be silent");
    }

    // An observed notification moves nothing and says nothing.
    let mut r = rec();
    let out = apply_with(
        &mut r,
        &ev(Event::Observed {
            label: "idle_prompt".into(),
        }),
        "t1",
        false,
    );
    assert!(!out.parent_changed);
    assert_eq!(out.sound, None);
    assert_eq!(r.state, State::Starting);
}

/// A repeat of the state a pane is already in is not a transition, so it does
/// not chime again — the second `done` of a pane that never left `done`.
#[test]
fn an_unchanged_state_chimes_nothing() {
    let mut r = rec();
    assert_eq!(apply_with(&mut r, &stop(), "t1", false).sound, Some("done"));
    let out = apply_with(&mut r, &stop(), "t2", false);
    assert!(!out.parent_changed);
    assert_eq!(out.sound, None);
    assert_eq!(r.since, "t1", "since moves only on a real change");
}
