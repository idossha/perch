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
