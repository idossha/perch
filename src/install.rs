//! Idempotent, backup-first installers for the harness hooks and the tmux config.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::paths::Paths;

/// The Claude hook events perch subscribes to.
pub const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
    "PreToolUse",
];

/// The Codex hook events perch subscribes to. Codex names `PermissionRequest`
/// where Claude sends a `Notification`; the rest of the names line up.
pub const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "PermissionRequest",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
    "PreToolUse",
];

/// Marker used to decide whether perch is already installed in a hook array.
pub const MARKER: &str = "perch hook";

pub fn claude_settings_path() -> PathBuf {
    Paths::from_env().claude_settings
}

pub fn codex_hooks_path() -> PathBuf {
    Paths::from_env().codex_hooks
}

/// Directory holding pi's TypeScript extensions.
pub fn pi_extension_dir() -> PathBuf {
    Paths::from_env().pi_ext_dir
}

/// The codex hooks document as it stands after a merge, without writing it.
///
/// The merge is idempotent, so this is the file's own content once perch is
/// installed and the file-to-be while a dry run is being reported.
pub fn codex_merged_doc(path: &Path) -> Value {
    let current: Value = match fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s).unwrap_or_else(|_| json!({})),
        _ => json!({}),
    };
    merge_codex_hooks(current).settings
}

/// Trust records codex needs for perch's own handlers in that document.
pub fn codex_trust_entries(path: &Path) -> Vec<crate::trust::TrustEntry> {
    crate::trust::entries_for(path, &codex_merged_doc(path), MARKER)
}

pub fn tmux_conf_path() -> PathBuf {
    Paths::from_env().tmux_conf
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
    /// Events whose array perch itself created, so uninstall may remove them.
    pub created_events: Vec<String>,
    /// `true` when perch created the top-level `hooks` object.
    pub created_hooks_key: bool,
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
    let root = settings.as_object_mut().expect("object");
    let created_hooks_key = !root.contains_key("hooks");
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().expect("object");

    let mut added = Vec::new();
    let mut skipped = Vec::new();
    let mut created_events = Vec::new();
    for event in events {
        if !hooks.contains_key(*event) {
            created_events.push((*event).to_string());
        }
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
        created_events,
        created_hooks_key,
    }
}

/// What one installer did, for the `perch setup` table and the marker file.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// `true` when nothing needed doing.
    pub already: bool,
    /// The file the installer targets.
    pub file: PathBuf,
    pub backup: Option<PathBuf>,
    /// Hook event arrays perch created (so uninstall may remove them again).
    pub created_events: Vec<String>,
    pub created_hooks_key: bool,
    pub created_file: bool,
}

/// Install (or preview) the Claude hooks.
pub fn install_claude(dry_run: bool, print: bool) -> Result<String> {
    Ok(install_hooks(
        claude_settings_path(),
        merge_claude_settings,
        dry_run,
        print,
    )?
    .0)
}

/// Install (or preview) the Codex hooks.
pub fn install_codex(dry_run: bool, print: bool) -> Result<String> {
    Ok(install_hooks(codex_hooks_path(), merge_codex_hooks, dry_run, print)?.0)
}

/// Install the Claude hooks at an explicit path, reporting structurally.
pub fn install_claude_at(path: &Path, dry_run: bool) -> Result<(String, Outcome)> {
    install_hooks(path.to_path_buf(), merge_claude_settings, dry_run, false)
}

/// Install the Codex hooks at an explicit path, reporting structurally.
pub fn install_codex_at(path: &Path, dry_run: bool) -> Result<(String, Outcome)> {
    install_hooks(path.to_path_buf(), merge_codex_hooks, dry_run, false)
}

/// Merge perch into one JSON hooks file, backing it up first.
fn install_hooks(
    path: PathBuf,
    merge: fn(Value) -> Merge,
    dry_run: bool,
    print: bool,
) -> Result<(String, Outcome)> {
    let current: Value = match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s)
            .with_context(|| format!("{} is not valid JSON", path.display()))?,
        _ => json!({}),
    };
    let merge = merge(current);
    let body = serde_json::to_string_pretty(&merge.settings)? + "\n";
    let mut outcome = Outcome {
        file: path.clone(),
        created_file: !path.exists(),
        created_events: merge.created_events.clone(),
        created_hooks_key: merge.created_hooks_key,
        ..Outcome::default()
    };

    if print {
        return Ok((body, outcome));
    }
    let mut report = format!("settings: {}\n", path.display());
    if merge.added.is_empty() {
        report.push_str("already installed for every event; nothing to do\n");
        outcome.already = true;
        return Ok((report, outcome));
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
        return Ok((report, outcome));
    }
    if path.exists() {
        let backup = backup_path(&path);
        fs::copy(&path, &backup)?;
        report.push_str(&format!("backed up to {}\n", backup.display()));
        outcome.backup = Some(backup);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_atomic(&path, body.as_bytes())?;
    report.push_str("installed\n");
    Ok((report, outcome))
}

/// Write pi's extension to `~/.pi/agent/extensions/perch.ts`.
///
/// The extension is a static template embedded in the binary, so "installed"
/// means "byte-identical to the template"; a stale copy is replaced.
pub fn install_pi(dry_run: bool, print: bool) -> Result<String> {
    Ok(install_pi_at(&pi_extension_dir(), dry_run, print)?.0)
}

/// Write pi's extension into an explicit extension directory.
pub fn install_pi_at(ext_dir: &Path, dry_run: bool, print: bool) -> Result<(String, Outcome)> {
    let body = crate::adapters::PI_EXTENSION;
    let path = ext_dir.join("perch.ts");
    let mut outcome = Outcome {
        file: path.clone(),
        created_file: !path.exists(),
        ..Outcome::default()
    };
    if print {
        return Ok((body.to_string(), outcome));
    }
    let mut report = format!("extension: {}\n", path.display());

    if fs::read_to_string(&path).is_ok_and(|s| s == body) {
        report.push_str("already installed and up to date; nothing to do\n");
        outcome.already = true;
        return Ok((report, outcome));
    }
    let exists = path.exists();
    report.push_str(if exists {
        "would replace the existing perch.ts\n"
    } else {
        "would write a new perch.ts\n"
    });
    if dry_run {
        report.push_str("dry run: no files written\n");
        return Ok((report, outcome));
    }
    if exists {
        let backup = backup_path(&path);
        fs::copy(&path, &backup)?;
        report.push_str(&format!("backed up to {}\n", backup.display()));
        outcome.backup = Some(backup);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    write_atomic(&path, body.as_bytes())?;
    report.push_str("installed\n");
    Ok((report, outcome))
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
bind N run-shell 'perch next'
# Per-window flag: ⚑ waiting on you, ✓ finished. Empty the rest of the time.
set -ga window-status-format ' #{@perch_flag}'
set -ga window-status-current-format ' #{@perch_flag}'
# Looking at a finished pane marks it seen: done -> idle, flag cleared.
set -g focus-events on
set-hook -ga pane-focus-in \"run-shell -b 'perch seen #{pane_id}'\"
# Opt in to a status-line counter by prepending it to your theme's status-right, e.g.:
#   set -ga status-right '#(perch status --format tmux) '
";

/// Write `~/.config/perch/perch.tmux.conf` and report the source-file line.
pub fn install_tmux(dry_run: bool, apply: bool, print: bool) -> Result<String> {
    Ok(install_tmux_at(&Paths::from_env(), dry_run, apply, print)?.0)
}

/// Write the perch tmux config and, with `apply`, the `source-file` line.
pub fn install_tmux_at(
    paths: &Paths,
    dry_run: bool,
    apply: bool,
    print: bool,
) -> Result<(String, Outcome)> {
    let conf = paths.perch_tmux_conf();
    let tmux_conf = paths.tmux_conf.clone();
    let source_line = format!("source-file {}", conf.display());
    let mut outcome = Outcome {
        file: tmux_conf.clone(),
        created_file: !conf.exists(),
        ..Outcome::default()
    };
    if print {
        return Ok((TMUX_SNIPPET.to_string(), outcome));
    }

    let mut report = String::new();
    report.push_str(&format!("perch tmux config: {}\n", conf.display()));
    let conf_current = fs::read_to_string(&conf).unwrap_or_default();
    if !dry_run {
        if let Some(dir) = conf.parent() {
            fs::create_dir_all(dir)?;
        }
        write_atomic(&conf, TMUX_SNIPPET.as_bytes())?;
        report.push_str("written\n");
    } else {
        report.push_str("dry run: not written\n");
    }

    let existing = fs::read_to_string(&tmux_conf).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == source_line) {
        report.push_str(&format!("{} already sources it\n", tmux_conf.display()));
        outcome.already = conf_current == TMUX_SNIPPET;
        return Ok((report, outcome));
    }
    if apply && !dry_run {
        let backup = backup_path(&tmux_conf);
        if tmux_conf.exists() {
            fs::copy(&tmux_conf, &backup)?;
            report.push_str(&format!("backed up to {}\n", backup.display()));
            outcome.backup = Some(backup);
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
    Ok((report, outcome))
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
        assert!(TMUX_SNIPPET.contains("bind N run-shell 'perch next'"));
        assert!(TMUX_SNIPPET.contains("window-status-format ' #{@perch_flag}'"));
        assert!(TMUX_SNIPPET.contains("window-status-current-format ' #{@perch_flag}'"));
        assert!(TMUX_SNIPPET.contains("set -g focus-events on"));
        assert!(TMUX_SNIPPET.contains("pane-focus-in"));
    }
}
