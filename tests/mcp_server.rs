//! `perch mcp`, driven as Codex would drive it: JSON-RPC lines on stdin, the
//! `initialize` / `tools/list` / `tools/call` trio, and an `ask_user` call
//! that is answered through the popup's answer file while the tool waits.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn wait_for(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    cond()
}

#[test]
fn codex_asks_through_the_mcp_tool_and_gets_the_popups_answer() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let mut child = Command::new(env!("CARGO_BIN_EXE_perch"))
        .arg("mcp")
        .env("PERCH_STATE_DIR", p)
        .env("PERCH_CONFIG_DIR", p.join("config"))
        .env("PERCH_NO_SOUND", "1")
        .env("PERCH_NO_TMUX", "1")
        .env("PERCH_TMUX_LOG", p.join("tmux.log"))
        // What Codex forwards through `env_vars`: the pane the tool speaks for.
        .env("TMUX_PANE", "%999")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut send = |v: serde_json::Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let recv = |out: &mut BufReader<_>| -> serde_json::Value {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    };

    send(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"codex","version":"0"}}}),
    );
    let r = recv(&mut out);
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["serverInfo"]["name"], "perch");
    send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let r = recv(&mut out);
    assert_eq!(r["result"]["tools"][0]["name"], "ask_user");

    // The call blocks in the tool while the popup is up; answer it.
    let body: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/fixtures/codex/ask_user.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    send(
        serde_json::json!({"jsonrpc":"2.0","id":"call-1","method":"tools/call","params":{"name":"ask_user","arguments":body["tool_input"]}}),
    );
    let rec = p.join("panes/_999.json");
    assert!(
        wait_for(
            || std::fs::read_to_string(&rec)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .is_some_and(
                    |r| r["question"]["tool_use_id"] == "mcp-call-1" && r["harness"] == "codex"
                ),
            Duration::from_secs(5)
        ),
        "the tool never reached the hook"
    );
    assert!(
        wait_for(
            || std::fs::read_to_string(p.join("tmux.log"))
                .unwrap_or_default()
                .contains("ask-popup %999"),
            Duration::from_secs(3)
        ),
        "the popup, as for Claude"
    );
    std::fs::create_dir_all(p.join("answers")).unwrap();
    std::fs::write(
        p.join("answers/_999.json"),
        serde_json::json!({"tool_use_id":"mcp-call-1","answers":{"Which store backend should perch use?":"SQLite","Which harnesses need the change?":"claude, codex"}}).to_string(),
    )
    .unwrap();
    let r = recv(&mut out);
    assert_eq!(r["id"], "call-1");
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("Which store backend should perch use? → SQLite"),
        "{text}"
    );
    assert!(
        text.contains("Which harnesses need the change? → claude, codex"),
        "{text}"
    );
    assert_eq!(r["result"]["isError"], false);

    // Handed back: the tool tells the model to ask in chat.
    send(
        serde_json::json!({"jsonrpc":"2.0","id":"call-2","method":"tools/call","params":{"name":"ask_user","arguments":body["tool_input"]}}),
    );
    assert!(wait_for(
        || std::fs::read_to_string(&rec)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .is_some_and(|r| r["question"]["tool_use_id"] == "mcp-call-2"),
        Duration::from_secs(5)
    ));
    std::fs::write(
        p.join("answers/_999.json"),
        serde_json::json!({"tool_use_id":"mcp-call-2","defer":true}).to_string(),
    )
    .unwrap();
    let r = recv(&mut out);
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("ask the question in chat") || text.contains("Ask the question in chat"),
        "{text}"
    );

    drop(stdin);
    let _ = child.wait();
}

/// Regression: Codex refused the first live call with "MCP tool call requires
/// approval, but approval policy is never". Two things keep that from coming
/// back: the tool declares itself read-only in its MCP annotations, and the
/// registered entry gates it with `approve`, not `auto` (which reads the
/// annotations and, for a tool without them, requires approval).
#[test]
fn the_tool_is_declared_read_only_and_registered_as_never_gated() {
    let out = Command::new(env!("CARGO_BIN_EXE_perch"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut c| {
            c.stdin
                .take()
                .unwrap()
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n")?;
            c.wait_with_output()
        })
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tool = &v["result"]["tools"][0];
    assert_eq!(tool["name"], "ask_user");
    assert_eq!(tool["annotations"]["readOnlyHint"], true, "{tool}");
    assert_eq!(tool["annotations"]["destructiveHint"], false, "{tool}");

    // The config entry, as setup writes it — and an `auto` entry from the
    // first cut is refreshed to `approve`.
    let mut doc: toml_edit::DocumentMut =
        "[mcp_servers.perch]\ncommand = \"perch\"\nargs = [\"mcp\"]\nenv_vars = [\"TMUX_PANE\", \"TMUX\", \"PERCH_STATE_DIR\", \"PERCH_CONFIG_DIR\"]\n\n[mcp_servers.perch.tools.ask_user]\napproval_mode = \"auto\"\n"
            .parse()
            .unwrap();
    assert!(!perch::codex_mcp::is_installed(&doc), "auto is the bug");
    assert!(perch::codex_mcp::upsert(&mut doc));
    let s = doc.to_string();
    assert!(s.contains("approval_mode = \"approve\""), "{s}");
    assert!(!s.contains("approval_mode = \"auto\""), "{s}");
    assert!(perch::codex_mcp::is_installed(&doc));
}
