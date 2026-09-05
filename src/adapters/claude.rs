//! Claude Code hook payloads -> perch events.
//!
//! Claude writes one JSON object to the hook's stdin with `hook_event_name`
//! plus event-specific fields. Subagent events carry `agent_id` and are ignored.

use crate::model::{Event, ParsedEvent};

/// Notification types that mean "the human has to do something".
const NEEDS_INPUT: &[&str] = &[
    "permission_prompt",
    "idle_prompt",
    "agent_needs_input",
    "elicitation_dialog",
];

pub fn parse(raw: &serde_json::Value) -> anyhow::Result<Option<ParsedEvent>> {
    // Subagents get their own ids; perch tracks only top-level sessions.
    if raw.get("agent_id").and_then(|v| v.as_str()).is_some() {
        return Ok(None);
    }

    let name = raw
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("payload has no hook_event_name"))?;

    let event = match name {
        "SessionStart" => Event::SessionStart,
        "UserPromptSubmit" => Event::UserPromptSubmit,
        "Stop" => Event::Stop {
            last_message: raw
                .get("last_assistant_message")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        },
        "Notification" => {
            let kind = raw
                .get("notification_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if kind == "agent_completed" {
                Event::Completed
            } else if NEEDS_INPUT.contains(&kind) || kind.starts_with("elicitation") {
                Event::NeedsInput {
                    reason: kind.to_string(),
                }
            } else {
                // An unrecognised notification is not a state change.
                return Ok(None);
            }
        }
        "SessionEnd" => Event::SessionEnd,
        // PreCompact and friends are noise for the dashboard.
        _ => return Ok(None),
    };

    Ok(Some(ParsedEvent {
        event,
        session_id: str_field(raw, "session_id"),
        cwd: str_field(raw, "cwd"),
    }))
}

fn str_field(raw: &serde_json::Value, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
