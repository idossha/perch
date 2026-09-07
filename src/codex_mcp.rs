//! The `[mcp_servers.perch]` entry in codex's `config.toml`: how Codex gets
//! the `ask_user` tool. Written by `perch install codex`, removed by
//! `perch uninstall`, reported by `perch doctor`. Edited with `toml_edit` so
//! the rest of the file — comments, order, the user's other servers — survives.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use toml_edit::{value, Array, DocumentMut, Item, Table};

pub const SERVER: &str = "perch";
/// Environment the hook needs and Codex would otherwise strip from an MCP
/// server: the pane identity, the tmux socket, and perch's own overrides.
pub const ENV_VARS: &[&str] = &["TMUX_PANE", "TMUX", "PERCH_STATE_DIR", "PERCH_CONFIG_DIR"];

fn parse_doc(path: &Path) -> Result<DocumentMut> {
    let body = fs::read_to_string(path).unwrap_or_default();
    body.parse::<DocumentMut>()
        .with_context(|| format!("{} is not valid TOML", path.display()))
}

/// The entry as perch wants it.
fn desired() -> Table {
    let mut t = Table::new();
    t.insert("command", value("perch"));
    let mut args = Array::new();
    args.push("mcp");
    t.insert("args", value(args));
    let mut env = Array::new();
    for v in ENV_VARS {
        env.push(*v);
    }
    t.insert("env_vars", value(env));
    let mut tools = Table::new();
    let mut ask = Table::new();
    // `approve`: never gate the call. `auto` would read the tool's
    // annotations, which also say read-only, but a session with
    // `approval_policy = "never"` must not be able to refuse a question.
    ask.insert("approval_mode", value("approve"));
    tools.insert(crate::mcp::TOOL_NAME, Item::Table(ask));
    t.insert("tools", Item::Table(tools));
    t
}

/// `true` when the document already carries the entry as desired.
pub fn is_installed(doc: &DocumentMut) -> bool {
    let Some(server) = doc
        .get("mcp_servers")
        .and_then(|m| m.as_table())
        .and_then(|m| m.get(SERVER))
        .and_then(|s| s.as_table())
    else {
        return false;
    };
    let command_ok = server.get("command").and_then(|c| c.as_str()) == Some("perch");
    let args_ok = server
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>() == ["mcp"])
        .unwrap_or(false);
    let env_ok = server
        .get("env_vars")
        .and_then(|a| a.as_array())
        .map(|a| {
            let have: Vec<&str> = a.iter().filter_map(|v| v.as_str()).collect();
            ENV_VARS.iter().all(|e| have.contains(e))
        })
        .unwrap_or(false);
    let approval_ok = server
        .get("tools")
        .and_then(|t| t.as_table())
        .and_then(|t| t.get(crate::mcp::TOOL_NAME))
        .and_then(|t| t.as_table())
        .and_then(|t| t.get("approval_mode"))
        .and_then(|a| a.as_str())
        == Some("approve");
    command_ok && args_ok && env_ok && approval_ok
}

/// Insert or refresh the entry. Returns `true` when the document changed.
pub fn upsert(doc: &mut DocumentMut) -> bool {
    if is_installed(doc) {
        return false;
    }
    let root = doc.as_table_mut();
    if !root.contains_key("mcp_servers") {
        root.insert("mcp_servers", Item::Table(Table::new()));
    }
    let servers = root
        .get_mut("mcp_servers")
        .and_then(|m| m.as_table_mut())
        .expect("mcp_servers is a table");
    servers.set_implicit(true);
    servers.insert(SERVER, Item::Table(desired()));
    true
}

/// Remove the entry. Returns `true` when the document changed.
pub fn remove(doc: &mut DocumentMut) -> bool {
    let Some(servers) = doc.get_mut("mcp_servers").and_then(|m| m.as_table_mut()) else {
        return false;
    };
    let removed = servers.remove(SERVER).is_some();
    if servers.is_empty() {
        doc.as_table_mut().remove("mcp_servers");
    }
    removed
}

/// Write the entry into codex's config, backing the file up first and
/// replacing it atomically. Returns a one-line report.
pub fn install(path: &Path, dry_run: bool) -> Result<String> {
    let mut doc = parse_doc(path)?;
    if !upsert(&mut doc) {
        return Ok(format!(
            "mcp     ask_user already registered in {}\n",
            path.display()
        ));
    }
    if dry_run {
        return Ok(format!(
            "mcp     would register ask_user in {}\n",
            path.display()
        ));
    }
    write_doc(path, &doc)?;
    Ok(format!(
        "mcp     registered ask_user in {}\n",
        path.display()
    ))
}

/// Drop the entry again.
pub fn uninstall(path: &Path, dry_run: bool) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    let mut doc = parse_doc(path)?;
    if !remove(&mut doc) {
        return Ok(String::new());
    }
    if dry_run {
        return Ok(format!(
            "{}: would remove the perch MCP server\n",
            path.display()
        ));
    }
    write_doc(path, &doc)?;
    Ok(format!("{}: perch MCP server removed\n", path.display()))
}

/// `true` when the file on disk carries the entry as desired.
pub fn installed_at(path: &Path) -> bool {
    parse_doc(path).map(|d| is_installed(&d)).unwrap_or(false)
}

fn write_doc(path: &Path, doc: &DocumentMut) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if path.exists() {
        let backup = crate::install::backup_path(path);
        fs::copy(path, &backup)?;
    }
    let tmp = path.with_extension(format!("perchtmp{}", std::process::id()));
    fs::write(&tmp, doc.to_string())?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_keeps_other_servers_and_is_idempotent() {
        let mut doc: DocumentMut = "model = \"gpt-5\"\n\n[mcp_servers.playwright]\ncommand = \"npx\"\nargs = [\"-y\", \"@playwright/mcp\"]\n"
            .parse()
            .unwrap();
        assert!(upsert(&mut doc));
        assert!(is_installed(&doc));
        assert!(!upsert(&mut doc), "second run changes nothing");
        let s = doc.to_string();
        assert!(s.contains("model = \"gpt-5\""), "{s}");
        assert!(s.contains("[mcp_servers.playwright]"), "{s}");
        assert!(s.contains("[mcp_servers.perch]"), "{s}");
        assert!(
            s.contains(
                "env_vars = [\"TMUX_PANE\", \"TMUX\", \"PERCH_STATE_DIR\", \"PERCH_CONFIG_DIR\"]"
            ),
            "{s}"
        );
        assert!(s.contains("[mcp_servers.perch.tools.ask_user]"), "{s}");
        assert!(s.contains("approval_mode = \"approve\""), "{s}");
        assert!(remove(&mut doc));
        let s = doc.to_string();
        assert!(!s.contains("perch"), "{s}");
        assert!(
            s.contains("[mcp_servers.playwright]"),
            "the other server survives: {s}"
        );
        assert!(!remove(&mut doc));
    }

    #[test]
    fn a_stale_entry_is_refreshed() {
        let mut doc: DocumentMut = "[mcp_servers.perch]\ncommand = \"perch\"\nargs = [\"mcp\"]\n"
            .parse()
            .unwrap();
        assert!(!is_installed(&doc), "no env_vars, no approval");
        assert!(upsert(&mut doc));
        assert!(is_installed(&doc));
    }
}
