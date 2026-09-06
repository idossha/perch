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

/// `idle_prompt` is Claude nudging you after 60 s of quiet, not a question:
/// it is logged and changes nothing. Same for auth and quota notices.
#[test]
fn idle_and_informational_notifications_are_only_observed() {
    for (file, label) in [
        ("notification_idle_prompt.json", "idle_prompt"),
        ("notification_auth_success.json", "auth_success"),
    ] {
        assert_eq!(
            parse(file).unwrap().event,
            Event::Observed {
                label: label.into()
            },
            "{file}"
        );
    }
}

/// An answered dialog, and any tool call, prove the agent is running again.
#[test]
fn a_finished_dialog_or_a_tool_call_is_tool_use() {
    assert_eq!(
        parse("notification_elicitation_complete.json")
            .unwrap()
            .event,
        Event::ToolUse
    );
    assert_eq!(parse("pre_tool_use.json").unwrap().event, Event::ToolUse);
}

/// `SendMessage` resumes an existing background subagent; the id lives in
/// `tool_input.to` and nowhere else, and every other tool stays a tool call.
#[test]
fn a_send_message_pre_tool_use_is_a_resume() {
    let p = parse("pre_tool_use_send_message.json").unwrap();
    assert_eq!(
        p.event,
        Event::SubagentResume {
            id: "a41eb56e05dc8146f".into()
        }
    );
    assert_eq!(p.agent_id, None, "the resume names the child, not the pane");
    assert_eq!(p.session_id.as_deref(), Some("s-1"));

    // A ` [ref]` suffix is dropped; an empty or missing target is not a resume.
    let mut raw = fixture("pre_tool_use_send_message.json");
    raw["tool_input"]["to"] = serde_json::json!("a41eb56e05dc8146f [ref]");
    assert_eq!(
        adapters::parse(Harness::Claude, &raw)
            .unwrap()
            .unwrap()
            .event,
        Event::SubagentResume {
            id: "a41eb56e05dc8146f".into()
        }
    );
    // A teammate or a session is named, not identified: `to: main` is kept
    // verbatim and becomes a child of that name (decision 17).
    raw["tool_input"]["to"] = serde_json::json!("main");
    assert_eq!(
        adapters::parse(Harness::Claude, &raw)
            .unwrap()
            .unwrap()
            .event,
        Event::SubagentResume { id: "main".into() }
    );
    raw["tool_input"]["to"] = serde_json::json!("");
    assert_eq!(
        adapters::parse(Harness::Claude, &raw)
            .unwrap()
            .unwrap()
            .event,
        Event::ToolUse
    );
}

/// A helper agent's stop carries an empty `agent_type`; the reducer needs to
/// see that, so the adapter keeps the distinction.
#[test]
fn a_helper_stop_has_no_agent_type() {
    let p = parse("subagent_stop_helper.json").unwrap();
    assert_eq!(
        p.event,
        Event::SubagentStop {
            last_message: None,
            agent_type: None
        }
    );
    assert_eq!(p.agent_id.as_deref(), Some("helper-1"));
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
            last_message: Some("Found it in src/reducer.rs.".into()),
            agent_type: Some("Explore".into())
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
