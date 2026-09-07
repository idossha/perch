//! pi extension payloads -> perch events.
//!
//! The payload is written by perch's own extension (`src/adapters/perch.pi.ts`),
//! so its shape is fixed: `{event, session_id, cwd, last_message}` — but every
//! field except `event` may be null, since pi does not always know them. The
//! `ask_user` event adds `tool_use_id` and `tool_input.questions`.

use crate::model::{question_key, Event, ParsedEvent, Question};

pub fn parse(raw: &serde_json::Value) -> anyhow::Result<Option<ParsedEvent>> {
    let name = str_field(raw, "event")
        .or_else(|| str_field(raw, "hook_event_name"))
        .ok_or_else(|| anyhow::anyhow!("payload has no event"))?;

    let event = match name.as_str() {
        "session_start" => Event::SessionStart,
        "agent_start" => Event::UserPromptSubmit,
        "agent_end" => Event::Stop {
            last_message: str_field(raw, "last_message"),
        },
        "session_shutdown" => Event::SessionEnd,
        // The extension's own `ask_user` tool: a question set in the shape
        // Claude's tool uses, sent to `perch hook pi --ask`, which waits for
        // the popup's answer and prints it back for the tool to return.
        "ask_user" => {
            let questions = raw
                .get("tool_input")
                .map(Question::parse_list)
                .unwrap_or_default();
            if questions.is_empty() {
                return Ok(None);
            }
            Event::Question {
                tool_use_id: str_field(raw, "tool_use_id")
                    .unwrap_or_else(|| question_key(&questions)),
                questions,
            }
        }
        // tool_call and anything else is not a state change for the dashboard.
        _ => return Ok(None),
    };

    Ok(Some(ParsedEvent::top_level(
        event,
        str_field(raw, "session_id").or_else(|| str_field(raw, "thread_id")),
        str_field(raw, "cwd"),
    )))
}

fn str_field(raw: &serde_json::Value, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
