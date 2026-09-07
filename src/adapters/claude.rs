//! Claude Code hook payloads -> perch events.
//!
//! Claude writes one JSON object to the hook's stdin with `hook_event_name`
//! plus event-specific fields. Subagent events carry `agent_id`; they are
//! parsed too, and the reducer folds them into the parent pane's `children`.

use crate::model::{question_key, Event, ParsedEvent, Question};

/// Notification types that mean the human is blocking the agent: an approval
/// or a question with a dialog on screen. `idle_prompt` is deliberately absent
/// — it is Claude's "you have been idle" nudge, not a request.
const NEEDS_INPUT: &[&str] = &[
    "permission_prompt",
    "agent_needs_input",
    "elicitation_dialog",
    "elicitation_url_dialog",
];

/// Notification types that end a dialog: whatever was asked has been answered.
const NEEDS_INPUT_CLEARED: &[&str] = &["elicitation_complete", "elicitation_response"];

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
            agent_type: str_field(raw, "agent_type"),
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
            } else if NEEDS_INPUT.contains(&kind) {
                Event::NeedsInput {
                    reason: kind.to_string(),
                }
            } else if NEEDS_INPUT_CLEARED.contains(&kind) {
                Event::ToolUse
            } else if kind.is_empty() {
                // An unrecognised notification is not a state change.
                return Ok(None);
            } else {
                // idle_prompt, auth_success, quota_* and friends: logged, not
                // acted on.
                Event::Observed {
                    label: kind.to_string(),
                }
            }
        }
        "SessionEnd" if agent_id.is_none() => Event::SessionEnd,
        // The dialog is about to be shown. For `AskUserQuestion` this is the
        // event auto mode fires (it skips `PreToolUse` for that tool), and it
        // accepts the same `updatedInput` answer. Anything else here is not
        // subscribed to; parse it to nothing rather than guess.
        "PermissionRequest" if agent_id.is_none() => match ask_user_question(raw) {
            Some(q) => q,
            None => return Ok(None),
        },
        // `SendMessage` resumes an existing background subagent, and Claude
        // sends no `SubagentStart` for that. Its target is the only place the
        // agent id appears, so the pre-tool payload is where a resume is seen.
        "PreToolUse" if agent_id.is_none() => match send_message_target(raw) {
            Some(id) => Event::SubagentResume { id },
            None => match ask_user_question(raw) {
                Some(q) => q,
                None => Event::ToolUse,
            },
        },
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

/// The agent id a `PreToolUse` for `SendMessage` is addressed to.
///
/// `tool_input.to` is an agent id for a subagent and a name for a teammate or
/// a session; the id is kept exactly as given, minus the ` [ref]` suffix the
/// tool accepts.
fn send_message_target(raw: &serde_json::Value) -> Option<String> {
    if raw.get("tool_name").and_then(|v| v.as_str())? != "SendMessage" {
        return None;
    }
    let to = raw.get("tool_input")?.get("to")?.as_str()?;
    let to = match to.split_once(" [") {
        Some((head, _)) => head,
        None => to,
    };
    let to = to.trim();
    (!to.is_empty()).then(|| to.to_string())
}

/// An `AskUserQuestion` call, read in full: the one tool whose input perch
/// needs the text of, because the `--ask` hook can answer it.
fn ask_user_question(raw: &serde_json::Value) -> Option<Event> {
    if raw.get("tool_name").and_then(|v| v.as_str())? != "AskUserQuestion" {
        return None;
    }
    let questions = Question::parse_list(raw.get("tool_input")?);
    if questions.is_empty() {
        return None;
    }
    // `PermissionRequest` carries no `tool_use_id`: derive one from the
    // questions themselves, so the two events agree on what was asked.
    let tool_use_id = str_field(raw, "tool_use_id").unwrap_or_else(|| question_key(&questions));
    Some(Event::Question {
        tool_use_id,
        questions,
    })
}

fn str_field(raw: &serde_json::Value, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
