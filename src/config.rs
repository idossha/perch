use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub sounds: Sounds,
    /// The centered notification card.
    pub notify: Notify,
    /// Per-pane sound cooldown, seconds.
    pub cooldown_secs: i64,
    /// Commands shown as `unknown` panes (reserved; no scraping in v1).
    pub watch_commands: Vec<String>,
}

/// `[notify]`. Unknown keys — the retired `desktop`, the retired style
/// arrays, and the whole retired `[toast]` table — are ignored rather than
/// failing the parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Notify {
    pub enabled: bool,
    /// How long a card stays up, fades included.
    pub duration_ms: u64,
    /// Colour of `✓ done` on the card, a tmux/terminal colour name.
    pub accent_done: String,
    /// Colour of `⚑ needs input`.
    pub accent_needs_input: String,
    /// Colour of the card's rounded border.
    pub border: String,
}

/// The dashboard's `done` colour, so a card and a row agree.
pub const ACCENT_DONE: &str = "colour114";
/// The dashboard's `needs_input` colour.
pub const ACCENT_NEEDS_INPUT: &str = "colour203";
/// Grey enough to read as chrome rather than as a banner.
pub const BORDER: &str = "colour240";

impl Default for Notify {
    fn default() -> Self {
        Notify {
            enabled: true,
            duration_ms: 3500,
            accent_done: ACCENT_DONE.into(),
            accent_needs_input: ACCENT_NEEDS_INPUT.into(),
            border: BORDER.into(),
        }
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
        assert!(c.notify.enabled);
        assert_eq!(c.notify.duration_ms, 3500);
        assert_eq!(c.notify.accent_done, "colour114", "the dashboard's done");
        assert_eq!(c.notify.accent_needs_input, "colour203");
        assert_eq!(c.notify.border, "colour240");
    }

    /// The colours are three plain names, overridable one at a time.
    #[test]
    fn the_accents_and_the_border_are_overridable() {
        let c: Config = toml::from_str(
            "[notify]\nenabled = false\nduration_ms = 900\naccent_done = \"colour42\"\n",
        )
        .unwrap();
        assert!(!c.notify.enabled);
        assert_eq!(c.notify.duration_ms, 900);
        assert_eq!(c.notify.accent_done, "colour42");
        assert_eq!(
            c.notify.accent_needs_input, ACCENT_NEEDS_INPUT,
            "one override does not blank the others"
        );
        assert_eq!(c.notify.border, BORDER);
    }

    /// The retired style arrays still parse, and are ignored.
    #[test]
    fn the_old_style_arrays_are_ignored_not_fatal() {
        let c: Config = toml::from_str(
            "[notify]\ndone_style = [\"a\", \"b\", \"c\"]\nneeds_input_style = [\"x\"]\n",
        )
        .unwrap();
        assert!(c.notify.enabled);
        assert_eq!(c.notify.accent_done, ACCENT_DONE);
    }

    /// The retired `[toast]` table and the retired `[notify] desktop` key are
    /// ignored rather than failing the parse and taking the file with them.
    #[test]
    fn a_pre_sound_only_config_still_loads() {
        let c: Config = toml::from_str(
            "cooldown_secs = 9\n[toast]\nenabled = false\nduration_ms = 100\n[notify]\ndesktop = true\ntmux_message = false\n[sounds]\ndone = \"Hero\"\n",
        )
        .unwrap();
        assert!(c.notify.enabled, "an unknown key does not disable the card");
        assert_eq!(c.sounds.done, "Hero");
        assert_eq!(c.sounds.needs_input, "Ping");
        assert_eq!(c.cooldown_secs, 9);
    }

    #[test]
    fn partial_toml_keeps_defaults() {
        let c: Config = toml::from_str("[sounds]\ndone = \"Hero\"\n").unwrap();
        assert_eq!(c.sounds.done, "Hero");
        assert_eq!(c.sounds.needs_input, "Ping");
    }
}
