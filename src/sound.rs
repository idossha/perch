use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::store;

/// `true` unless sound was disabled for this process or muted globally.
pub fn enabled() -> bool {
    if std::env::var("PERCH_NO_SOUND")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return false;
    }
    !store::mute_path().exists()
}

pub fn is_muted() -> bool {
    store::mute_path().exists()
}

/// Toggle the global mute file; returns the new muted state.
pub fn toggle_mute() -> bool {
    let p = store::mute_path();
    if p.exists() {
        let _ = std::fs::remove_file(&p);
        false
    } else {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, b"");
        true
    }
}

fn sound_file(name: &str) -> PathBuf {
    if name.contains('/') {
        return PathBuf::from(name);
    }
    PathBuf::from(format!("/System/Library/Sounds/{name}.aiff"))
}

/// Spawn a detached `afplay` for the configured sound of `event`.
///
/// Every failure is swallowed: a missing player must never break a hook.
pub fn play(cfg: &Config, event: &str) {
    if !enabled() {
        return;
    }
    let Some(name) = cfg.sound_for(event) else {
        return;
    };
    let path = sound_file(name);
    if !path.exists() {
        return;
    }
    let _ = Command::new("/usr/bin/afplay")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Optional desktop notification, best-effort.
pub fn notify(cfg: &Config, title: &str, body: &str) {
    if !cfg.notify || !enabled() {
        return;
    }
    let script = format!(
        "display notification {} with title {}",
        applescript_string(body),
        applescript_string(title)
    );
    let _ = Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_sound_resolves_under_system_sounds() {
        assert_eq!(
            sound_file("Glass"),
            PathBuf::from("/System/Library/Sounds/Glass.aiff")
        );
        assert_eq!(sound_file("/tmp/a.aiff"), PathBuf::from("/tmp/a.aiff"));
    }

    #[test]
    fn applescript_quotes_are_escaped() {
        assert_eq!(applescript_string("a\"b"), "\"a\\\"b\"");
    }
}
