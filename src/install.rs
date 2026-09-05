//! Idempotent, backup-first installers for the harness hooks and the tmux config.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// The Claude hook events perch subscribes to.
pub const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "SessionEnd",
];

/// The Codex hook events perch subscribes to. Codex names `PermissionRequest`
/// where Claude sends a `Notification`; the rest of the names line up.
pub const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "PermissionRequest",
    "SessionEnd",
];

/// Marker used to decide whether perch is already installed in a hook array.
const MARKER: &str = "perch hook";

pub fn claude_settings_path() -> PathBuf {
    if let Ok(p) = std::env::var("PERCH_CLAUDE_SETTINGS") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude/settings.json")
}

pub fn codex_hooks_path() -> PathBuf {
    if let Ok(p) = std::env::var("PERCH_CODEX_HOOKS") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex/hooks.json")
}

/// Directory holding pi's TypeScript extensions.
pub fn pi_extension_dir() -> PathBuf {
    if let Ok(p) = std::env::var("PERCH_PI_EXT_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pi/agent/extensions")
}

pub fn tmux_conf_path() -> PathBuf {
    if let Ok(p) = std::env::var("PERCH_TMUX_CONF") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tmux.conf")
}

pub fn perch_tmux_conf_path() -> PathBuf {
    crate::config::config_dir().join("perch.tmux.conf")
}

/// The hook group perch appends. `matcher` is set for the events that take one.
fn perch_group(event: &str, command: &str, timeout: u64) -> Value {
    let hooks = json!([{ "type": "command", "command": command, "timeout": timeout }]);
    if event == "SessionStart" {
        json!({ "matcher": "*", "hooks": hooks })
    } else {
        json!({ "hooks": hooks })
    }
}

/// `true` when any command string anywhere in this event's array mentions perch.
fn already_installed(arr: &Value) -> bool {
    let Some(groups) = arr.as_array() else {
        return false;
    };
    groups.iter().any(|g| {
        g.get("hooks")
            .and_then(|h| h.as_array())
            .map(|hs| {
                hs.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c.contains(MARKER))
                })
            })
            .unwrap_or(false)
    })
}

/// Result of a merge: the new document plus which events were touched.
pub struct Merge {
    pub settings: Value,
    pub added: Vec<String>,
    pub skipped: Vec<String>,
}

/// Append perch's hook group to each event array, never reordering or dropping
/// what is already there.
pub fn merge_claude_settings(settings: Value) -> Merge {
    merge_hooks(settings, CLAUDE_EVENTS, "perch hook claude", 5)
}

/// The same merge against `~/.codex/hooks.json`, which has Claude's shape.
pub fn merge_codex_hooks(settings: Value) -> Merge {
    merge_hooks(settings, CODEX_EVENTS, "perch hook codex", 10)
}

fn merge_hooks(mut settings: Value, events: &[&str], command: &str, timeout: u64) -> Merge {
    if !settings.is_object() {
        settings = json!({});
    }
    let hooks = settings
        .as_object_mut()
        .expect("object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().expect("object");

    let mut added = Vec::new();
    let mut skipped = Vec::new();
    for event in events {
        let arr = hooks.entry(*event).or_insert_with(|| json!([]));
        if !arr.is_array() {
            *arr = json!([]);
        }
        if already_installed(arr) {
            skipped.push((*event).to_string());
            continue;
        }
        arr.as_array_mut()
            .expect("array")
            .push(perch_group(event, command, timeout));
        added.push((*event).to_string());
    }
    Merge {
        settings,
        added,
        skipped,
    }
}

/// Install (or preview) the Claude hooks.
pub fn install_claude(dry_run: bool, print: bool) -> Result<String> {
    install_hooks(
        claude_settings_path(),
        merge_claude_settings,
        dry_run,
        print,
    )
}

/// Install (or preview) the Codex hooks.
pub fn install_codex(dry_run: bool, print: bool) -> Result<String> {
    install_hooks(codex_hooks_path(), merge_codex_hooks, dry_run, print)
}

/// Merge perch into one JSON hooks file, backing it up first.
fn install_hooks(
    path: PathBuf,
    merge: fn(Value) -> Merge,
    dry_run: bool,
    print: bool,
) -> Result<String> {
    let current: Value = match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s)
            .with_context(|| format!("{} is not valid JSON", path.display()))?,
        _ => json!({}),
    };
    let merge = merge(current);
    let body = serde_json::to_string_pretty(&merge.settings)? + "\n";

    if print {
        return Ok(body);
    }
    let mut report = format!("settings: {}\n", path.display());
    if merge.added.is_empty() {
        report.push_str("already installed for every event; nothing to do\n");
        return Ok(report);
    }
    report.push_str(&format!(
        "would add perch hook to: {}\n",
        merge.added.join(", ")
    ));
    if !merge.skipped.is_empty() {
        report.push_str(&format!(
            "already present in: {}\n",
            merge.skipped.join(", ")
        ));
    }
    if dry_run {
        report.push_str("dry run: no files written\n");
        return Ok(report);
    }
    if path.exists() {
        let backup = backup_path(&path);
        fs::copy(&path, &backup)?;
        report.push_str(&format!("backed up to {}\n", backup.display()));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_atomic(&path, body.as_bytes())?;
    report.push_str("installed\n");
    Ok(report)
}

/// Write pi's extension to `~/.pi/agent/extensions/perch.ts`.
///
/// The extension is a static template embedded in the binary, so "installed"
/// means "byte-identical to the template"; a stale copy is replaced.
pub fn install_pi(dry_run: bool, print: bool) -> Result<String> {
    let body = crate::adapters::PI_EXTENSION;
    if print {
        return Ok(body.to_string());
    }
    let path = pi_extension_dir().join("perch.ts");
    let mut report = format!("extension: {}\n", path.display());

    if fs::read_to_string(&path).is_ok_and(|s| s == body) {
        report.push_str("already installed and up to date; nothing to do\n");
        return Ok(report);
    }
    let exists = path.exists();
    report.push_str(if exists {
        "would replace the existing perch.ts\n"
    } else {
        "would write a new perch.ts\n"
    });
    if dry_run {
        report.push_str("dry run: no files written\n");
        return Ok(report);
    }
    if exists {
        let backup = backup_path(&path);
        fs::copy(&path, &backup)?;
        report.push_str(&format!("backed up to {}\n", backup.display()));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    write_atomic(&path, body.as_bytes())?;
    report.push_str("installed\n");
    Ok(report)
}

/// `<path>.bak-<unix seconds>` beside the original.
pub fn backup_path(path: &Path) -> PathBuf {
    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "settings.json".into());
    path.with_file_name(format!("{name}.bak-{ts}"))
}

pub const TMUX_SNIPPET: &str = "\
# perch — managed by `perch install tmux`
bind g display-popup -E -w 85% -h 75% 'perch tui'
set -g status-right '#(perch status --format tmux) #{?client_prefix,^A ,}%H:%M'
";

/// Write `~/.config/perch/perch.tmux.conf` and report the source-file line.
pub fn install_tmux(dry_run: bool, apply: bool, print: bool) -> Result<String> {
    let conf = perch_tmux_conf_path();
    let source_line = format!("source-file {}", conf.display());
    if print {
        return Ok(TMUX_SNIPPET.to_string());
    }

    let mut report = String::new();
    report.push_str(&format!("perch tmux config: {}\n", conf.display()));
    if !dry_run {
        if let Some(dir) = conf.parent() {
            fs::create_dir_all(dir)?;
        }
        write_atomic(&conf, TMUX_SNIPPET.as_bytes())?;
        report.push_str("written\n");
    } else {
        report.push_str("dry run: not written\n");
    }

    let tmux_conf = tmux_conf_path();
    let existing = fs::read_to_string(&tmux_conf).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == source_line) {
        report.push_str(&format!("{} already sources it\n", tmux_conf.display()));
        return Ok(report);
    }
    if apply && !dry_run {
        let backup = backup_path(&tmux_conf);
        if tmux_conf.exists() {
            fs::copy(&tmux_conf, &backup)?;
            report.push_str(&format!("backed up to {}\n", backup.display()));
        }
        let mut body = existing;
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&source_line);
        body.push('\n');
        write_atomic(&tmux_conf, body.as_bytes())?;
        report.push_str(&format!("appended to {}\n", tmux_conf.display()));
    } else {
        report.push_str(&format!(
            "add this line to {} (or run with --apply):\n{}\n",
            tmux_conf.display(),
            source_line
        ));
    }
    Ok(report)
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("perchtmp{}", std::process::id()));
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_and_preserves_existing() {
        let before = json!({
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "idosleep hook"}]}],
                "SessionStart": [{"matcher": "*", "hooks": [{"type": "command", "command": "herdr hook"}]}]
            },
            "model": "opus"
        });
        let m = merge_claude_settings(before);
        let stop = m.settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "idosleep hook");
        assert_eq!(stop[1]["hooks"][0]["command"], "perch hook claude");
        assert_eq!(stop[1]["hooks"][0]["timeout"], 5);
        assert_eq!(m.settings["hooks"]["SessionStart"][1]["matcher"], "*");
        assert_eq!(m.settings["model"], "opus");
        assert_eq!(m.added.len(), CLAUDE_EVENTS.len());
    }

    #[test]
    fn merge_is_idempotent() {
        let once = merge_claude_settings(json!({})).settings;
        let twice = merge_claude_settings(once.clone());
        assert_eq!(twice.settings, once);
        assert!(twice.added.is_empty());
        assert_eq!(twice.skipped.len(), CLAUDE_EVENTS.len());
    }

    #[test]
    fn only_session_start_gets_a_matcher() {
        let m = merge_claude_settings(json!({}));
        assert!(m.settings["hooks"]["Stop"][0].get("matcher").is_none());
        assert_eq!(m.settings["hooks"]["SessionStart"][0]["matcher"], "*");
    }

    #[test]
    fn tmux_snippet_has_the_popup_binding() {
        assert!(TMUX_SNIPPET.contains("bind g display-popup -E -w 85% -h 75% 'perch tui'"));
    }
}
