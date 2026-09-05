use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub sounds: Sounds,
    /// How a transition is announced beyond the sound.
    pub notify: Notify,
    /// Per-pane sound cooldown, seconds.
    pub cooldown_secs: i64,
    /// Commands shown as `unknown` panes (reserved; no scraping in v1).
    pub watch_commands: Vec<String>,
}

/// The instant cue on `done` / `needs_input`.
#[derive(Debug, Clone, Serialize)]
pub struct Notify {
    /// Flash a one-line `tmux display-message` on every attached client.
    pub tmux_message: bool,
    /// How long that message stays up.
    pub duration_ms: u64,
    /// Also post a macOS notification via `osascript`.
    pub desktop: bool,
}

impl Default for Notify {
    fn default() -> Self {
        Notify {
            tmux_message: true,
            duration_ms: 4000,
            desktop: false,
        }
    }
}

/// Deserialised leniently: `notify = true`, the pre-`[notify]` spelling, still
/// means "post a desktop notification", so an old config keeps working instead
/// of failing to parse and losing every other setting with it.
impl<'de> Deserialize<'de> for Notify {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Table {
            tmux_message: Option<bool>,
            duration_ms: Option<u64>,
            desktop: Option<bool>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Desktop(bool),
            Table(Table),
        }
        let base = Notify::default();
        Ok(match Raw::deserialize(d)? {
            Raw::Desktop(desktop) => Notify { desktop, ..base },
            Raw::Table(t) => Notify {
                tmux_message: t.tmux_message.unwrap_or(base.tmux_message),
                duration_ms: t.duration_ms.unwrap_or(base.duration_ms),
                desktop: t.desktop.unwrap_or(base.desktop),
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Sounds {
    pub done: String,
    pub needs_input: String,
    pub error: String,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds {
            done: "Glass".into(),
            needs_input: "Ping".into(),
            error: "Basso".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            sounds: Sounds::default(),
            notify: Notify::default(),
            cooldown_secs: 3,
            watch_commands: Vec::new(),
        }
    }
}

impl Config {
    /// Sound name for an event key, or `None` when the key is unknown.
    pub fn sound_for(&self, event: &str) -> Option<&str> {
        match event {
            "done" => Some(&self.sounds.done),
            "needs_input" => Some(&self.sounds.needs_input),
            "error" => Some(&self.sounds.error),
            _ => None,
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("PERCH_CONFIG_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    crate::paths::home().join(".config/perch")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Load the config, falling back to defaults on a missing or broken file.
pub fn load() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_plan() {
        let c = Config::default();
        assert_eq!(c.sound_for("done"), Some("Glass"));
        assert_eq!(c.sound_for("needs_input"), Some("Ping"));
        assert_eq!(c.sound_for("error"), Some("Basso"));
        assert_eq!(c.sound_for("nope"), None);
        assert_eq!(c.cooldown_secs, 3);
    }

    #[test]
    fn notify_accepts_a_table_or_the_old_bool() {
        let c: Config = toml::from_str("[notify]\ndesktop = true\n").unwrap();
        assert!(c.notify.desktop);
        assert!(c.notify.tmux_message);
        assert_eq!(c.notify.duration_ms, 4000);

        let c: Config = toml::from_str("notify = true\ncooldown_secs = 9\n").unwrap();
        assert!(c.notify.desktop, "the old spelling still means desktop");
        assert_eq!(c.cooldown_secs, 9, "and the rest of the file survives");

        let c: Config =
            toml::from_str("[notify]\ntmux_message = false\nduration_ms = 100\n").unwrap();
        assert!(!c.notify.tmux_message);
        assert_eq!(c.notify.duration_ms, 100);
        assert!(!c.notify.desktop);
    }

    #[test]
    fn partial_toml_keeps_defaults() {
        let c: Config = toml::from_str("[sounds]\ndone = \"Hero\"\n").unwrap();
        assert_eq!(c.sounds.done, "Hero");
        assert_eq!(c.sounds.needs_input, "Ping");
    }
}
