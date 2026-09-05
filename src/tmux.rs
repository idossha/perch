use std::process::{Command, Stdio};

/// One live tmux pane, as reported by `tmux list-panes -a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePane {
    pub pane: String,
    pub session: String,
    pub window: String,
    pub command: String,
    pub pid: Option<u32>,
}

/// Anything that can answer "which panes exist right now".
///
/// Injected so the store's liveness check is testable without a real server.
pub trait Tmux {
    fn list_panes(&self) -> Vec<LivePane>;
    fn set_pane_option(&self, _pane: &str, _name: &str, _value: &str) {}
    fn focus(&self, _pane: &str) {}
}

/// Disabled tmux, used when `PERCH_NO_TMUX=1` and in tests.
pub struct NullTmux {
    pub panes: Vec<LivePane>,
}

impl NullTmux {
    pub fn empty() -> Self {
        NullTmux { panes: Vec::new() }
    }
}

impl Tmux for NullTmux {
    fn list_panes(&self) -> Vec<LivePane> {
        self.panes.clone()
    }
}

/// Real tmux, shelling out best-effort; every failure degrades to "no info".
pub struct RealTmux;

impl Tmux for RealTmux {
    fn list_panes(&self) -> Vec<LivePane> {
        let out = Command::new("tmux")
            .args([
                "list-panes",
                "-a",
                "-F",
                "#{pane_id} #{session_name} #{window_index} #{pane_current_command} #{pane_pid}",
            ])
            .output();
        let Ok(out) = out else { return Vec::new() };
        if !out.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(parse_pane_line)
            .collect()
    }

    fn set_pane_option(&self, pane: &str, name: &str, value: &str) {
        let _ = Command::new("tmux")
            .args(["set-option", "-p", "-t", pane, name, value])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    fn focus(&self, pane: &str) {
        let _ = Command::new("tmux")
            .args(["select-window", "-t", pane])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("tmux")
            .args(["select-pane", "-t", pane])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

pub fn parse_pane_line(line: &str) -> Option<LivePane> {
    let mut it = line.split_whitespace();
    let pane = it.next()?.to_string();
    if !pane.starts_with('%') {
        return None;
    }
    Some(LivePane {
        pane,
        session: it.next().unwrap_or_default().to_string(),
        window: it.next().unwrap_or_default().to_string(),
        command: it.next().unwrap_or_default().to_string(),
        pid: it.next().and_then(|s| s.parse().ok()),
    })
}

/// `true` unless the user disabled tmux calls for this process.
pub fn enabled() -> bool {
    std::env::var("PERCH_NO_TMUX")
        .map(|v| v != "1")
        .unwrap_or(true)
}

/// The tmux implementation appropriate for this process.
pub fn current() -> Box<dyn Tmux> {
    if enabled() {
        Box::new(RealTmux)
    } else {
        Box::new(NullTmux::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_pane_line() {
        let p = parse_pane_line("%12 main 3 claude 4242").unwrap();
        assert_eq!(p.pane, "%12");
        assert_eq!(p.session, "main");
        assert_eq!(p.window, "3");
        assert_eq!(p.command, "claude");
        assert_eq!(p.pid, Some(4242));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_pane_line("").is_none());
        assert!(parse_pane_line("not-a-pane x").is_none());
    }
}
