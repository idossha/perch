//! Codex hook trust records.
//!
//! Codex runs a hook only once the user has accepted it in the TUI; the
//! acceptance is stored in `~/.codex/config.toml` as
//!
//! ```toml
//! [hooks.state."<abs hooks.json>:<event_label>:<group>:<handler>"]
//! trusted_hash = "sha256:<hex>"
//! ```
//!
//! perch writes exactly the record the user would have got by accepting the
//! prompt, so `perch setup` stays one step. The hash recipe mirrors codex's
//! `hook_hash` (codex-rs/hooks/src/engine/discovery.rs) over the canonical
//! form from `version_for_toml` (codex-rs/config/src/fingerprint.rs): a small
//! identity object, keys sorted recursively, compact JSON, sha256, lowercase
//! hex, `sha256:` prefixed.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, Table, Value as TomlValue};

/// Codex's default per-handler timeout, used when a group declares none.
const DEFAULT_TIMEOUT: u64 = 600;

/// The event label codex uses in a trust key, for a hook event name.
pub fn event_label(event: &str) -> String {
    let mut out = String::with_capacity(event.len() + 4);
    for (i, c) in event.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Every event label codex knows, in its own order. Kept for documentation and
/// so a typo in an event name shows up as an unknown label rather than silently
/// writing a key codex will never look up.
pub const EVENT_LABELS: &[&str] = &[
    "session_start",
    "user_prompt_submit",
    "stop",
    "permission_request",
    "session_end",
    "subagent_start",
    "subagent_stop",
    "pre_tool_use",
    "post_tool_use",
    "pre_compact",
    "post_compact",
    "interrupt",
];

/// Recursively sort object keys, then serialise compactly.
fn canonical(value: &Value) -> Vec<u8> {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                // serde_json's Map is ordered; rebuilding through a BTreeMap of
                // keys makes the sort explicit and independent of that default.
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&o[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_vec(&sorted(value)).unwrap_or_default()
}

/// The identity object codex hashes for one handler of one group.
///
/// `matcher` is included only when the group actually has the key — an empty
/// string is a real matcher and hashes differently from no matcher at all.
pub fn handler_identity(event_label: &str, group: &Value, handler_index: usize) -> Option<Value> {
    let handler = group.get("hooks")?.as_array()?.get(handler_index)?;
    let command = handler.get("command")?.as_str()?;
    let timeout = handler
        .get("timeout")
        .and_then(|t| t.as_u64())
        .or_else(|| group.get("timeout").and_then(|t| t.as_u64()))
        .unwrap_or(DEFAULT_TIMEOUT);
    let mut identity = json!({
        "event_name": event_label,
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": timeout,
            "async": false,
        }],
    });
    if let Some(m) = group.get("matcher") {
        identity["matcher"] = m.clone();
    }
    Some(identity)
}

/// `sha256:<hex>` for one handler, or `None` when the group has no such handler.
pub fn hook_hash(event_label: &str, group: &Value, handler_index: usize) -> Option<String> {
    let identity = handler_identity(event_label, group, handler_index)?;
    let digest = Sha256::digest(canonical(&identity));
    Some(format!("sha256:{digest:x}"))
}

/// One trust record: the config key and the hash it must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEntry {
    pub key: String,
    pub hash: String,
}

/// Trust records for every perch handler in a merged `hooks.json` document.
///
/// Indices are read from the document as it will be written, so reordering the
/// file and re-running `perch setup` refreshes them.
pub fn entries_for(hooks_path: &Path, hooks_doc: &Value, marker: &str) -> Vec<TrustEntry> {
    let mut out = Vec::new();
    let Some(events) = hooks_doc.get("hooks").and_then(|h| h.as_object()) else {
        return out;
    };
    for (event, groups) in events {
        let label = event_label(event);
        let Some(groups) = groups.as_array() else {
            continue;
        };
        for (gi, group) in groups.iter().enumerate() {
            let Some(handlers) = group.get("hooks").and_then(|h| h.as_array()) else {
                continue;
            };
            for (hi, handler) in handlers.iter().enumerate() {
                let is_perch = handler
                    .get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains(marker));
                if !is_perch {
                    continue;
                }
                if let Some(hash) = hook_hash(&label, group, hi) {
                    out.push(TrustEntry {
                        key: format!("{}:{}:{}:{}", hooks_path.display(), label, gi, hi),
                        hash,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// `~/.codex/config.toml`, or `PERCH_CODEX_CONFIG`.
pub fn codex_config_path() -> PathBuf {
    crate::paths::Paths::from_env().codex_config
}

fn parse_doc(path: &Path) -> Result<DocumentMut> {
    let body = fs::read_to_string(path).unwrap_or_default();
    body.parse::<DocumentMut>()
        .with_context(|| format!("{} is not valid TOML", path.display()))
}

/// `true` when every entry is already recorded with the right hash.
pub fn all_trusted(path: &Path, entries: &[TrustEntry]) -> bool {
    if entries.is_empty() {
        return true;
    }
    let Ok(doc) = parse_doc(path) else {
        return false;
    };
    entries.iter().all(|e| {
        doc.get("hooks")
            .and_then(|h| h.get("state"))
            .and_then(|s| s.get(&e.key))
            .and_then(|t| t.get("trusted_hash"))
            .and_then(|v| v.as_str())
            == Some(e.hash.as_str())
    })
}

fn state_table(doc: &mut DocumentMut) -> &mut Table {
    let hooks = doc
        .as_table_mut()
        .entry("hooks")
        .or_insert_with(|| Item::Table(Table::new()));
    if !hooks.is_table() {
        *hooks = Item::Table(Table::new());
    }
    let hooks = hooks.as_table_mut().expect("table");
    hooks.set_implicit(true);
    let state = hooks
        .entry("state")
        .or_insert_with(|| Item::Table(Table::new()));
    if !state.is_table() {
        *state = Item::Table(Table::new());
    }
    let state = state.as_table_mut().expect("table");
    state.set_implicit(true);
    state
}

/// Upsert every entry into the document, returning `true` when it changed.
pub fn upsert(doc: &mut DocumentMut, entries: &[TrustEntry]) -> bool {
    let mut changed = false;
    let state = state_table(doc);
    for e in entries {
        let entry = state
            .entry(&e.key)
            .or_insert_with(|| Item::Table(Table::new()));
        if !entry.is_table() {
            *entry = Item::Table(Table::new());
        }
        let t = entry.as_table_mut().expect("table");
        let current = t
            .get("trusted_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if current.as_deref() != Some(e.hash.as_str()) {
            t.insert("trusted_hash", Item::Value(TomlValue::from(e.hash.clone())));
            changed = true;
        }
    }
    changed
}

/// Remove every perch trust key (those naming `hooks_path` and holding a
/// perch-shaped key), returning `true` when the document changed.
pub fn remove_keys(doc: &mut DocumentMut, keys: &[String]) -> bool {
    let mut changed = false;
    let Some(hooks) = doc.get_mut("hooks").and_then(|h| h.as_table_mut()) else {
        return false;
    };
    if let Some(state) = hooks.get_mut("state").and_then(|s| s.as_table_mut()) {
        for k in keys {
            if state.remove(k).is_some() {
                changed = true;
            }
        }
        if state.is_empty() {
            hooks.remove("state");
        }
    }
    if hooks.is_empty() {
        doc.as_table_mut().remove("hooks");
    }
    changed
}

/// Write the trust records for `entries` into codex's config, backing the file
/// up first and replacing it atomically. Returns a one-line report.
pub fn install(path: &Path, entries: &[TrustEntry], dry_run: bool) -> Result<String> {
    if entries.is_empty() {
        return Ok("trust   no perch handlers to trust\n".to_string());
    }
    let mut doc = parse_doc(path)?;
    if !upsert(&mut doc, entries) {
        return Ok(format!("trust   already recorded in {}\n", path.display()));
    }
    if dry_run {
        return Ok(format!(
            "trust   would record {} handler(s) in {}\n",
            entries.len(),
            path.display()
        ));
    }
    write_doc(path, &doc)?;
    Ok(format!(
        "trust   recorded {} handler(s) in {}\n",
        entries.len(),
        path.display()
    ))
}

/// Drop perch's trust records again.
pub fn uninstall(path: &Path, keys: &[String], dry_run: bool) -> Result<String> {
    if !path.exists() || keys.is_empty() {
        return Ok(String::new());
    }
    let mut doc = parse_doc(path)?;
    if !remove_keys(&mut doc, keys) {
        return Ok(String::new());
    }
    if dry_run {
        return Ok(format!(
            "{}: would remove perch trust records\n",
            path.display()
        ));
    }
    write_doc(path, &doc)?;
    Ok(format!("{}: perch trust records removed\n", path.display()))
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
    fn labels_are_snake_case() {
        assert_eq!(event_label("SessionStart"), "session_start");
        assert_eq!(event_label("UserPromptSubmit"), "user_prompt_submit");
        assert_eq!(event_label("Stop"), "stop");
        for e in [
            "SessionStart",
            "UserPromptSubmit",
            "Stop",
            "PermissionRequest",
            "SessionEnd",
            "SubagentStart",
            "SubagentStop",
            "PreToolUse",
            "PostToolUse",
            "PreCompact",
            "PostCompact",
            "Interrupt",
        ] {
            assert!(EVENT_LABELS.contains(&event_label(e).as_str()), "{e}");
        }
    }

    /// The two pairs the recipe was derived from, straight out of a real
    /// `~/.codex/config.toml`.
    #[test]
    fn hashes_match_codex() {
        let group = json!({"hooks":[{"type":"command","command":"bash '/Users/idohaber/.codex/herdr-agent-state.sh' session","timeout":10}]});
        assert_eq!(
            hook_hash("session_start", &group, 0).unwrap(),
            "sha256:62893f36ebfc36aaf3660a20494d3aad7c9f8efed1ead6b6e78da6087fdb9283"
        );
        let group = json!({"matcher":"","hooks":[{"type":"command","command":"chrome-devtools-axi","timeout":10}]});
        assert_eq!(
            hook_hash("session_start", &group, 0).unwrap(),
            "sha256:2035963d8116b7f9c8418a165ee1d22cc62fa076e6536be410a8534452869541"
        );
    }

    #[test]
    fn a_missing_matcher_hashes_differently_from_an_empty_one() {
        let bare = json!({"hooks":[{"type":"command","command":"x","timeout":10}]});
        let empty = json!({"matcher":"","hooks":[{"type":"command","command":"x","timeout":10}]});
        assert_ne!(
            hook_hash("stop", &bare, 0).unwrap(),
            hook_hash("stop", &empty, 0).unwrap()
        );
    }

    #[test]
    fn entries_use_real_indices_and_only_perch_handlers() {
        let doc = json!({"hooks": {
            "Stop": [
                {"hooks":[{"type":"command","command":"other","timeout":10}]},
                {"hooks":[{"type":"command","command":"perch hook codex","timeout":10}]}
            ],
            "SessionStart": [
                {"matcher":"*","hooks":[{"type":"command","command":"perch hook codex","timeout":10}]}
            ]
        }});
        let entries = entries_for(Path::new("/h/hooks.json"), &doc, "perch hook");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "/h/hooks.json:session_start:0:0");
        assert_eq!(entries[1].key, "/h/hooks.json:stop:1:0");
        assert!(entries[0].hash.starts_with("sha256:"));
    }

    #[test]
    fn upsert_keeps_foreign_keys_and_formatting() {
        let mut doc =
            "model = \"gpt-5\"\n\n[hooks.state.\"/x:stop:0:0\"]\ntrusted_hash = \"sha256:aa\"\n"
                .parse::<DocumentMut>()
                .unwrap();
        let entries = vec![TrustEntry {
            key: "/h/hooks.json:stop:1:0".into(),
            hash: "sha256:bb".into(),
        }];
        assert!(upsert(&mut doc, &entries));
        let s = doc.to_string();
        assert!(s.starts_with("model = \"gpt-5\""), "{s}");
        assert!(s.contains("\"/x:stop:0:0\""), "{s}");
        assert!(s.contains("sha256:bb"), "{s}");
        // Second pass is a no-op.
        assert!(!upsert(&mut doc, &entries));
    }

    #[test]
    fn remove_takes_only_the_named_keys() {
        let mut doc = "[hooks.state.\"/x:stop:0:0\"]\ntrusted_hash = \"sha256:aa\"\n\n[hooks.state.\"/y:stop:0:0\"]\ntrusted_hash = \"sha256:bb\"\n"
            .parse::<DocumentMut>()
            .unwrap();
        assert!(remove_keys(&mut doc, &["/y:stop:0:0".to_string()]));
        let s = doc.to_string();
        assert!(s.contains("/x:stop:0:0"));
        assert!(!s.contains("/y:stop:0:0"));
    }
}
