use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub sounds: Sounds,
    /// Post a desktop notification via `osascript` alongside the sound.
    pub notify: bool,
    /// Per-pane sound cooldown, seconds.
    pub cooldown_secs: i64,
    /// Commands shown as `unknown` panes (reserved; no scraping in v1).
    pub watch_commands: Vec<String>,
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
            notify: false,
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
    fn partial_toml_keeps_defaults() {
        let c: Config = toml::from_str("[sounds]\ndone = \"Hero\"\n").unwrap();
        assert_eq!(c.sounds.done, "Hero");
        assert_eq!(c.sounds.needs_input, "Ping");
    }
}
