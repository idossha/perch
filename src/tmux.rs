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
    /// Every attached client, by `#{client_name}`.
    fn list_clients(&self) -> Vec<String> {
        Vec::new()
    }
    /// Run several tmux commands in one invocation, separated by `;`.
    ///
    /// The hook is on the harness's critical path, so the cue is one spawned
    /// process, never one per command.
    fn batch(&self, _cmds: &[Vec<String>]) {}

    fn set_pane_option(&self, pane: &str, name: &str, value: &str) {
        self.batch(&[vec![
            "set-option".into(),
            "-p".into(),
            "-t".into(),
            pane.into(),
            name.into(),
            value.into(),
        ]]);
    }

    /// Select a pane in the client's current session.
    fn focus(&self, pane: &str) {
        self.batch(&[
            vec!["select-window".into(), "-t".into(), pane.into()],
            vec!["select-pane".into(), "-t".into(), pane.into()],
        ]);
    }

    /// Move the calling client to a pane, switching session first.
    ///
    /// Everything is addressed by pane id, never by name, so a duplicate
    /// window or session name cannot send the client somewhere else.
    fn jump(&self, session: &str, pane: &str) {
        self.batch(&[
            vec!["switch-client".into(), "-t".into(), session.into()],
            vec!["select-window".into(), "-t".into(), pane.into()],
            vec!["select-pane".into(), "-t".into(), pane.into()],
        ]);
    }
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

    /// `PERCH_FAKE_CLIENTS` stands in for attached clients under `PERCH_NO_TMUX`.
    fn list_clients(&self) -> Vec<String> {
        std::env::var("PERCH_FAKE_CLIENTS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    /// With `PERCH_TMUX_LOG=<file>`, record what would have been run — one
    /// line per invocation — so the cue is testable without a tmux server.
    fn batch(&self, cmds: &[Vec<String>]) {
        let Ok(path) = std::env::var("PERCH_TMUX_LOG") else {
            return;
        };
        if path.is_empty() || cmds.is_empty() {
            return;
        }
        let line = cmds
            .iter()
            .map(|c| c.join(" "))
            .collect::<Vec<_>>()
            .join(" ; ");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
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

    fn list_clients(&self) -> Vec<String> {
        let out = Command::new("tmux")
            .args(["list-clients", "-F", "#{client_name}"])
            .output();
        let Ok(out) = out else { return Vec::new() };
        if !out.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    /// One `tmux a ; b ; c` invocation, spawned and never waited on.
    fn batch(&self, cmds: &[Vec<String>]) {
        if cmds.is_empty() {
            return;
        }
        let mut args: Vec<String> = Vec::new();
        for (i, cmd) in cmds.iter().enumerate() {
            if i > 0 {
                args.push(";".into());
            }
            args.extend(cmd.iter().cloned());
        }
        let _ = Command::new("tmux")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
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
    fn a_null_batch_records_one_line_per_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("tmux.log");
        std::env::set_var("PERCH_TMUX_LOG", &log);
        NullTmux::empty().jump("main", "%12");
        std::env::remove_var("PERCH_TMUX_LOG");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().trim(),
            "switch-client -t main ; select-window -t %12 ; select-pane -t %12"
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_pane_line("").is_none());
        assert!(parse_pane_line("not-a-pane x").is_none());
    }
}
