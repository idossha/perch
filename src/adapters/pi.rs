//! pi extension payloads -> perch events.
//!
//! The payload is written by perch's own extension (`src/adapters/perch.pi.ts`),
//! so its shape is fixed: `{event, session_id, cwd, last_message}` — but every
//! field except `event` may be null, since pi does not always know them.

use crate::model::{Event, ParsedEvent};

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
        // tool_call and anything else is not a state change for the dashboard.
        _ => return Ok(None),
    };

    Ok(Some(ParsedEvent {
        event,
        session_id: str_field(raw, "session_id").or_else(|| str_field(raw, "thread_id")),
        cwd: str_field(raw, "cwd"),
    }))
}

fn str_field(raw: &serde_json::Value, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
