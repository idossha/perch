//! pi adapter: the payloads perch's own extension writes.

use perch::adapters;
use perch::model::{Event, Harness, ParsedEvent};

fn fixture(name: &str) -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pi/").to_string() + name;
    let body = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&body).unwrap()
}

fn parse(name: &str) -> Option<ParsedEvent> {
    adapters::parse(Harness::Pi, &fixture(name)).unwrap()
}

#[test]
fn session_start_carries_the_identity_fields() {
    let p = parse("session_start.json").unwrap();
    assert_eq!(p.event, Event::SessionStart);
    assert_eq!(p.session_id.as_deref(), Some("p-1"));
    assert_eq!(p.cwd.as_deref(), Some("/Users/x/00_development/perch"));
}

#[test]
fn agent_start_is_working() {
    assert_eq!(
        parse("agent_start.json").unwrap().event,
        Event::UserPromptSubmit
    );
}

#[test]
fn agent_end_is_done_with_the_last_message() {
    assert_eq!(
        parse("agent_end.json").unwrap().event,
        Event::Stop {
            last_message: Some("Wrote the pi extension.".into())
        }
    );
}

#[test]
fn a_null_session_id_and_message_are_tolerated() {
    let p = parse("agent_end_no_message.json").unwrap();
    assert_eq!(p.event, Event::Stop { last_message: None });
    assert_eq!(p.session_id, None);
}

#[test]
fn session_shutdown_ends_the_session() {
    assert_eq!(
        parse("session_shutdown.json").unwrap().event,
        Event::SessionEnd
    );
}

#[test]
fn tool_call_is_not_a_state_change() {
    assert!(parse("tool_call.json").is_none());
}

#[test]
fn payload_without_an_event_is_an_error() {
    assert!(adapters::parse(Harness::Pi, &serde_json::json!({})).is_err());
}

#[test]
fn the_embedded_extension_spawns_the_pi_hook() {
    let src = perch::adapters::PI_EXTENSION;
    assert!(src.contains(r#"spawn("perch", ["hook", "pi"]"#), "{src}");
    for event in [
        "session_start",
        "agent_start",
        "agent_end",
        "session_shutdown",
    ] {
        assert!(
            src.contains(&format!("pi.on(\"{event}\"")),
            "missing {event}"
        );
    }
}

/// pi has no question tool of its own, so perch's extension registers one:
/// `ask_user`. Its payload to `perch hook pi --ask` is the same question set
/// Claude sends, under pi's field names.
#[test]
fn an_ask_user_payload_is_a_question() {
    let p = parse("ask_user.json").unwrap();
    let perch::model::Event::Question {
        tool_use_id,
        questions,
    } = p.event
    else {
        panic!("not a question: {:?}", p.event);
    };
    assert_eq!(tool_use_id, "pi-call-1");
    assert_eq!(questions.len(), 2);
    assert_eq!(questions[0].header, "Backend");
    assert_eq!(questions[0].options[1].label, "SQLite");
    assert!(questions[1].multi_select);
    assert_eq!(p.session_id.as_deref(), Some("p-1"));
}

/// The extension gives pi's model an `ask_user` tool that asks through perch
/// and waits, and falls back to pi's own select dialog when perch hands the
/// question back or is not there.
#[test]
fn the_embedded_extension_registers_an_ask_user_tool_that_asks_through_perch() {
    let src = perch::adapters::PI_EXTENSION;
    assert!(src.contains("registerTool("), "{src}");
    assert!(src.contains("name: \"ask_user\""), "{src}");
    assert!(
        src.contains("[\"hook\", \"pi\", \"--ask\"]"),
        "the tool waits on the ask hook"
    );
    assert!(src.contains("ctx.ui.select("), "native fallback");
    assert!(src.contains("ctx.ui.input("), "free text in the fallback");
    assert!(
        src.contains("multiSelect"),
        "multi-select is carried through"
    );
    assert!(
        src.contains("from \"typebox\""),
        "parameters are a TypeBox schema"
    );
}

/// Regression: pi's model asked one question per call, so the popup showed
/// no tabs. The tool description tells it to batch, and the fallback numbers
/// the questions so a set still reads as one.
#[test]
fn the_pi_tool_asks_for_one_call_per_set_and_numbers_the_fallback() {
    let src = perch::adapters::PI_EXTENSION;
    assert!(src.contains("into ONE call"), "{src}");
    assert!(
        src.contains("Question ${i + 1}/${questions.length}"),
        "{src}"
    );
}

/// The extension reports pi's model id and thinking level when it knows them.
#[test]
fn model_and_effort_come_from_the_extension_payload() {
    let p = parse("session_start_with_model.json").unwrap();
    assert_eq!(p.model.as_deref(), Some("anthropic/claude-sonnet-5"));
    assert_eq!(p.effort.as_deref(), Some("medium"));
    assert_eq!(parse("session_start.json").unwrap().model, None);
    let src = perch::adapters::PI_EXTENSION;
    assert!(
        src.contains("model:") && src.contains("ctx?.model?.id"),
        "{src}"
    );
}
