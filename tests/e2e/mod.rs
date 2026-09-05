//! A real tmux server, a real attached client, and the real `perch` binary.
//!
//! Every test gets its own private server on its own socket
//! (`perch-e2e-<pid>-<n>` under a temporary `TMUX_TMPDIR`), killed on drop even
//! when the test panics. Nothing here can reach the user's default server: the
//! socket is unique, and the client is attached inside a pty, never on the
//! screen.
//!
//! ## How perch is told which server to talk to
//!
//! `src/tmux.rs` shells out to a bare `tmux` with no `-L`/`-S`, so the socket
//! cannot be passed as a flag. tmux itself resolves the socket from `$TMUX`
//! (`<socket path>,<server pid>,<session id>`) when no flag is given, which is
//! exactly the variable a pane already carries. So every perch child gets
//! `TMUX=<socket_path>,<server pid>,0` and its own `TMUX_PANE`, and the real
//! `RealTmux` path runs against the private server. `PERCH_NO_TMUX` is never
//! set — that would defeat the point of these tests.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize};

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub const PERCH_BIN: &str = env!("CARGO_BIN_EXE_perch");

/// `true` when a tmux binary is on `PATH`. CI without tmux skips, never fails.
pub fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `true` when the test must skip: tmux is not installed. Prints the reason,
/// so a skipped test says why instead of silently passing.
pub fn no_tmux() -> bool {
    if tmux_available() {
        return false;
    }
    eprintln!("skipping: tmux is not on PATH");
    true
}

/// A private tmux server plus the temp home/state/config perch runs against.
pub struct Server {
    pub label: String,
    pub socket_path: String,
    pub server_pid: String,
    /// Held so the directories outlive the server.
    _root: tempfile::TempDir,
    pub tmux_tmpdir: PathBuf,
    pub home: PathBuf,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
}

impl Server {
    /// Start a private server with one session (`one`) holding a `sleep` pane.
    pub fn start() -> Server {
        let root = tempfile::tempdir().expect("tempdir");
        let r = root.path();
        let tmux_tmpdir = r.join("tmux");
        let home = r.join("home");
        let state_dir = r.join("state");
        let config_dir = r.join("config");
        for d in [&tmux_tmpdir, &home, &state_dir, &config_dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        // The toast opens a popup on every attached client; these tests assert
        // on captured text, so it stays off.
        std::fs::write(config_dir.join("config.toml"), "[toast]\nenabled = false\n").unwrap();

        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let label = format!("perch-e2e-{}-{}", std::process::id(), n);

        let mut s = Server {
            label,
            socket_path: String::new(),
            server_pid: String::new(),
            _root: root,
            tmux_tmpdir,
            home,
            state_dir,
            config_dir,
        };

        let out = s.tmux(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "one",
            "-x",
            "120",
            "-y",
            "40",
            "sleep 100000",
        ]);
        assert!(
            out.status.success(),
            "new-session failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        s.socket_path = s.tmux_out(&["display", "-p", "#{socket_path}"]);
        s.server_pid = s.tmux_out(&["display", "-p", "#{pid}"]);
        assert!(!s.socket_path.is_empty() && !s.server_pid.is_empty());

        // New panes inherit the server's global environment, so anything run
        // inside a pane (the TUI, a harness) sees the same temp dirs.
        for (k, v) in s.perch_env() {
            s.tmux(&["set-environment", "-g", &k, &v]);
        }
        s
    }

    /// `TMUX` as a pane would carry it, pointing at this private server.
    pub fn tmux_env_value(&self) -> String {
        format!("{},{},0", self.socket_path, self.server_pid)
    }

    fn perch_env(&self) -> Vec<(String, String)> {
        vec![
            (
                "PERCH_STATE_DIR".into(),
                self.state_dir.display().to_string(),
            ),
            (
                "PERCH_CONFIG_DIR".into(),
                self.config_dir.display().to_string(),
            ),
            ("PERCH_HOME".into(), self.home.display().to_string()),
            ("PERCH_NO_SOUND".into(), "1".into()),
            ("TMUX".into(), self.tmux_env_value()),
            ("TMUX_TMPDIR".into(), self.tmux_tmpdir.display().to_string()),
        ]
    }

    /// Run a tmux command against this server. `-L <label>` is always passed.
    pub fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .arg("-L")
            .arg(&self.label)
            .args(args)
            .env("TMUX_TMPDIR", &self.tmux_tmpdir)
            // Never inherit an outer session's socket.
            .env_remove("TMUX")
            .output()
            .expect("spawn tmux")
    }

    /// Trimmed stdout of a tmux command; panics when it fails.
    pub fn tmux_out(&self, args: &[&str]) -> String {
        let out = self.tmux(args);
        assert!(
            out.status.success(),
            "tmux {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    /// Same, but an unsuccessful command yields an empty string.
    pub fn tmux_try(&self, args: &[&str]) -> String {
        let out = self.tmux(args);
        if !out.status.success() {
            return String::new();
        }
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    /// The first pane of a window, as `%id`.
    pub fn pane_of(&self, target: &str) -> String {
        self.tmux_out(&["display", "-p", "-t", target, "#{pane_id}"])
    }

    /// A new window running `sleep`, returning its pane id.
    pub fn new_window(&self, session: &str, name: &str) -> String {
        self.tmux_out(&[
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            session,
            "-n",
            name,
            "sleep 100000",
        ])
    }

    /// A new window running an interactive shell, so keys can be sent to it.
    pub fn new_shell_window(&self, session: &str, name: &str) -> String {
        self.tmux_out(&[
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            session,
            "-n",
            name,
            "/bin/sh",
        ])
    }

    pub fn new_session(&self, name: &str) -> String {
        self.tmux_out(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            name,
            "-x",
            "120",
            "-y",
            "40",
            "sleep 100000",
        ])
    }

    pub fn capture(&self, pane: &str) -> String {
        self.tmux_try(&["capture-pane", "-p", "-t", pane])
    }

    pub fn send_keys(&self, pane: &str, keys: &[&str]) {
        let mut args = vec!["send-keys", "-t", pane];
        args.extend_from_slice(keys);
        self.tmux(&args);
    }

    /// The pane the client is looking at right now.
    pub fn client_pane(&self, client: &str) -> String {
        self.tmux_try(&["display", "-p", "-c", client, "#{pane_id}"])
    }

    pub fn pane_option(&self, pane: &str, name: &str) -> String {
        self.tmux_try(&["show-options", "-pv", "-t", pane, name])
    }

    /// Run the real perch binary against this server.
    ///
    /// `pane` becomes `TMUX_PANE`, so hooks key on it exactly as a harness
    /// running in that pane would.
    pub fn perch(&self, args: &[&str]) -> PerchRun {
        PerchRun {
            args: args.iter().map(|s| s.to_string()).collect(),
            env: self.perch_env(),
            stdin: None,
        }
    }

    pub fn perch_in(&self, pane: &str, args: &[&str]) -> PerchRun {
        let mut r = self.perch(args);
        r.env.push(("TMUX_PANE".into(), pane.to_string()));
        r
    }

    /// `perch hook <harness>` with a fixture body, from `pane`.
    pub fn hook(&self, pane: &str, harness: &str, fixture_body: &str) {
        let out = self
            .perch_in(pane, &["hook", harness])
            .stdin(fixture_body)
            .run();
        assert!(out.ok, "hook exited non-zero: {}", out.stderr);
    }

    /// Every record from `perch list --json`.
    pub fn records(&self) -> serde_json::Value {
        let out = self.perch(&["list", "--json"]).run();
        assert!(out.ok, "list failed: {}", out.stderr);
        serde_json::from_str(&out.stdout).expect("list --json is JSON")
    }

    /// The recorded state of one pane, or `""` when it has no record.
    pub fn state_of(&self, pane: &str) -> String {
        self.records()
            .as_array()
            .map(|rs| {
                rs.iter()
                    .find(|r| r["pane"] == pane)
                    .and_then(|r| r["state"].as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default()
    }

    /// Attach a real client in a pty and wait for tmux to name it.
    pub fn attach(&self, session: &str) -> Client {
        Client::attach(self, session)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Runs on panic too: no private server outlives its test.
        let _ = Command::new("tmux")
            .arg("-L")
            .arg(&self.label)
            .arg("kill-server")
            .env("TMUX_TMPDIR", &self.tmux_tmpdir)
            .env_remove("TMUX")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// A configured, not-yet-spawned `perch` invocation.
pub struct PerchRun {
    args: Vec<String>,
    env: Vec<(String, String)>,
    stdin: Option<String>,
}

pub struct PerchOut {
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
}

impl PerchRun {
    pub fn stdin(mut self, body: &str) -> Self {
        self.stdin = Some(body.to_string());
        self
    }

    pub fn env(mut self, k: &str, v: &str) -> Self {
        self.env.push((k.to_string(), v.to_string()));
        self
    }

    pub fn run(self) -> PerchOut {
        let mut cmd = Command::new(PERCH_BIN);
        // CI sets PERCH_NO_TMUX=1 workflow-wide for the unit suite; these
        // tests exist to exercise the real tmux path, so it must not survive.
        cmd.env_remove("PERCH_NO_TMUX");
        cmd.args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn perch");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(self.stdin.unwrap_or_default().as_bytes())
            .ok();
        let out = child.wait_with_output().expect("wait perch");
        PerchOut {
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            ok: out.status.success(),
        }
    }
}

/// A real attached client, living in a pty for the length of the test.
pub struct Client {
    pub name: String,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Client {
    fn attach(server: &Server, session: &str) -> Client {
        let before = list_clients(server);
        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new("tmux");
        cmd.args(["-L", &server.label, "attach", "-t", session]);
        cmd.env("TMUX_TMPDIR", server.tmux_tmpdir.display().to_string());
        cmd.env("TERM", "xterm-256color");
        cmd.env_remove("TMUX");
        let portable_pty::PtyPair { slave, master } = pair;
        let child = slave.spawn_command(cmd).expect("attach");
        // The slave fd must go, or the pty never reports the client leaving.
        drop(slave);

        // Drain the master, or tmux blocks once the pty buffer fills.
        let mut reader = master.try_clone_reader().expect("reader");
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        });

        let mut name = String::new();
        wait_for(
            || {
                let now = list_clients(server);
                match now.iter().find(|c| !before.contains(c)) {
                    Some(c) => {
                        name = c.clone();
                        true
                    }
                    None => false,
                }
            },
            Duration::from_secs(3),
        );
        assert!(!name.is_empty(), "no client attached within 3s");

        Client {
            name,
            _master: master,
            child,
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn list_clients(server: &Server) -> Vec<String> {
    server
        .tmux_try(&["list-clients", "-F", "#{client_name}"])
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Poll `cond` every 50 ms until it holds or `timeout` elapses.
pub fn wait_for(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if cond() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Read a committed harness fixture.
pub fn fixture(harness: &str, name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(harness)
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Seed a pane record directly, for tests about navigation rather than hooks.
pub fn seed_record(state_dir: &Path, pane: &str, project: &str, state: &str, since: &str) {
    let dir = state_dir.join("panes");
    std::fs::create_dir_all(&dir).unwrap();
    let key: String = pane
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let body = serde_json::json!({
        "pane": pane,
        "harness": "claude",
        "state": state,
        "since": since,
        "project": project,
        "children": [],
    });
    std::fs::write(
        dir.join(format!("{key}.json")),
        serde_json::to_vec_pretty(&body).unwrap(),
    )
    .unwrap();
}
