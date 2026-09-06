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

/// `[notify]`. Unknown keys — the retired `desktop`, and the whole retired
/// `[toast]` table — are ignored rather than failing the parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Notify {
    pub enabled: bool,
    /// How long a card stays up, fades included.
    pub duration_ms: u64,
    /// Three tmux styles, dim to full, the `done` card fades through.
    pub done_style: Vec<String>,
    /// The same three for `needs_input`.
    pub needs_input_style: Vec<String>,
}

/// The built-in fade for a finished turn: green, dark to bright.
pub const DONE_STYLE: [&str; 3] = [
    "bg=colour235,fg=colour240",
    "bg=colour22,fg=colour250",
    "bg=colour28,fg=colour255,bold",
];

/// The built-in fade for a turn that wants you: red, dark to bright.
pub const NEEDS_INPUT_STYLE: [&str; 3] = [
    "bg=colour235,fg=colour240",
    "bg=colour88,fg=colour250",
    "bg=colour160,fg=colour255,bold",
];

impl Default for Notify {
    fn default() -> Self {
        Notify {
            enabled: true,
            duration_ms: 3500,
            done_style: Vec::new(),
            needs_input_style: Vec::new(),
        }
    }
}

impl Notify {
    pub fn done_styles(&self) -> [String; 3] {
        styles(&self.done_style, &DONE_STYLE)
    }

    pub fn needs_input_styles(&self) -> [String; 3] {
        styles(&self.needs_input_style, &NEEDS_INPUT_STYLE)
    }
}

/// An override of exactly three styles wins; anything else keeps the built-in,
/// because a half-configured fade is worse than none.
fn styles(over: &[String], built_in: &[&str; 3]) -> [String; 3] {
    if over.len() == 3 {
        return [over[0].clone(), over[1].clone(), over[2].clone()];
    }
    [
        built_in[0].to_string(),
        built_in[1].to_string(),
        built_in[2].to_string(),
    ]
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
        assert_eq!(c.notify.done_styles()[2], "bg=colour28,fg=colour255,bold");
        assert_eq!(
            c.notify.needs_input_styles()[2],
            "bg=colour160,fg=colour255,bold"
        );
    }

    /// Three styles or none: a two-entry override is a typo, not a fade.
    #[test]
    fn a_style_override_must_be_all_three() {
        let c: Config = toml::from_str(
            "[notify]\nenabled = false\nduration_ms = 900\ndone_style = [\"a\", \"b\", \"c\"]\nneeds_input_style = [\"x\"]\n",
        )
        .unwrap();
        assert!(!c.notify.enabled);
        assert_eq!(c.notify.duration_ms, 900);
        assert_eq!(c.notify.done_styles(), ["a", "b", "c"]);
        assert_eq!(c.notify.needs_input_styles()[0], DONE_STYLE[0]);
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
