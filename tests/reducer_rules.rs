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

fn question() -> ParsedEvent {
    ev(Event::Question {
        tool_use_id: "toolu_1".into(),
        questions: vec![perch::model::Question {
            question: "Which backend?".into(),
            header: "Backend".into(),
            kind: "choice".into(),
            options: vec![perch::model::QuestionOption {
                label: "Files".into(),
                description: String::new(),
            }],
            multi_select: false,
        }],
    })
}

/// A question is a `needs_input` like any other — same state, same chime —
/// and the record carries what was asked, with a deadline, so the dashboard
/// can put a form under it while the hook is still waiting.
#[test]
fn a_question_is_needs_input_that_remembers_what_was_asked() {
    let mut r = rec();
    let now = chrono::Utc::now().to_rfc3339();
    let out = apply_with(&mut r, &question(), &now, false);
    assert_eq!(r.state, State::NeedsInput);
    assert_eq!(out.sound, Some("needs_input"));
    assert!(out.parent_changed);
    assert_eq!(r.last_message.as_deref(), Some("Which backend?"));
    let q = r.question.as_ref().expect("the question is on the record");
    assert_eq!(q.tool_use_id, "toolu_1");
    assert_eq!(q.questions[0].header, "Backend");
    assert!(q.is_live(chrono::Utc::now()), "fresh, so still answerable");
    let far = chrono::Utc::now() + chrono::Duration::seconds(perch::reducer::ASK_WAIT_SECS + 5);
    assert!(!q.is_live(far), "past the hook's own deadline it is stale");
}

/// Whatever moves the pane off `needs_input` retires the question: a tool
/// call after a native answer, a new prompt, the session ending.
#[test]
fn leaving_needs_input_by_any_path_drops_the_question() {
    for leave in [Event::ToolUse, Event::UserPromptSubmit, Event::SessionEnd] {
        let mut r = rec();
        apply_with(&mut r, &question(), "t1", false);
        assert!(r.question.is_some());
        apply_with(&mut r, &ev(leave.clone()), "t2", false);
        assert_ne!(r.state, State::NeedsInput, "{leave:?}");
        assert!(r.question.is_none(), "{leave:?}");
    }
}

/// The hook's own answer path: the agent is unblocked by perch, so the pane
/// is working again this instant, not on the next tool call.
#[test]
fn an_answer_puts_the_pane_back_to_work_and_is_silent() {
    let mut r = rec();
    apply_with(&mut r, &question(), "t1", false);
    perch::reducer::answered(&mut r, "t2");
    assert_eq!(r.state, State::Working);
    assert_eq!(r.since, "t2");
    assert!(r.question.is_none());
}
