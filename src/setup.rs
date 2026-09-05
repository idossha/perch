//! `perch setup`, `perch doctor` and `perch uninstall`.
//!
//! One command wires every harness that is actually installed on this machine,
//! one reports what is wired, and one reverses exactly what perch put there.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::install::{self, Outcome};
use crate::paths::Paths;
use crate::store;

/// Marker left in `~/.config/perch/setup.json` after a successful setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupMarker {
    pub version: String,
    pub timestamp: String,
    pub components: Vec<ComponentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentRecord {
    pub name: String,
    pub status: String,
    pub file: String,
    #[serde(default)]
    pub backup: Option<String>,
    #[serde(default)]
    pub created_events: Vec<String>,
    #[serde(default)]
    pub created_hooks_key: bool,
    #[serde(default)]
    pub created_file: bool,
}

/// One line of the setup table.
#[derive(Debug, Clone)]
pub struct Step {
    pub name: &'static str,
    pub status: Status,
    pub file: Option<PathBuf>,
    pub backup: Option<PathBuf>,
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Installed,
    Already,
    SkippedNotFound,
    DryRun,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Installed => "installed",
            Status::Already => "already",
            Status::SkippedNotFound => "skipped: not found",
            Status::DryRun => "dry run",
        }
    }
}

/// Options for `perch setup`.
#[derive(Debug, Clone, Default)]
pub struct SetupOpts {
    pub dry_run: bool,
    pub no_tmux: bool,
    /// Restrict to these component names; empty means all.
    pub only: Vec<String>,
}

impl SetupOpts {
    fn wants(&self, name: &str) -> bool {
        if name == "tmux" && self.no_tmux {
            return false;
        }
        self.only.is_empty() || self.only.iter().any(|o| o == name)
    }
}

fn contains_perch_hook(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|s| s.contains("perch hook"))
        .unwrap_or(false)
}

/// `true` when the pi extension perch installs is present.
fn pi_wired(paths: &Paths) -> bool {
    fs::read_to_string(paths.pi_extension())
        .map(|s| s.contains("perch"))
        .unwrap_or(false)
}

/// Harnesses that exist on this machine but have no perch hook yet.
pub fn unwired_harnesses(paths: &Paths) -> Vec<&'static str> {
    let mut out = Vec::new();
    if paths.claude_present() && !contains_perch_hook(&paths.claude_settings) {
        out.push("claude");
    }
    if paths.codex_present() && !contains_perch_hook(&paths.codex_hooks) {
        out.push("codex");
    }
    if paths.pi_present() && !pi_wired(paths) {
        out.push("pi");
    }
    out
}

/// `true` when setup has never completed and something is still unwired.
pub fn needs_nudge(paths: &Paths) -> bool {
    !paths.setup_marker().exists() && !unwired_harnesses(paths).is_empty()
}

/// Wire every detected harness plus tmux, and write the setup marker.
pub fn run(paths: &Paths, opts: &SetupOpts) -> Result<String> {
    let mut steps: Vec<Step> = Vec::new();
    let mut trust_report = String::new();

    let harnesses: [(&'static str, bool); 3] = [
        ("claude", paths.claude_present()),
        ("codex", paths.codex_present()),
        ("pi", paths.pi_present()),
    ];
    for (name, present) in harnesses {
        if !opts.wants(name) {
            continue;
        }
        if !present {
            steps.push(Step {
                name,
                status: Status::SkippedNotFound,
                file: None,
                backup: None,
                outcome: None,
            });
            continue;
        }
        let (_, outcome) = match name {
            "claude" => install::install_claude_at(&paths.claude_settings, opts.dry_run)?,
            "codex" => install::install_codex_at(&paths.codex_hooks, opts.dry_run)?,
            _ => install::install_pi_at(&paths.pi_ext_dir, opts.dry_run, false)?,
        };
        steps.push(step_from(name, outcome, opts.dry_run));
        if name == "codex" {
            let entries = install::codex_trust_entries(&paths.codex_hooks);
            trust_report = crate::trust::install(&paths.codex_config, &entries, opts.dry_run)
                .unwrap_or_else(|e| format!("trust   {e:#}\n"));
        }
    }

    if opts.wants("tmux") {
        let (_, outcome) = install::install_tmux_at(paths, opts.dry_run, true, false)?;
        let already = outcome.already;
        let mut step = step_from("tmux", outcome, opts.dry_run);
        if already && !opts.dry_run {
            step.status = Status::Already;
        }
        steps.push(step);
        if !opts.dry_run && std::env::var("TMUX").is_ok_and(|v| !v.is_empty()) {
            let _ = std::process::Command::new("tmux")
                .arg("source-file")
                .arg(paths.perch_tmux_conf())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    let mut out = table(&steps);
    out.push_str(&trust_report);
    if !opts.dry_run {
        write_marker(paths, &steps)?;
        let _ = writeln!(out, "\nmarker: {}", paths.setup_marker().display());
    } else {
        out.push_str("\ndry run: nothing was written\n");
    }
    Ok(out)
}

fn step_from(name: &'static str, outcome: Outcome, dry_run: bool) -> Step {
    let status = if dry_run {
        Status::DryRun
    } else if outcome.already {
        Status::Already
    } else {
        Status::Installed
    };
    Step {
        name,
        status,
        file: Some(outcome.file.clone()),
        backup: outcome.backup.clone(),
        outcome: Some(outcome),
    }
}

fn table(steps: &[Step]) -> String {
    let dash = "-".to_string();
    let cell = |p: &Option<PathBuf>| p.as_ref().map(|p| p.display().to_string());
    let rows: Vec<[String; 4]> = steps
        .iter()
        .map(|s| {
            [
                s.name.to_string(),
                s.status.as_str().to_string(),
                cell(&s.file).unwrap_or_else(|| dash.clone()),
                cell(&s.backup).unwrap_or_else(|| dash.clone()),
            ]
        })
        .collect();
    let header = [
        "component".to_string(),
        "status".to_string(),
        "file".to_string(),
        "backup".to_string(),
    ];
    let mut widths = [0usize; 4];
    for row in std::iter::once(&header).chain(rows.iter()) {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let mut out = String::new();
    for row in std::iter::once(&header).chain(rows.iter()) {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<w$}", w = widths[i]))
            .collect();
        let _ = writeln!(out, "{}", line.join("  ").trim_end());
    }
    out
}

fn write_marker(paths: &Paths, steps: &[Step]) -> Result<()> {
    let components = steps
        .iter()
        .map(|s| {
            let o = s.outcome.clone().unwrap_or_default();
            ComponentRecord {
                name: s.name.to_string(),
                status: s.status.as_str().to_string(),
                file: s
                    .file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                backup: s.backup.as_ref().map(|p| p.display().to_string()),
                created_events: o.created_events,
                created_hooks_key: o.created_hooks_key,
                created_file: o.created_file,
            }
        })
        .collect();
    let marker = SetupMarker {
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        components,
    };
    let path = paths.setup_marker();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, serde_json::to_string_pretty(&marker)? + "\n")?;
    Ok(())
}

pub fn read_marker(paths: &Paths) -> Option<SetupMarker> {
    serde_json::from_str(&fs::read_to_string(paths.setup_marker()).ok()?).ok()
}

// ---------------------------------------------------------------- doctor

#[derive(Debug, Clone, Serialize)]
pub struct HarnessReport {
    pub name: String,
    pub present: bool,
    pub hook: bool,
    pub file: String,
    /// codex only: whether every perch handler is recorded as trusted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Doctor {
    pub binary: String,
    pub version: String,
    pub harnesses: Vec<HarnessReport>,
    pub tmux_binding: bool,
    pub tmux_sourced: bool,
    pub tmux_conf: String,
    pub perch_tmux_conf: String,
    pub player: Option<String>,
    pub state_dir: String,
    pub records: usize,
    pub muted: bool,
    pub setup_marker: bool,
    /// Detected harnesses with no perch hook.
    pub unwired: Vec<String>,
}

impl Doctor {
    pub fn ok(&self) -> bool {
        self.unwired.is_empty()
    }
}

/// `true` when codex's config records every perch handler with the hash codex
/// will recompute at load time.
pub fn codex_trusted(paths: &Paths) -> bool {
    let entries = install::codex_trust_entries(&paths.codex_hooks);
    !entries.is_empty() && crate::trust::all_trusted(&paths.codex_config, &entries)
}

pub fn doctor(paths: &Paths) -> Doctor {
    let binary = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "perch".into());
    let perch_conf = paths.perch_tmux_conf();
    let source_line = format!("source-file {}", perch_conf.display());
    let tmux_binding = fs::read_to_string(&perch_conf)
        .map(|s| s.contains("perch tui"))
        .unwrap_or(false);
    let tmux_sourced = fs::read_to_string(&paths.tmux_conf)
        .map(|s| s.lines().any(|l| l.trim() == source_line))
        .unwrap_or(false);
    let player = ["afplay", "paplay"]
        .into_iter()
        .find(|p| crate::paths::on_path(p))
        .map(|s| s.to_string());
    let harnesses = vec![
        HarnessReport {
            name: "claude".into(),
            present: paths.claude_present(),
            hook: contains_perch_hook(&paths.claude_settings),
            file: paths.claude_settings.display().to_string(),
            trust: None,
        },
        HarnessReport {
            name: "codex".into(),
            present: paths.codex_present(),
            hook: contains_perch_hook(&paths.codex_hooks),
            file: paths.codex_hooks.display().to_string(),
            trust: Some(codex_trusted(paths)),
        },
        HarnessReport {
            name: "pi".into(),
            present: paths.pi_present(),
            hook: pi_wired(paths),
            file: paths.pi_extension().display().to_string(),
            trust: None,
        },
    ];
    let records = fs::read_dir(paths.state_dir.join("panes"))
        .map(|d| d.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    Doctor {
        binary,
        version: env!("CARGO_PKG_VERSION").to_string(),
        unwired: harnesses
            .iter()
            .filter(|h| h.present && !h.hook)
            .map(|h| h.name.clone())
            .collect(),
        harnesses,
        tmux_binding,
        tmux_sourced,
        tmux_conf: paths.tmux_conf.display().to_string(),
        perch_tmux_conf: perch_conf.display().to_string(),
        player,
        state_dir: paths.state_dir.display().to_string(),
        records,
        muted: store::mute_path().exists(),
        setup_marker: paths.setup_marker().exists(),
    }
}

pub fn doctor_text(d: &Doctor) -> String {
    let yn = |b: bool| if b { "yes" } else { "no" };
    let mut out = String::new();
    let _ = writeln!(out, "perch {} at {}", d.version, d.binary);
    for h in &d.harnesses {
        if h.present {
            let trust = match h.trust {
                Some(t) => format!("  trust: {:<3}", yn(t)),
                None => String::new(),
            };
            let _ = writeln!(
                out,
                "{:<7} found      hook: {:<3}{}  {}",
                h.name,
                yn(h.hook),
                trust,
                h.file
            );
        } else {
            let _ = writeln!(out, "{:<7} not found", h.name);
        }
    }
    let _ = writeln!(
        out,
        "tmux    binding: {}  sourced in {}: {}",
        yn(d.tmux_binding),
        d.tmux_conf,
        yn(d.tmux_sourced)
    );
    let _ = writeln!(
        out,
        "sound   player: {}",
        d.player.clone().unwrap_or_else(|| "none".into())
    );
    let _ = writeln!(out, "state   {} ({} records)", d.state_dir, d.records);
    let _ = writeln!(out, "mute    {}", yn(d.muted));
    let _ = writeln!(out, "setup   marker: {}", yn(d.setup_marker));
    if d.unwired.is_empty() {
        out.push_str("ok\n");
    } else {
        let _ = writeln!(out, "unwired: {}", d.unwired.join(", "));
    }
    out
}

// ------------------------------------------------------------- uninstall

/// Options for `perch uninstall`.
#[derive(Debug, Clone, Default)]
pub struct UninstallOpts {
    pub dry_run: bool,
    pub keep_state: bool,
}

/// Strip every hook group whose command mentions `perch hook`.
///
/// `created` names the event arrays perch itself created; only those are
/// removed when they end up empty, so an array the user had before stays.
pub fn strip_perch_hooks(
    mut settings: Value,
    created: &[String],
    created_hooks_key: bool,
) -> Value {
    let Some(root) = settings.as_object_mut() else {
        return settings;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return settings;
    };
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(arr) = hooks.get_mut(&event).and_then(|a| a.as_array_mut()) else {
            continue;
        };
        arr.retain(|g| {
            !g.get("hooks")
                .and_then(|h| h.as_array())
                .map(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains("perch hook"))
                    })
                })
                .unwrap_or(false)
        });
        if arr.is_empty() && created.iter().any(|e| e == &event) {
            hooks.remove(&event);
        }
    }
    if hooks.is_empty() && created_hooks_key {
        root.remove("hooks");
    }
    settings
}

fn component<'a>(marker: &'a Option<SetupMarker>, name: &str) -> Option<&'a ComponentRecord> {
    marker.as_ref()?.components.iter().find(|c| c.name == name)
}

fn unhook_file(
    path: &Path,
    rec: Option<&ComponentRecord>,
    opts: &UninstallOpts,
    out: &mut String,
) -> Result<()> {
    let Ok(body) = fs::read_to_string(path) else {
        let _ = writeln!(out, "{}: absent, nothing to do", path.display());
        return Ok(());
    };
    if !body.contains("perch hook") {
        let _ = writeln!(out, "{}: no perch entries", path.display());
        return Ok(());
    }
    let current: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
    // With no marker, an emptied array is assumed to be perch's own.
    let created: Vec<String> = match rec {
        Some(r) => r.created_events.clone(),
        None => current["hooks"]
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default(),
    };
    let created_hooks_key = rec.map(|r| r.created_hooks_key).unwrap_or(false);
    let stripped = strip_perch_hooks(current, &created, created_hooks_key);
    let new_body = serde_json::to_string_pretty(&stripped)? + "\n";
    if opts.dry_run {
        let _ = writeln!(out, "{}: would remove perch entries", path.display());
        return Ok(());
    }
    let backup = install::backup_path(path);
    fs::copy(path, &backup)?;
    fs::write(path, new_body)?;
    let _ = writeln!(
        out,
        "{}: perch entries removed (backup {})",
        path.display(),
        backup.display()
    );
    Ok(())
}

/// Reverse every perch installer.
pub fn uninstall(paths: &Paths, opts: &UninstallOpts) -> Result<String> {
    let marker = read_marker(paths);
    let mut out = String::new();

    unhook_file(
        &paths.claude_settings,
        component(&marker, "claude"),
        opts,
        &mut out,
    )?;
    let trust_keys: Vec<String> = install::codex_trust_entries(&paths.codex_hooks)
        .into_iter()
        .map(|e| e.key)
        .collect();
    out.push_str(&crate::trust::uninstall(
        &paths.codex_config,
        &trust_keys,
        opts.dry_run,
    )?);
    unhook_file(
        &paths.codex_hooks,
        component(&marker, "codex"),
        opts,
        &mut out,
    )?;

    // pi: the extension file is entirely ours.
    let pi = paths.pi_extension();
    if pi.exists() {
        if opts.dry_run {
            let _ = writeln!(out, "{}: would delete", pi.display());
        } else {
            fs::remove_file(&pi)?;
            let _ = writeln!(out, "{}: deleted", pi.display());
        }
    }

    // tmux: one source-file line, then our own conf.
    let perch_conf = paths.perch_tmux_conf();
    let source_line = format!("source-file {}", perch_conf.display());
    if let Ok(body) = fs::read_to_string(&paths.tmux_conf) {
        if body.lines().any(|l| l.trim() == source_line) {
            if opts.dry_run {
                let _ = writeln!(
                    out,
                    "{}: would remove the source-file line",
                    paths.tmux_conf.display()
                );
            } else {
                let backup = install::backup_path(&paths.tmux_conf);
                fs::copy(&paths.tmux_conf, &backup)?;
                let kept: Vec<&str> = body
                    .lines()
                    .filter(|l| l.trim() != source_line)
                    .collect::<Vec<_>>();
                let mut new_body = kept.join("\n");
                if !new_body.is_empty() {
                    new_body.push('\n');
                }
                fs::write(&paths.tmux_conf, new_body)?;
                let _ = writeln!(
                    out,
                    "{}: source-file line removed (backup {})",
                    paths.tmux_conf.display(),
                    backup.display()
                );
            }
        }
    }
    for path in [perch_conf, paths.setup_marker()] {
        if path.exists() {
            if opts.dry_run {
                let _ = writeln!(out, "{}: would delete", path.display());
            } else {
                fs::remove_file(&path)?;
                let _ = writeln!(out, "{}: deleted", path.display());
            }
        }
    }

    if !opts.keep_state && paths.state_dir.exists() {
        if opts.dry_run {
            let _ = writeln!(out, "{}: would delete", paths.state_dir.display());
        } else {
            fs::remove_dir_all(&paths.state_dir)?;
            let _ = writeln!(out, "{}: deleted", paths.state_dir.display());
        }
    }
    if opts.dry_run {
        out.push_str("dry run: nothing was written\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_keeps_other_entries_and_user_arrays() {
        let v = json!({
            "hooks": {
                "Stop": [
                    {"hooks": [{"type": "command", "command": "idosleep hook"}]},
                    {"hooks": [{"type": "command", "command": "perch hook claude"}]}
                ],
                "SessionEnd": [{"hooks": [{"type": "command", "command": "perch hook claude"}]}],
                "PreCompact": []
            },
            "model": "opus"
        });
        let out = strip_perch_hooks(v, &["SessionEnd".into()], false);
        assert_eq!(out["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(out["hooks"].get("SessionEnd").is_none());
        // Not created by perch: an empty array the user had stays put.
        assert!(out["hooks"].get("PreCompact").is_some());
        assert_eq!(out["model"], "opus");
    }

    #[test]
    fn status_strings_are_stable() {
        assert_eq!(Status::SkippedNotFound.as_str(), "skipped: not found");
        assert_eq!(Status::Already.as_str(), "already");
    }
}
