//! Adapter tests: every fixture payload maps to the event the plan specifies.

use perch::adapters;
use perch::model::{Event, Harness};

fn fixture(name: &str) -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/claude/").to_string() + name;
    let body = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&body).unwrap()
}

fn parse(name: &str) -> Option<perch::model::ParsedEvent> {
    adapters::parse(Harness::Claude, &fixture(name)).unwrap()
}

#[test]
fn session_start() {
    let p = parse("session_start.json").unwrap();
    assert_eq!(p.event, Event::SessionStart);
    assert_eq!(p.session_id.as_deref(), Some("s-1"));
    assert_eq!(p.cwd.as_deref(), Some("/Users/x/00_development/perch"));
}

#[test]
fn user_prompt_submit() {
    assert_eq!(
        parse("user_prompt_submit.json").unwrap().event,
        Event::UserPromptSubmit
    );
}

#[test]
fn stop_carries_the_last_message() {
    let p = parse("stop.json").unwrap();
    assert_eq!(
        p.event,
        Event::Stop {
            last_message: Some("Done: added the reducer and its tests.".into())
        }
    );
}

#[test]
fn needs_input_notifications() {
    for (file, reason) in [
        ("notification_permission_prompt.json", "permission_prompt"),
        ("notification_idle_prompt.json", "idle_prompt"),
        ("notification_agent_needs_input.json", "agent_needs_input"),
        ("notification_elicitation_dialog.json", "elicitation_dialog"),
    ] {
        assert_eq!(
            parse(file).unwrap().event,
            Event::NeedsInput {
                reason: reason.into()
            },
            "{file}"
        );
    }
}

#[test]
fn agent_completed_is_done() {
    assert_eq!(
        parse("notification_agent_completed.json").unwrap().event,
        Event::Completed
    );
}

#[test]
fn session_end() {
    assert_eq!(parse("session_end.json").unwrap().event, Event::SessionEnd);
}

#[test]
fn untracked_events_are_dropped() {
    assert!(parse("precompact.json").is_none());
}

#[test]
fn subagent_events_carry_the_agent_id() {
    let p = parse("subagent_start.json").unwrap();
    assert_eq!(
        p.event,
        Event::SubagentStart {
            agent_type: Some("Explore".into())
        }
    );
    assert_eq!(p.agent_id.as_deref(), Some("sub-7"));
    assert_eq!(p.session_id.as_deref(), Some("s-1"));

    let p = parse("subagent_stop_event.json").unwrap();
    assert_eq!(
        p.event,
        Event::SubagentStop {
            last_message: Some("Found it in src/reducer.rs.".into())
        }
    );
    assert_eq!(p.agent_id.as_deref(), Some("sub-7"));
}

/// Claude also sends a plain `Stop` or `Notification` with an `agent_id` when
/// the subagent, not the session, produced it.
#[test]
fn a_stop_or_notification_with_an_agent_id_belongs_to_the_child() {
    let p = parse("subagent_stop.json").unwrap();
    assert_eq!(
        p.event,
        Event::Stop {
            last_message: Some("subagent finished".into())
        }
    );
    assert_eq!(p.agent_id.as_deref(), Some("sub-7"));

    let p = parse("notification_agent_needs_input_child.json").unwrap();
    assert_eq!(
        p.event,
        Event::NeedsInput {
            reason: "agent_needs_input".into()
        }
    );
    assert_eq!(p.agent_id.as_deref(), Some("sub-7"));
}

#[test]
fn payload_without_an_event_name_is_an_error() {
    assert!(adapters::parse(Harness::Claude, &serde_json::json!({})).is_err());
}

#[test]
fn each_harness_has_a_real_adapter() {
    // A Claude-shaped Stop is understood by codex too; pi uses its own key.
    for (h, raw) in [
        (
            Harness::Codex,
            serde_json::json!({"hook_event_name": "Stop"}),
        ),
        (Harness::Pi, serde_json::json!({"event": "agent_end"})),
    ] {
        let p = adapters::parse(h, &raw).unwrap().unwrap();
        assert_eq!(p.event, Event::Stop { last_message: None });
    }
}
