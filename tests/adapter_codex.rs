//! Codex adapter: every fixture payload maps to the event the plan specifies.

use perch::adapters;
use perch::model::{Event, Harness, ParsedEvent};

fn fixture(name: &str) -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/codex/").to_string() + name;
    let body = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&body).unwrap()
}

fn parse(name: &str) -> Option<ParsedEvent> {
    adapters::parse(Harness::Codex, &fixture(name)).unwrap()
}

#[test]
fn session_start_carries_the_identity_fields() {
    let p = parse("session_start.json").unwrap();
    assert_eq!(p.event, Event::SessionStart);
    assert_eq!(p.session_id.as_deref(), Some("c-1"));
    assert_eq!(p.cwd.as_deref(), Some("/Users/x/00_development/perch"));
}

#[test]
fn user_prompt_submit_is_working() {
    assert_eq!(
        parse("user_prompt_submit.json").unwrap().event,
        Event::UserPromptSubmit
    );
}

#[test]
fn stop_carries_the_last_message() {
    assert_eq!(
        parse("stop.json").unwrap().event,
        Event::Stop {
            last_message: Some("Added the codex adapter.".into())
        }
    );
}

#[test]
fn stop_without_a_message_is_still_done() {
    assert_eq!(
        parse("stop_without_message.json").unwrap().event,
        Event::Stop { last_message: None }
    );
}

#[test]
fn permission_request_is_needs_input() {
    assert_eq!(
        parse("permission_request.json").unwrap().event,
        Event::NeedsInput {
            reason: "permission_request".into()
        }
    );
}

#[test]
fn session_end() {
    assert_eq!(parse("session_end.json").unwrap().event, Event::SessionEnd);
}

#[test]
fn alias_field_names_are_accepted() {
    let p = parse("alias_fields.json").unwrap();
    assert_eq!(p.event, Event::UserPromptSubmit);
    assert_eq!(p.session_id.as_deref(), Some("c-2"));
}

#[test]
fn untracked_events_are_dropped() {
    assert!(parse("post_compact.json").is_none());
}

#[test]
fn pre_tool_use_is_proof_the_agent_is_running() {
    assert_eq!(parse("pre_tool_use.json").unwrap().event, Event::ToolUse);
}

#[test]
fn subagent_events_carry_the_agent_id() {
    let p = parse("subagent_start.json").unwrap();
    assert_eq!(
        p.event,
        Event::SubagentStart {
            agent_type: Some("reviewer".into())
        }
    );
    assert_eq!(p.agent_id.as_deref(), Some("a-7"));

    let p = parse("subagent_stop.json").unwrap();
    assert_eq!(
        p.event,
        Event::SubagentStop {
            last_message: Some("Reviewed the diff.".into()),
            agent_type: None
        }
    );
    assert_eq!(p.agent_id.as_deref(), Some("a-7"));
}

#[test]
fn payload_without_an_event_name_is_an_error() {
    assert!(adapters::parse(Harness::Codex, &serde_json::json!({})).is_err());
}

/// Codex's question tool is `request_user_input`. Codex hooks have no return
/// path for an answer, so perch cannot answer it — but the pane is blocked on
/// the human the moment the tool is called, and that is a `needs_input`.
#[test]
fn a_request_user_input_pre_tool_use_is_needs_input() {
    let p = parse("pre_tool_use_request_user_input.json").unwrap();
    assert_eq!(
        p.event,
        Event::NeedsInput {
            reason: "request_user_input".into()
        }
    );
    // Any other tool is still just proof of running.
    assert_eq!(parse("pre_tool_use.json").unwrap().event, Event::ToolUse);
}

/// Codex cannot have its own question dialog answered by a hook, so perch
/// gives it an `ask_user` MCP tool; the tool's payload to
/// `perch hook codex --ask` is the same question set Claude sends.
#[test]
fn an_ask_user_payload_is_a_question() {
    let p = parse("ask_user.json").unwrap();
    let Event::Question {
        tool_use_id,
        questions,
    } = p.event
    else {
        panic!("not a question: {:?}", p.event);
    };
    assert_eq!(tool_use_id, "mcp-7");
    assert_eq!(questions.len(), 2);
    assert_eq!(questions[1].header, "Harnesses");
    assert!(questions[1].multi_select);
}

/// Codex puts the active model slug on every hook payload.
#[test]
fn the_model_slug_is_read_from_any_payload() {
    let p = parse("session_start_with_model.json").unwrap();
    assert_eq!(p.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(parse("session_start.json").unwrap().model, None);
}
