//! Codex CLI hook payloads -> perch events.
//!
//! Codex writes one JSON object to the hook's stdin, mirroring Claude's shape
//! (`session_id`, `cwd`, `hook_event_name`, `transcript_path`). Field names have
//! drifted between Codex versions, so the adapter accepts the aliases it has
//! been seen to use rather than insisting on one spelling.

use crate::model::{question_key, Event, ParsedEvent, Question};

/// Keys that may carry the event name.
const EVENT_KEYS: &[&str] = &["hook_event_name", "event"];
/// Keys that may carry the session id.
const SESSION_KEYS: &[&str] = &["session_id", "thread_id", "turn_id"];
/// Keys that may carry the final assistant message on `Stop`.
const MESSAGE_KEYS: &[&str] = &["last_assistant_message", "last_message", "message"];

pub fn parse(raw: &serde_json::Value) -> anyhow::Result<Option<ParsedEvent>> {
    // An agent id attributes the event to a subagent of this pane's session.
    let agent_id = first_str(raw, &["agent_id"]);
    let name = first_str(raw, EVENT_KEYS)
        .ok_or_else(|| anyhow::anyhow!("payload has no hook_event_name"))?;

    let event = match name.as_str() {
        "SubagentStart" => Event::SubagentStart {
            agent_type: first_str(raw, &["agent_type"]),
        },
        "SubagentStop" => Event::SubagentStop {
            last_message: first_str(raw, MESSAGE_KEYS),
            agent_type: first_str(raw, &["agent_type"]),
        },
        "SessionStart" if agent_id.is_none() => Event::SessionStart,
        "UserPromptSubmit" if agent_id.is_none() => Event::UserPromptSubmit,
        // A Stop payload need not carry a message; the record keeps the old one.
        "Stop" => Event::Stop {
            last_message: first_str(raw, MESSAGE_KEYS),
        },
        "PermissionRequest" => Event::NeedsInput {
            reason: "permission_request".to_string(),
        },
        // perch's own `ask_user` MCP tool (`perch mcp`), sent to
        // `perch hook codex --ask`: the same question set Claude's tool uses.
        "ask_user" => {
            let questions = raw
                .get("tool_input")
                .map(Question::parse_list)
                .unwrap_or_default();
            if questions.is_empty() {
                return Ok(None);
            }
            Event::Question {
                tool_use_id: first_str(raw, &["tool_use_id"])
                    .unwrap_or_else(|| question_key(&questions)),
                questions,
            }
        }
        "SessionEnd" if agent_id.is_none() => Event::SessionEnd,
        // Codex's question tool: the pane is blocked on the human from here.
        // Codex hooks cannot return an answer, so this is detection only —
        // the chime, the card and the board flag, then `Enter` to the pane.
        "PreToolUse"
            if agent_id.is_none()
                && first_str(raw, &["tool_name"]).as_deref() == Some("request_user_input") =>
        {
            Event::NeedsInput {
                reason: "request_user_input".to_string(),
            }
        }
        // Proof the agent is running again: clears a stale `needs_input`.
        "PreToolUse" if agent_id.is_none() => Event::ToolUse,
        // Pre/PostCompact and Pre/PostToolUse are noise here.
        _ => return Ok(None),
    };

    Ok(Some(ParsedEvent {
        event,
        session_id: first_str(raw, SESSION_KEYS),
        cwd: first_str(raw, &["cwd"]),
        agent_id,
        // Codex puts the active model slug on every payload.
        model: first_str(raw, &["model"]),
        effort: first_str(raw, &["reasoning_effort", "effort"]),
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
