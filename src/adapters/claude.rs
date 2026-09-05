//! Claude Code hook payloads -> perch events.
//!
//! Claude writes one JSON object to the hook's stdin with `hook_event_name`
//! plus event-specific fields. Subagent events carry `agent_id`; they are
//! parsed too, and the reducer folds them into the parent pane's `children`.

use crate::model::{Event, ParsedEvent};

/// Notification types that mean "the human has to do something".
const NEEDS_INPUT: &[&str] = &[
    "permission_prompt",
    "idle_prompt",
    "agent_needs_input",
    "elicitation_dialog",
];

pub fn parse(raw: &serde_json::Value) -> anyhow::Result<Option<ParsedEvent>> {
    let agent_id = str_field(raw, "agent_id");
    let name = raw
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("payload has no hook_event_name"))?;

    let event = match name {
        "SubagentStart" => Event::SubagentStart {
            agent_type: str_field(raw, "agent_type"),
        },
        "SubagentStop" => Event::SubagentStop {
            last_message: str_field(raw, "last_assistant_message"),
        },
        // Without an agent id these are ordinary session events.
        "SessionStart" if agent_id.is_none() => Event::SessionStart,
        "UserPromptSubmit" if agent_id.is_none() => Event::UserPromptSubmit,
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
        "SessionEnd" if agent_id.is_none() => Event::SessionEnd,
        // PreCompact and friends are noise for the dashboard; so is a
        // session-level event attributed to a subagent.
        _ => return Ok(None),
    };

    Ok(Some(ParsedEvent {
        event,
        session_id: str_field(raw, "session_id"),
        cwd: str_field(raw, "cwd"),
        agent_id,
    }))
}

fn str_field(raw: &serde_json::Value, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
