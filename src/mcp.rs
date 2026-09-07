//! `perch mcp`: a stdio MCP server with one tool, `ask_user`, for harnesses
//! that let a model call MCP tools but give hooks no way to answer their own
//! question dialog — Codex, whose `request_user_input` cannot be pre-answered
//! and exists only in Plan mode.
//!
//! The tool's questions go to `perch hook codex --ask` exactly as Claude's and
//! pi's do: the popup appears on every attached client, the answer file comes
//! back, and the answers are returned to the model as the tool result. If the
//! question is handed back (the popup closed, the deadline passed), the tool
//! says so and the model asks in chat.
//!
//! Protocol: JSON-RPC 2.0, one message per line on stdin/stdout, the MCP
//! `initialize` / `tools/list` / `tools/call` trio. `TMUX_PANE` reaches the
//! hook because `perch install codex` whitelists it in the server's `env_vars`.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

pub const TOOL_NAME: &str = "ask_user";

/// The tool as the model sees it.
pub fn tool_definition() -> Value {
    json!({
        "name": TOOL_NAME,
        // Asking the user changes nothing on disk or on the network. Codex
        // reads these to decide whether a call needs approval; without them an
        // `approval_policy = "never"` session refuses the call outright.
        "annotations": {
            "title": "Ask the user",
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false
        },
        "description": "Ask the user one to four multiple-choice questions and wait for their answers. Use it whenever you need a decision, a clarification, a preference or an approval you cannot resolve from the code or the conversation — never ask such things in plain chat. Put every question you have right now into ONE call. The questions appear in front of the user wherever they are (perch pops them up on every tmux client) and the answers come back as a map from question text to the chosen label; multi-select answers are comma-separated; a question the user left unanswered is absent — take your recommended option and say so.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "description": "One self-contained sentence. It is also the key the answer comes back under, so keep it unique within the call." },
                            "header": { "type": "string", "description": "Two or three words, at most twelve characters: the tab label." },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string", "description": "One line on what choosing it leads to." }
                                    },
                                    "required": ["label"]
                                },
                                "description": "Two to four options, the recommended one first, marked (Recommended). No Other or Skip: the form provides free text."
                            },
                            "multiSelect": { "type": "boolean", "description": "Several answers may hold at once." }
                        },
                        "required": ["question", "options"]
                    }
                }
            },
            "required": ["questions"]
        }
    })
}

/// Serve until stdin closes. Never returns an error to the caller: a broken
/// message gets a JSON-RPC error, a broken pipe ends the loop.
pub fn serve() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(
                    out,
                    "{}",
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}})
                );
                let _ = out.flush();
                continue;
            }
        };
        if let Some(reply) = handle(&msg) {
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
}

/// One request in, one response out; notifications get none.
pub fn handle(msg: &Value) -> Option<Value> {
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = msg.get("id").cloned();
    if id.is_none() || id == Some(Value::Null) {
        // A notification (`notifications/initialized`, cancellations): no reply.
        return None;
    }
    let id = id.unwrap();
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2025-06-18");
            Ok(json!({
                "protocolVersion": requested,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "perch", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Use ask_user for every question to the user; never ask in chat."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [tool_definition()] })),
        "tools/call" => call(&id, &params),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
        Err((code, message)) => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
        }
    })
}

fn call(id: &Value, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    if name != TOOL_NAME {
        return Err((-32602, format!("unknown tool: {name}")));
    }
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let questions = crate::model::Question::parse_list(&args);
    if questions.is_empty() {
        return Ok(tool_text(
            "ask_user needs at least one question with a `question` string and `options`.",
            true,
        ));
    }
    let payload = json!({
        "event": "ask_user",
        "session_id": std::env::var("CODEX_THREAD_ID").ok(),
        "cwd": std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        "tool_use_id": format!("mcp-{}", id_text(id)),
        "tool_input": args,
    });
    let answers = ask_through_hook(&payload);
    match answers {
        Some(map) if !map.is_empty() => {
            let lines: Vec<String> = map
                .iter()
                .map(|(q, a)| format!("{q} → {}", a.as_str().unwrap_or("")))
                .collect();
            Ok(tool_text(
                &format!("User answered:\n{}", lines.join("\n")),
                false,
            ))
        }
        _ => Ok(tool_text(
            "The user did not answer through perch (they closed the popup, or perch cannot see this pane). Ask the question in chat instead: number the questions, letter the options, mark your recommendation, and wait.",
            false,
        )),
    }
}

fn id_text(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn tool_text(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// Run `perch hook codex --ask` with the payload and read its answer.
fn ask_through_hook(payload: &Value) -> Option<serde_json::Map<String, Value>> {
    let exe = std::env::current_exe().ok()?;
    let mut child = Command::new(exe)
        .args(["hook", "codex", "--ask"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.to_string().as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    v.get("answers")?.as_object().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_lists_the_tool_and_ignores_notifications() {
        let r = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "perch");
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let r = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        assert_eq!(r["result"]["tools"][0]["name"], "ask_user");
        assert_eq!(
            r["result"]["tools"][0]["inputSchema"]["required"][0],
            "questions"
        );
        assert_eq!(r["result"]["tools"][0]["annotations"]["readOnlyHint"], true);
        let r = handle(&json!({"jsonrpc":"2.0","id":3,"method":"nope"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(
            &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"other"}}),
        )
        .unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }
}
