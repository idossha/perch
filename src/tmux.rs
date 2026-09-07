use std::process::{Command, Stdio};

/// One live tmux pane, as reported by `tmux list-panes -a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePane {
    pub pane: String,
    pub session: String,
    /// `#{window_index}`, as a string: it is a label, never arithmetic.
    pub window: String,
    /// `#{window_name}` — what the user actually reads off the tmux top rail.
    pub window_name: String,
    pub pane_index: u32,
    /// How many panes that window holds; 1 means the index is not worth showing.
    pub window_panes: u32,
    pub session_attached: bool,
    pub command: String,
    pub pid: Option<u32>,
}

impl LivePane {
    /// The location as a user reads it off the top rail.
    ///
    /// `<window_name>`, with `<session>/` only when the board spans more than
    /// one session and `.<pane_index>` only when the window is split — the
    /// parts that disambiguate, and nothing else.
    pub fn location(&self, with_session: bool) -> String {
        let name = if self.window_name.is_empty() {
            self.window.clone()
        } else {
            self.window_name.clone()
        };
        let mut out = String::new();
        if with_session && !self.session.is_empty() {
            out.push_str(&self.session);
            out.push('/');
        }
        out.push_str(&name);
        if self.window_panes > 1 {
            out.push('.');
            out.push_str(&self.pane_index.to_string());
        }
        out
    }
}

/// One attached client, as far as "is anybody looking at this pane" goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientView {
    /// The client's *active* pane: what its screen is showing.
    pub pane: String,
    pub client: String,
    /// `#{client_flags}` contains `focused` — the terminal window itself has
    /// keyboard focus. A terminal that does not report focus never sets it.
    pub focused: bool,
}

/// A client that is showing one particular pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub client: String,
    pub focused: bool,
}

/// Anything that can answer "which panes exist right now".
///
/// Injected so the store's liveness check is testable without a real server.
pub trait Tmux {
    fn list_panes(&self) -> Vec<LivePane>;
    /// Every attached client: which pane it shows, its name, its focus flag.
    ///
    /// One `list-clients`. This is the whole input to the seen rule, so it is
    /// fetched once per decision and never per record.
    fn client_views(&self) -> Vec<ClientView> {
        Vec::new()
    }

    /// Every attached client currently showing `pane`.
    fn viewers(&self, pane: &str) -> Vec<Viewer> {
        viewers_of(&self.client_views(), pane)
    }

    /// `true` when a focused client is showing this pane right now — see
    /// [`pane_is_seen`]. One `list-clients`.
    fn pane_seen_now(&self, pane: &str) -> bool {
        let views = self.client_views();
        pane_is_seen(&viewers_of(&views, pane), any_focus_info(&views))
    }

    /// The stored location of a pane — see [`parse_location_line`].
    fn pane_location(&self, _pane: &str) -> Option<String> {
        None
    }

    /// A client's terminal size, columns by rows: what a popup on it can be
    /// at most.
    fn client_size(&self, _client: &str) -> Option<(u16, u16)> {
        None
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

    /// `true` when this pane id is in the live pane list.
    fn pane_exists(&self, pane: &str) -> bool {
        self.list_panes().iter().any(|p| p.pane == pane)
    }

    /// Move one named client to a pane: the only jump primitive perch has.
    ///
    /// `switch-client -c <client> -t <pane_id>` changes that client's session,
    /// window and pane in one atomic call. Nothing else is used: a command
    /// without `-c` picks a "current client" by heuristic (tty match, else most
    /// recent activity), which is exactly how a jump made from a popup's pty,
    /// or with two clients attached, sends the user to the wrong place.
    ///
    /// Returns whether tmux accepted it.
    fn jump(&self, client: &str, pane: &str) -> bool {
        let argv: Vec<String> = vec![
            "switch-client".into(),
            "-c".into(),
            client.into(),
            "-t".into(),
            pane.into(),
        ];
        if debug() {
            eprintln!("perch: tmux {}", argv.join(" "));
        }
        self.run_checked(&[argv])
    }

    /// Like [`Tmux::run`], reporting whether tmux exited zero.
    fn run_checked(&self, cmds: &[Vec<String>]) -> bool {
        self.run(cmds);
        true
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

    /// The location comes from the injected pane list.
    fn pane_location(&self, pane: &str) -> Option<String> {
        self.panes
            .iter()
            .find(|p| p.pane == pane)
            .map(|p| p.location(false))
    }

    /// `PERCH_FAKE_CLIENT_SIZE="200x50"` stands in for every client's size.
    fn client_size(&self, _client: &str) -> Option<(u16, u16)> {
        let v = std::env::var("PERCH_FAKE_CLIENT_SIZE").ok()?;
        let (w, h) = v.trim().split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }

    /// `PERCH_FAKE_VIEWERS="%1:focused,%2"` stands in for `list-clients`:
    /// a comma-separated list of the pane each client shows, `:focused` when
    /// that client carries tmux's focus flag.
    fn client_views(&self) -> Vec<ClientView> {
        std::env::var("PERCH_FAKE_VIEWERS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, spec)| {
                let (pane, flags) = spec.split_once(':').unwrap_or((spec, ""));
                ClientView {
                    pane: pane.to_string(),
                    client: format!("/dev/fake{i}"),
                    focused: flags.split_whitespace().any(|f| f == "focused"),
                }
            })
            .collect()
    }

    /// `PERCH_FAKE_JUMP_FAIL=1` makes every checked command report failure.
    fn run_checked(&self, cmds: &[Vec<String>]) -> bool {
        self.run(cmds);
        !std::env::var("PERCH_FAKE_JUMP_FAIL").is_ok_and(|v| v == "1")
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
                // `#{window_name}` is last on purpose: it is the one field a
                // user can put spaces in, so it takes the rest of the line.
                PANE_FORMAT,
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

    fn client_views(&self) -> Vec<ClientView> {
        let out = Command::new("tmux")
            .args(["list-clients", "-F", CLIENT_FORMAT])
            .output();
        let Ok(out) = out else { return Vec::new() };
        if !out.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(parse_client_line)
            .collect()
    }

    /// One `display -p` for the client's size.
    fn client_size(&self, client: &str) -> Option<(u16, u16)> {
        let out = Command::new("tmux")
            .args([
                "display",
                "-p",
                "-c",
                client,
                "#{client_width} #{client_height}",
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let body = String::from_utf8_lossy(&out.stdout);
        let mut it = body.split_whitespace();
        Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
    }

    /// One `display -p` for the location fields.
    fn pane_location(&self, pane: &str) -> Option<String> {
        let out = Command::new("tmux")
            .args(["display", "-p", "-t", pane, LOCATION_FORMAT])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let body = String::from_utf8_lossy(&out.stdout);
        parse_location_line(body.lines().next().unwrap_or(""))
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
        let _ = self.run_checked(cmds);
    }

    fn run_checked(&self, cmds: &[Vec<String>]) -> bool {
        if cmds.is_empty() {
            return true;
        }
        Command::new("tmux")
            .args(joined(cmds))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
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

/// The `list-panes` format. `#{window_name}` trails so it may contain spaces.
pub const PANE_FORMAT: &str = "#{pane_id} #{session_name} #{window_index} \
#{pane_index} #{window_panes} #{session_attached} #{pane_current_command} \
#{pane_pid} #{window_name}";

/// The location format the hook asks for, so an ended pane still remembers
/// where it was: `<session> <window_panes> <pane_index> <window name>`.
pub const LOCATION_FORMAT: &str = "#{session_name} #{window_panes} #{pane_index} #{window_name}";

/// The `list-clients` format behind [`Tmux::client_views`]. Tab-separated: a
/// client name is a tty path, and flags never contain a tab.
pub const CLIENT_FORMAT: &str = "#{pane_id}\t#{client_name}\t#{client_flags}";

pub fn parse_client_line(line: &str) -> Option<ClientView> {
    let mut it = line.splitn(3, '\t');
    let pane = it.next()?.trim().to_string();
    if !pane.starts_with('%') {
        return None;
    }
    let client = it.next().unwrap_or_default().trim().to_string();
    let flags = it.next().unwrap_or_default();
    Some(ClientView {
        focused: flags.split(',').any(|f| f.trim() == "focused"),
        pane,
        client,
    })
}

/// `true` when any client on the server reports tmux's `focused` flag.
///
/// When nothing does, this server has no focus information at all — the
/// terminal never sends focus-in — and the rule has to fall back.
pub fn any_focus_info(views: &[ClientView]) -> bool {
    views.iter().any(|c| c.focused)
}

/// The clients showing one pane.
pub fn viewers_of(views: &[ClientView], pane: &str) -> Vec<Viewer> {
    views
        .iter()
        .filter(|c| c.pane == pane)
        .map(|c| Viewer {
            client: c.client.clone(),
            focused: c.focused,
        })
        .collect()
}

/// **The seen rule.** A pane is seen when a *focused* client is showing it.
///
/// `any_focus_info` says whether the server knows about focus at all. When no
/// client anywhere carries the flag — a terminal that does not report focus,
/// or a client attached from a pty — the flag carries no information, and any
/// viewer counts. Otherwise an unfocused viewer does not: the user has that
/// pane on screen but is looking at their browser.
pub fn pane_is_seen(viewers: &[Viewer], any_focus_info: bool) -> bool {
    if viewers.iter().any(|v| v.focused) {
        return true;
    }
    !any_focus_info && !viewers.is_empty()
}

pub fn parse_pane_line(line: &str) -> Option<LivePane> {
    // Eight fixed fields, then the window name as the remainder.
    let mut it = line.splitn(9, ' ');
    let pane = it.next()?.to_string();
    if !pane.starts_with('%') {
        return None;
    }
    let mut next = || it.next().unwrap_or_default();
    Some(LivePane {
        pane,
        session: next().to_string(),
        window: next().to_string(),
        pane_index: next().parse().unwrap_or(0),
        window_panes: next().parse().unwrap_or(1),
        session_attached: next() != "0",
        command: next().to_string(),
        pid: next().parse().ok(),
        window_name: next().trim().to_string(),
    })
}

/// Turn one `LOCATION_FORMAT` line into the string stored on a record.
///
/// The session prefix is left off: it is contextual — the board only shows it
/// when more than one session is on screen — and cannot be judged from here.
pub fn parse_location_line(line: &str) -> Option<String> {
    let mut it = line.trim_end().splitn(4, ' ');
    let _session = it.next()?;
    let panes: u32 = it.next()?.parse().unwrap_or(1);
    let index = it.next()?;
    let name = it.next().unwrap_or("").trim();
    if name.is_empty() {
        return None;
    }
    Some(if panes > 1 {
        format!("{name}.{index}")
    } else {
        name.to_string()
    })
}

/// `true` with `PERCH_DEBUG=1`: jumps then log their exact tmux argv.
pub fn debug() -> bool {
    std::env::var("PERCH_DEBUG").is_ok_and(|v| v == "1")
}

/// The client every client-moving command must name.
///
/// `--client` is the truth; a launcher (`bind g run-shell 'perch open --client
/// "#{client_name}"'`) always has it, because `run-shell` and key bindings
/// expand `#{…}` formats. `display-popup` does *not* expand them in its
/// shell-command, so a popup can only know its client because the launcher
/// passed it in. Falling back to `display -p '#{client_name}'` is a guess when
/// more than one client is attached, so it warns.
pub fn resolve_client(explicit: Option<&str>) -> Option<String> {
    if let Some(c) = explicit.map(str::trim).filter(|c| !c.is_empty()) {
        return Some(c.to_string());
    }
    eprintln!("perch: no --client given; guessing the current client");
    let out = Command::new("tmux")
        .args(["display", "-p", "#{client_name}"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
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
        let p = parse_pane_line("%12 main 3 0 1 1 claude 4242 editor").unwrap();
        assert_eq!(p.pane, "%12");
        assert_eq!(p.session, "main");
        assert_eq!(p.window, "3");
        assert_eq!(p.pane_index, 0);
        assert_eq!(p.window_panes, 1);
        assert!(p.session_attached);
        assert_eq!(p.command, "claude");
        assert_eq!(p.pid, Some(4242));
        assert_eq!(p.window_name, "editor");
    }

    /// A window name may contain spaces, so it is the rest of the line.
    #[test]
    fn a_window_name_may_contain_spaces() {
        let p = parse_pane_line("%1 main 0 2 3 0 zsh 9 my long name").unwrap();
        assert_eq!(p.window_name, "my long name");
        assert_eq!(p.pane_index, 2);
        assert_eq!(p.window_panes, 3);
        assert!(!p.session_attached);
    }

    #[test]
    fn a_location_names_the_window_and_only_disambiguates_when_it_must() {
        let mut p = parse_pane_line("%1 main 0 2 1 1 zsh 9 editor").unwrap();
        assert_eq!(p.location(false), "editor");
        assert_eq!(p.location(true), "main/editor");
        p.window_panes = 4;
        assert_eq!(p.location(false), "editor.2");
        // An unnamed window falls back to its index.
        p.window_name = String::new();
        assert_eq!(p.location(false), "0.2");
    }

    #[test]
    fn a_stored_location_drops_the_contextual_session_prefix() {
        assert_eq!(
            parse_location_line("main 1 0 editor").as_deref(),
            Some("editor")
        );
        assert_eq!(
            parse_location_line("main 2 1 my editor").as_deref(),
            Some("my editor.1")
        );
        assert_eq!(parse_location_line("main 1 0 "), None);
        assert_eq!(parse_location_line(""), None);
    }

    #[test]
    fn a_null_batch_records_one_line_per_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("tmux.log");
        std::env::set_var("PERCH_TMUX_LOG", &log);
        NullTmux::empty().jump("/dev/ttys003", "%12");
        std::env::remove_var("PERCH_TMUX_LOG");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().trim(),
            "switch-client -c /dev/ttys003 -t %12"
        );
    }

    /// A jump must go through the synchronous, checked `run_checked`, never
    /// the fire-and-forget `batch`: the TUI's popup dies the instant it
    /// returns, taking any un-waited child with it.
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
    fn the_only_jump_primitive_is_switch_client_with_an_explicit_client() {
        let t = RunOnly(Default::default());
        assert!(t.jump("work", "%7"));
        assert_eq!(
            t.0.into_inner(),
            vec!["switch-client -c work -t %7".to_string()],
            "no select-window, no select-pane, no session name"
        );
    }

    #[test]
    fn a_failed_jump_is_reported() {
        std::env::set_var("PERCH_FAKE_JUMP_FAIL", "1");
        let ok = NullTmux::empty().jump("c", "%1");
        std::env::remove_var("PERCH_FAKE_JUMP_FAIL");
        assert!(!ok);
    }

    #[test]
    fn an_explicit_client_wins_without_asking_tmux() {
        assert_eq!(
            resolve_client(Some("/dev/ttys009")).as_deref(),
            Some("/dev/ttys009")
        );
    }

    #[test]
    fn pane_existence_comes_from_the_live_list() {
        let t = NullTmux {
            panes: vec![parse_pane_line("%3 a 0 0 1 1 claude 7 shell").unwrap()],
        };
        assert!(t.pane_exists("%3"));
        assert!(!t.pane_exists("%4"));
    }

    fn v(focused: bool) -> Viewer {
        Viewer {
            client: "c".into(),
            focused,
        }
    }

    /// The seen rule, all four cases.
    #[test]
    fn a_pane_is_seen_when_a_focused_client_is_showing_it() {
        // A focused viewer: seen, whatever else is attached.
        assert!(pane_is_seen(&[v(true)], true));
        assert!(pane_is_seen(&[v(false), v(true)], true));
        // An unfocused viewer while the server does know about focus: the pane
        // is on screen but the user is in their browser. Not seen.
        assert!(!pane_is_seen(&[v(false)], true));
        // No client anywhere reports focus: the flag says nothing, so any
        // viewer counts.
        assert!(pane_is_seen(&[v(false)], false));
        // Nobody is showing it at all.
        assert!(!pane_is_seen(&[], false));
        assert!(!pane_is_seen(&[], true));
    }

    #[test]
    fn client_lines_carry_the_pane_the_client_shows() {
        let a = parse_client_line("%3\t/dev/ttys001\tattached,focused,UTF-8").unwrap();
        assert_eq!(
            (a.pane.as_str(), a.client.as_str(), a.focused),
            ("%3", "/dev/ttys001", true)
        );
        let b = parse_client_line("%4\t/dev/ttys002\tattached,UTF-8").unwrap();
        assert!(!b.focused);
        assert!(parse_client_line("").is_none());
        assert!(parse_client_line("nope\tx\ty").is_none());

        let views = vec![a, b];
        assert!(any_focus_info(&views));
        assert_eq!(viewers_of(&views, "%3").len(), 1);
        assert!(viewers_of(&views, "%9").is_empty());
    }

    #[test]
    fn fake_viewers_stand_in_for_list_clients() {
        std::env::set_var("PERCH_FAKE_VIEWERS", "%1:focused,%2");
        let t = NullTmux::empty();
        assert!(t.pane_seen_now("%1"));
        assert!(
            !t.pane_seen_now("%2"),
            "a real focus flag exists, %2 has none"
        );
        assert!(!t.pane_seen_now("%3"));
        std::env::set_var("PERCH_FAKE_VIEWERS", "%2");
        assert!(
            NullTmux::empty().pane_seen_now("%2"),
            "no focus info anywhere"
        );
        std::env::remove_var("PERCH_FAKE_VIEWERS");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_pane_line("").is_none());
        assert!(parse_pane_line("not-a-pane x").is_none());
    }
}
