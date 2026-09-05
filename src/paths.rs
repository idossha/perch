//! Every filesystem location perch touches, resolved once.
//!
//! Detection and the installers take a `Paths` so a test can point the whole
//! program at a temporary home without ever reading the real one.

use std::path::{Path, PathBuf};

/// The home directory perch resolves everything against.
///
/// `PERCH_HOME` wins so a test can redirect detection wholesale; otherwise the
/// real home, falling back to `.` when there is none.
pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("PERCH_HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `true` when `name` resolves to an executable on `PATH`.
///
/// `PERCH_NO_PATH_PROBE=1` turns the probe off, which is what the tests use so
/// a harness installed on the developer's machine cannot leak into a fake home.
pub fn on_path(name: &str) -> bool {
    if std::env::var("PERCH_NO_PATH_PROBE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return false;
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let p = dir.join(name);
        p.is_file() && is_executable(&p)
    })
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.exists()
}

/// Resolved locations for one run.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub claude_dir: PathBuf,
    pub claude_settings: PathBuf,
    pub codex_dir: PathBuf,
    pub codex_hooks: PathBuf,
    /// codex's own config, where hook trust records live.
    pub codex_config: PathBuf,
    pub pi_dir: PathBuf,
    pub pi_ext_dir: PathBuf,
    pub tmux_conf: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl Paths {
    /// Resolve from the environment: the `PERCH_*` overrides, then `PERCH_HOME`.
    pub fn from_env() -> Self {
        let home = home();
        Paths {
            claude_dir: home.join(".claude"),
            claude_settings: env_path("PERCH_CLAUDE_SETTINGS")
                .unwrap_or_else(|| home.join(".claude/settings.json")),
            codex_dir: home.join(".codex"),
            codex_hooks: env_path("PERCH_CODEX_HOOKS")
                .unwrap_or_else(|| home.join(".codex/hooks.json")),
            codex_config: env_path("PERCH_CODEX_CONFIG")
                .unwrap_or_else(|| home.join(".codex/config.toml")),
            pi_dir: home.join(".pi/agent"),
            pi_ext_dir: env_path("PERCH_PI_EXT_DIR")
                .unwrap_or_else(|| home.join(".pi/agent/extensions")),
            tmux_conf: env_path("PERCH_TMUX_CONF").unwrap_or_else(|| home.join(".tmux.conf")),
            config_dir: env_path("PERCH_CONFIG_DIR").unwrap_or_else(|| home.join(".config/perch")),
            state_dir: env_path("PERCH_STATE_DIR")
                .unwrap_or_else(|| home.join(".local/state/perch")),
            home,
        }
    }

    pub fn perch_tmux_conf(&self) -> PathBuf {
        self.config_dir.join("perch.tmux.conf")
    }

    pub fn setup_marker(&self) -> PathBuf {
        self.config_dir.join("setup.json")
    }

    pub fn pi_extension(&self) -> PathBuf {
        self.pi_ext_dir.join("perch.ts")
    }

    /// A harness counts as present when its config directory exists (an
    /// override pointing at an existing file counts too) or its binary is on
    /// `PATH`.
    pub fn claude_present(&self) -> bool {
        self.claude_dir.exists() || self.claude_settings.exists() || on_path("claude")
    }

    pub fn codex_present(&self) -> bool {
        self.codex_dir.exists() || self.codex_hooks.exists() || on_path("codex")
    }

    pub fn pi_present(&self) -> bool {
        self.pi_dir.exists() || self.pi_ext_dir.exists() || on_path("pi")
    }
}

impl Default for Paths {
    fn default() -> Self {
        Self::from_env()
    }
}
