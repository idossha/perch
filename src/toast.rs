//! The bottom-right toast: a borderless one-line `display-popup` per client.
//!
//! A window-status flag put perch's state into the user's window list forever;
//! a toast says the same thing, in the corner, and then goes away. The first
//! keystroke dismisses it *and* is forwarded to the pane the user was typing
//! at, so a toast can never eat a character.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthStr;

use crate::config::Config;
use crate::tmux::Tmux;

/// The widest a toast may get, borders and padding included.
pub const MAX_WIDTH: usize = 60;
/// Padding around the text: two spaces each side, plus the glyph and a space.
const CHROME: usize = 4;
/// How long the fade at the end of a toast takes.
const FADE_MS: u64 = 600;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    Done,
    NeedsInput,
    /// A sample, so a user can check placement without waiting for an agent.
    Test,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "done" => Some(Kind::Done),
            "needs_input" | "needs-input" => Some(Kind::NeedsInput),
            "test" => Some(Kind::Test),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Done => "done",
            Kind::NeedsInput => "needs_input",
            Kind::Test => "test",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Kind::NeedsInput => "⚑",
            _ => "✓",
        }
    }

    /// The style this kind starts at, before the fade.
    pub fn style(self, cfg: &Config) -> String {
        match self {
            Kind::NeedsInput => cfg.toast.needs_input_style.clone(),
            _ => cfg.toast.done_style.clone(),
        }
    }

    /// The two dimmer styles the toast fades through on its way out.
    pub fn fade(self) -> [&'static str; 2] {
        match self {
            Kind::NeedsInput => ["bg=colour88,fg=colour250", "bg=colour235,fg=colour240"],
            _ => ["bg=colour22,fg=colour250", "bg=colour235,fg=colour240"],
        }
    }
}

/// The state a kind maps to a toast for, or `None` for one not worth a toast.
pub fn kind_for(state: crate::model::State) -> Option<Kind> {
    match state {
        crate::model::State::Done => Some(Kind::Done),
        crate::model::State::NeedsInput => Some(Kind::NeedsInput),
        _ => None,
    }
}

/// `<text>` trimmed to fit `MAX_WIDTH`, with an ellipsis when it was cut.
pub fn ellipsize(text: &str) -> String {
    let budget = MAX_WIDTH - CHROME;
    if text.width() <= budget {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + c.to_string().width() > budget - 1 {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// The popup width for a line of text: its display width plus the chrome,
/// never wider than `MAX_WIDTH`.
pub fn width_for(text: &str) -> usize {
    (text.width() + CHROME).min(MAX_WIDTH)
}

/// The `display-popup` argv that draws one toast on one client.
///
/// `-B` drops the border, `-x R -y P` pins it to the bottom-right corner of
/// that client, and `-E` closes the popup when the body exits.
pub fn launch_command(
    exe: &str,
    client: &str,
    kind: Kind,
    duration_ms: u64,
    text: &str,
    style: &str,
) -> Vec<String> {
    let text = ellipsize(text);
    vec![
        "display-popup".into(),
        "-c".into(),
        client.into(),
        "-B".into(),
        "-E".into(),
        "-x".into(),
        "R".into(),
        "-y".into(),
        "P".into(),
        "-w".into(),
        width_for(&text).to_string(),
        "-h".into(),
        "1".into(),
        "-s".into(),
        style.into(),
        "--".into(),
        exe.into(),
        "toast-body".into(),
        kind.as_str().into(),
        duration_ms.to_string(),
        text,
        "--client".into(),
        client.into(),
    ]
}

/// Draw a toast on every attached client.
pub fn show(t: &dyn Tmux, cfg: &Config, kind: Kind, text: &str) {
    if !cfg.toast.enabled && kind != Kind::Test {
        return;
    }
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "perch".into());
    let style = kind.style(cfg);
    for client in t.list_clients() {
        // One spawned `tmux display-popup` per client: the popup is the
        // caller's to wait on, never the hook's.
        t.batch(&[launch_command(
            &exe,
            &client,
            kind,
            cfg.toast.duration_ms,
            text,
            &style,
        )]);
    }
}

/// Ask a detached `perch toast` to draw the toast, off the hook's critical
/// path entirely.
///
/// Under `PERCH_NO_TMUX` nothing is spawned; the request is recorded in
/// `PERCH_TMUX_LOG` so the hook's behaviour is testable.
pub fn spawn_detached(kind: Kind, text: &str) {
    if !crate::tmux::enabled() {
        crate::tmux::NullTmux::empty().batch(&[vec![
            "toast".into(),
            kind.as_str().into(),
            text.into(),
        ]]);
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .args(["toast", kind.as_str(), text])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// The active pane of a client, which is where a forwarded keystroke goes.
pub fn active_pane(client: &str) -> Option<String> {
    let out = Command::new("tmux")
        .args(["display", "-p", "-c", client, "#{pane_id}"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let pane = String::from_utf8_lossy(&out.stdout).trim().to_string();
    pane.starts_with('%').then_some(pane)
}

/// Read once; if the user typed, send those bytes verbatim to `pane`.
///
/// `-l` means literal, and `--` keeps a leading `-` from being read as a flag,
/// so the keystroke that dismissed the toast still lands in the agent.
pub fn forward_key<R: Read>(r: &mut R, t: &dyn Tmux, pane: &str) -> bool {
    let mut buf = [0u8; 512];
    match r.read(&mut buf) {
        Ok(n) if n > 0 => {
            t.run(&[vec![
                "send-keys".into(),
                "-t".into(),
                pane.into(),
                "-l".into(),
                "--".into(),
                String::from_utf8_lossy(&buf[..n]).into_owned(),
            ]]);
            true
        }
        _ => false,
    }
}

/// Restyle the popup this process is running inside — the fade.
fn restyle(t: &dyn Tmux, style: &str) {
    t.batch(&[vec!["display-popup".into(), "-s".into(), style.into()]]);
}

/// The popup's body: print the line, then wait out the duration, forwarding
/// the first keystroke and exiting early if one arrives.
pub fn body(kind: Kind, duration_ms: u64, text: &str, client: Option<&str>) {
    let mut out = std::io::stdout();
    let _ = write!(out, "  {} {}  ", kind.glyph(), text);
    let _ = out.flush();

    let t = crate::tmux::current();
    let pane = client.and_then(active_pane);

    let raw = crossterm::terminal::enable_raw_mode().is_ok();
    // A blocking read on its own thread: the timing lives here, and the
    // process exit takes the thread with it.
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    if pane.is_some() {
        std::thread::spawn(move || {
            let mut buf = [0u8; 1];
            let _ = std::io::stdin().read(&mut buf);
            let _ = tx.send(());
        });
    }

    let deadline = Instant::now() + Duration::from_millis(duration_ms);
    let fade = kind.fade();
    let steps = [
        (duration_ms.saturating_sub(FADE_MS), fade[0]),
        (duration_ms.saturating_sub(FADE_MS / 2), fade[1]),
    ];
    let start = Instant::now();
    for (at, style) in steps {
        let at = start + Duration::from_millis(at);
        if rx
            .recv_timeout(at.saturating_duration_since(Instant::now()))
            .is_ok()
        {
            return dismiss(raw, &*t, pane.as_deref());
        }
        restyle(&*t, style);
    }
    let _ = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    dismiss(raw, &*t, pane.as_deref());
}

/// Leave the toast: forward whatever is waiting on stdin, restore the tty.
fn dismiss(raw: bool, t: &dyn Tmux, pane: Option<&str>) {
    if let Some(pane) = pane {
        // The thread consumed at most one byte; the rest of an escape
        // sequence is still queued, and `read` returns what is there.
        let mut stdin = std::io::stdin();
        forward_key(&mut stdin, t, pane);
    }
    if raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::LivePane;

    #[derive(Default)]
    struct Rec(std::cell::RefCell<Vec<String>>);

    impl Tmux for Rec {
        fn list_panes(&self) -> Vec<LivePane> {
            Vec::new()
        }
        fn run(&self, cmds: &[Vec<String>]) {
            self.0.borrow_mut().push(cmds[0].join(" "));
        }
    }

    #[test]
    fn a_long_message_is_ellipsized_to_the_max_width() {
        assert_eq!(ellipsize("short"), "short");
        assert_eq!(width_for("short"), 9);
        let long = "x".repeat(200);
        let cut = ellipsize(&long);
        assert!(cut.ends_with('…'));
        assert_eq!(cut.width(), MAX_WIDTH - CHROME);
        assert_eq!(width_for(&cut), MAX_WIDTH);
    }

    #[test]
    fn width_counts_display_columns_not_bytes() {
        // Two wide glyphs are four columns, not two chars and not six bytes.
        assert_eq!(width_for("日本"), 4 + CHROME);
    }

    #[test]
    fn the_first_keystroke_reaches_the_pane_verbatim() {
        let t = Rec::default();
        let mut input = std::io::Cursor::new(b"-y".to_vec());
        assert!(forward_key(&mut input, &t, "%7"));
        assert_eq!(
            t.0.borrow()[0],
            "send-keys -t %7 -l -- -y",
            "-- keeps a leading dash out of tmux's flag parsing"
        );
        // Nothing left to read: no phantom keystroke.
        assert!(!forward_key(&mut input, &t, "%7"));
    }

    #[test]
    fn the_launch_command_is_a_borderless_bottom_right_popup() {
        let argv = launch_command("/bin/perch", "/dev/ttys001", Kind::Done, 3000, "ok", "bg=x");
        assert_eq!(
            argv.join(" "),
            "display-popup -c /dev/ttys001 -B -E -x R -y P -w 6 -h 1 -s bg=x \
             -- /bin/perch toast-body done 3000 ok --client /dev/ttys001"
        );
    }
}
