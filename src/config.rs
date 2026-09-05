use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub sounds: Sounds,
    /// How a transition is announced beyond the sound.
    pub notify: Notify,
    /// The bottom-right toast.
    pub toast: Toast,
    /// Per-pane sound cooldown, seconds.
    pub cooldown_secs: i64,
    /// Commands shown as `unknown` panes (reserved; no scraping in v1).
    pub watch_commands: Vec<String>,
}

/// The instant cue on `done` / `needs_input`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Notify {
    /// Post a macOS notification via `osascript`.
    pub desktop: bool,
}

/// The bottom-right toast drawn on every attached client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Toast {
    pub enabled: bool,
    /// How long it stays up, fade included.
    pub duration_ms: u64,
    /// `tmux display-popup -s` style for each kind.
    pub done_style: String,
    pub needs_input_style: String,
}

impl Default for Toast {
    fn default() -> Self {
        Toast {
            enabled: true,
            duration_ms: 3000,
            done_style: "bg=colour28,fg=colour255,bold".into(),
            needs_input_style: "bg=colour160,fg=colour255,bold".into(),
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
            Raw::Desktop(desktop) => Notify { desktop },
            Raw::Table(t) => Notify {
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
            toast: Toast::default(),
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

        let c: Config = toml::from_str("notify = true\ncooldown_secs = 9\n").unwrap();
        assert!(c.notify.desktop, "the old spelling still means desktop");
        assert_eq!(c.cooldown_secs, 9, "and the rest of the file survives");

        // The retired `[notify] tmux_message` / `duration_ms` keys are simply
        // ignored, rather than failing the parse and losing the whole file.
        let c: Config = toml::from_str(
            "[notify]\ntmux_message = false\nduration_ms = 100\n[sounds]\ndone = \"Hero\"\n",
        )
        .unwrap();
        assert!(!c.notify.desktop);
        assert_eq!(c.sounds.done, "Hero");
    }

    #[test]
    fn toast_defaults_and_overrides() {
        let c = Config::default();
        assert!(c.toast.enabled);
        assert_eq!(c.toast.duration_ms, 3000);
        assert_eq!(c.toast.done_style, "bg=colour28,fg=colour255,bold");

        let c: Config = toml::from_str("[toast]\nneeds_input_style = \"bg=blue\"\n").unwrap();
        assert_eq!(c.toast.needs_input_style, "bg=blue");
        assert_eq!(c.toast.duration_ms, 3000);
    }

    #[test]
    fn partial_toml_keeps_defaults() {
        let c: Config = toml::from_str("[sounds]\ndone = \"Hero\"\n").unwrap();
        assert_eq!(c.sounds.done, "Hero");
        assert_eq!(c.sounds.needs_input, "Ping");
    }
}
