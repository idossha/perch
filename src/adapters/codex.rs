//! Codex CLI hook payloads -> perch events.
//!
//! Codex writes one JSON object to the hook's stdin, mirroring Claude's shape
//! (`session_id`, `cwd`, `hook_event_name`, `transcript_path`). Field names have
//! drifted between Codex versions, so the adapter accepts the aliases it has
//! been seen to use rather than insisting on one spelling.

use crate::model::{Event, ParsedEvent};

/// Keys that may carry the event name.
const EVENT_KEYS: &[&str] = &["hook_event_name", "event"];
/// Keys that may carry the session id.
const SESSION_KEYS: &[&str] = &["session_id", "thread_id", "turn_id"];
/// Keys that may carry the final assistant message on `Stop`.
const MESSAGE_KEYS: &[&str] = &["last_assistant_message", "last_message", "message"];

pub fn parse(raw: &serde_json::Value) -> anyhow::Result<Option<ParsedEvent>> {
    // Subagents get their own ids; perch tracks only top-level sessions.
    if raw.get("agent_id").and_then(|v| v.as_str()).is_some() {
        return Ok(None);
    }

    let name = first_str(raw, EVENT_KEYS)
        .ok_or_else(|| anyhow::anyhow!("payload has no hook_event_name"))?;

    let event = match name.as_str() {
        "SessionStart" => Event::SessionStart,
        "UserPromptSubmit" => Event::UserPromptSubmit,
        // A Stop payload need not carry a message; the record keeps the old one.
        "Stop" => Event::Stop {
            last_message: first_str(raw, MESSAGE_KEYS),
        },
        "PermissionRequest" => Event::NeedsInput {
            reason: "permission_request".to_string(),
        },
        "SessionEnd" => Event::SessionEnd,
        // SubagentStop, Pre/PostCompact and Pre/PostToolUse are noise here.
        _ => return Ok(None),
    };

    Ok(Some(ParsedEvent {
        event,
        session_id: first_str(raw, SESSION_KEYS),
        cwd: first_str(raw, &["cwd"]),
    }))
}

/// The first non-empty string among `keys`.
fn first_str(raw: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        raw.get(*k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    })
}
