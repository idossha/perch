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
    /// `true` when this pane is the active pane of an attached client — the
    /// user is looking at it right now.
    fn pane_focused(&self, _pane: &str) -> bool {
        false
    }

    /// Run several tmux commands in one invocation, separated by `;`.
    ///
    /// The hook is on the harness's critical path, so the cue is one spawned
    /// process, never one per command.
    fn batch(&self, _cmds: &[Vec<String>]) {}

    /// Run several tmux commands in one invocation and *wait* for them.
    ///
    /// Anything that moves the client must go through here: the TUI runs
    /// inside a `display-popup`, and the moment it returns the popup's pty is
    /// gone and a merely-spawned child is killed before tmux ever reads it.
    fn run(&self, cmds: &[Vec<String>]) {
        self.batch(cmds);
    }

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
        self.run(&[
            vec!["select-window".into(), "-t".into(), pane.into()],
            vec!["select-pane".into(), "-t".into(), pane.into()],
        ]);
    }

    /// Move the calling client to a pane, switching session first.
    ///
    /// Everything is addressed by pane id, never by name, so a duplicate
    /// window or session name cannot send the client somewhere else.
    fn jump(&self, session: &str, pane: &str) {
        self.run(&[
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

    /// `PERCH_FAKE_PANE_FOCUSED=1` stands in for a focused pane in tests.
    fn pane_focused(&self, _pane: &str) -> bool {
        std::env::var("PERCH_FAKE_PANE_FOCUSED").is_ok_and(|v| v == "1")
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

    fn pane_focused(&self, pane: &str) -> bool {
        let out = Command::new("tmux")
            .args([
                "display",
                "-p",
                "-t",
                pane,
                "#{pane_active}#{window_active}#{session_attached}",
            ])
            .output();
        let Ok(out) = out else { return false };
        out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "111"
    }

    /// One `tmux a ; b ; c` invocation, spawned and never waited on.
    fn batch(&self, cmds: &[Vec<String>]) {
        if cmds.is_empty() {
            return;
        }
        let _ = Command::new("tmux")
            .args(joined(cmds))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    /// The same invocation, waited on — see `Tmux::run`.
    fn run(&self, cmds: &[Vec<String>]) {
        if cmds.is_empty() {
            return;
        }
        let _ = Command::new("tmux")
            .args(joined(cmds))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// `[[a, b], [c]]` -> `a b ; c`, the argv of one multi-command `tmux` call.
fn joined(cmds: &[Vec<String>]) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            args.push(";".into());
        }
        args.extend(cmd.iter().cloned());
    }
    args
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

    /// `focus` and `jump` must go through the synchronous `run`, never the
    /// fire-and-forget `batch`: the TUI's popup dies the instant it returns.
    struct RunOnly(std::cell::RefCell<Vec<String>>);

    impl Tmux for RunOnly {
        fn list_panes(&self) -> Vec<LivePane> {
            Vec::new()
        }
        fn batch(&self, _cmds: &[Vec<String>]) {
            panic!("a client move must not be spawned and abandoned");
        }
        fn run(&self, cmds: &[Vec<String>]) {
            self.0.borrow_mut().push(
                cmds.iter()
                    .map(|c| c.join(" "))
                    .collect::<Vec<_>>()
                    .join(" ; "),
            );
        }
    }

    #[test]
    fn moving_the_client_is_synchronous() {
        let t = RunOnly(Default::default());
        t.jump("work", "%7");
        t.focus("%7");
        assert_eq!(
            t.0.into_inner(),
            vec![
                "switch-client -t work ; select-window -t %7 ; select-pane -t %7".to_string(),
                "select-window -t %7 ; select-pane -t %7".to_string(),
            ]
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_pane_line("").is_none());
        assert!(parse_pane_line("not-a-pane x").is_none());
    }
}
