//! The centered notification card: a borderless `display-popup` per client.
//!
//! The sound says *something* happened; the card says *what*, on every screen
//! attached to this server, and then fades away. It is three lines — who
//! finished, where it lives, and what it said — drawn dead center, faded in
//! over 300 ms and out over 500 ms by restyling the popup from inside itself.
//!
//! The first keystroke dismisses the card *and* is forwarded verbatim to the
//! client's active pane with `send-keys -l --`, so a card drawn over someone
//! who is typing can never eat a character. That passthrough is the whole
//! reason this is allowed to take the screen at all.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::model::{PaneRecord, State};
use crate::tmux::Tmux;

/// Padding around the widest line: two columns each side.
const CHROME: usize = 4;
/// The card never gets narrower than this, however short its lines.
pub const MIN_WIDTH: usize = 30;
/// …nor wider than this, however long the last message.
pub const MAX_WIDTH: usize = 70;
/// The card is always exactly this tall.
pub const HEIGHT: usize = 3;
/// How long the fade-in takes, in three restyle steps.
const FADE_IN_MS: u64 = 300;
/// How long the fade-out takes, in the reverse three steps.
const FADE_OUT_MS: u64 = 500;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    Done,
    NeedsInput,
    /// A sample card, so a user can check placement without waiting for an
    /// agent to finish.
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

    /// The glyph and the words that open line one.
    pub fn heading(self) -> &'static str {
        match self {
            Kind::NeedsInput => "⚑ needs input",
            _ => "✓ done",
        }
    }

    /// The three styles this kind fades through: dim, mid, full.
    pub fn styles(self, cfg: &Config) -> [String; 3] {
        match self {
            Kind::NeedsInput => cfg.notify.needs_input_styles(),
            _ => cfg.notify.done_styles(),
        }
    }
}

/// The kind a state deserves a card for, or `None` for one that does not.
pub fn kind_for(state: State) -> Option<Kind> {
    match state {
        State::Done => Some(Kind::Done),
        State::NeedsInput => Some(Kind::NeedsInput),
        _ => None,
    }
}

/// Display columns of a string.
///
/// `chars().count()` — perch does not carry `unicode-width`, and the cost of
/// being one column off on a CJK last message is a card one column narrow.
pub fn width(s: &str) -> usize {
    s.chars().count()
}

/// `text` cut to `budget` columns, with an ellipsis when it was cut.
pub fn ellipsize(text: &str, budget: usize) -> String {
    if width(text) <= budget {
        return text.to_string();
    }
    if budget == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(budget - 1).collect();
    out.push('…');
    out
}

/// The three lines of a card, and the popup width that holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub lines: [String; 3],
    pub width: usize,
}

impl Card {
    /// Each line centered in `width` columns: what the body prints.
    pub fn rendered(&self) -> String {
        self.lines
            .iter()
            .map(|l| center(l, self.width))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn center(line: &str, cols: usize) -> String {
    let w = width(line);
    if w >= cols {
        return line.to_string();
    }
    let left = (cols - w) / 2;
    format!("{}{}", " ".repeat(left), line)
}

/// Build the card for one pane record.
///
/// Line 1 is the heading plus `<project> (<branch>)`, line 2 the tmux
/// location and the harness, line 3 the agent's last message. The width is
/// the widest line plus the chrome, clamped, and the message is then cut to
/// fit — so a long message widens the card up to the cap and no further.
pub fn card(kind: Kind, rec: &PaneRecord) -> Card {
    let project = rec.project.clone().unwrap_or_else(|| "—".into());
    let head = match &rec.branch {
        Some(b) if !b.is_empty() => format!("{}  {project} ({b})", kind.heading()),
        _ => format!("{}  {project}", kind.heading()),
    };
    let where_ = rec.location.clone().unwrap_or_else(|| rec.pane.clone());
    let mid = format!("{where_}  {}", rec.harness.as_str());
    let msg = rec
        .last_message
        .clone()
        .unwrap_or_default()
        .replace(['\n', '\r', '\t'], " ")
        .trim()
        .to_string();

    let widest = width(&head).max(width(&mid)).max(width(&msg));
    let w = (widest + CHROME).clamp(MIN_WIDTH, MAX_WIDTH);
    let budget = w - CHROME;
    Card {
        lines: [
            ellipsize(&head, budget),
            ellipsize(&mid, budget),
            ellipsize(&msg, budget),
        ],
        width: w,
    }
}

/// The record a `perch notify test` card is drawn from.
pub fn sample_record() -> PaneRecord {
    let mut rec = PaneRecord::new(
        "%0",
        crate::model::Harness::Claude,
        &crate::store::now_rfc3339(),
    );
    rec.project = Some("perch".into());
    rec.branch = Some("main".into());
    rec.location = Some("editor.1".into());
    rec.state = State::Done;
    rec.last_message = Some("Sample card — this is where the last message goes.".into());
    rec
}

/// The `display-popup` argv that draws one card on one client.
///
/// `-B` drops the border, `-x C -y C` centers it on that client, `-E` closes
/// the popup when the body exits, and `-s` sets the style it fades up from.
pub fn launch_command(
    exe: &str,
    client: &str,
    kind: Kind,
    duration_ms: u64,
    card: &Card,
    style0: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "display-popup".into(),
        "-c".into(),
        client.into(),
        "-B".into(),
        "-E".into(),
        "-x".into(),
        "C".into(),
        "-y".into(),
        "C".into(),
        "-w".into(),
        card.width.to_string(),
        "-h".into(),
        HEIGHT.to_string(),
        "-s".into(),
        style0.into(),
        "--".into(),
        exe.into(),
        "notify-body".into(),
        kind.as_str().into(),
        duration_ms.to_string(),
        "--client".into(),
        client.into(),
    ];
    for line in &card.lines {
        argv.push("--line".into());
        argv.push(line.clone());
    }
    argv
}

/// Every attached client, once each.
pub fn clients(t: &dyn Tmux) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in t.client_views() {
        if !v.client.is_empty() && !out.contains(&v.client) {
            out.push(v.client);
        }
    }
    out
}

/// Draw the card on every attached client.
pub fn show(t: &dyn Tmux, cfg: &Config, kind: Kind, rec: &PaneRecord) {
    if !cfg.notify.enabled && kind != Kind::Test {
        return;
    }
    let card = card(kind, rec);
    let styles = kind.styles(cfg);
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "perch".into());
    for client in clients(t) {
        // One spawned `display-popup` per client, never waited on: a second
        // popup on the same client replaces the first, which is what makes
        // two transitions in a row behave.
        t.batch(&[launch_command(
            &exe,
            &client,
            kind,
            cfg.notify.duration_ms,
            &card,
            &styles[0],
        )]);
    }
}

/// Ask a detached `perch notify` to draw the card, off the hook's path.
///
/// Under `PERCH_NO_TMUX` nothing is spawned; the request is recorded in
/// `PERCH_TMUX_LOG` so the hook's behaviour stays testable without a server.
pub fn spawn_detached(kind: Kind, pane: &str) {
    if !crate::tmux::enabled() {
        crate::tmux::NullTmux::empty().batch(&[vec![
            "notify".into(),
            kind.as_str().into(),
            pane.into(),
        ]]);
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .args(["notify", kind.as_str(), "--pane", pane])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// The active pane of a client: where a forwarded keystroke goes.
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

/// Send bytes the user typed at the card straight on to `pane`.
///
/// `-l` is literal and `--` keeps a leading `-` out of tmux's flag parsing,
/// so the keystroke that dismissed the card still lands in the agent.
pub fn send_keys(t: &dyn Tmux, pane: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    // Synchronous: the popup's pty dies the moment this process returns, and
    // a merely-spawned tmux would be killed before it was read.
    t.run(&[vec![
        "send-keys".into(),
        "-t".into(),
        pane.into(),
        "-l".into(),
        "--".into(),
        String::from_utf8_lossy(bytes).into_owned(),
    ]]);
}

/// Read once from `r`; if the user typed, forward those bytes to `pane`.
///
/// The reader is a parameter so the passthrough can be tested without a tty.
pub fn forward_key<R: Read>(r: &mut R, t: &dyn Tmux, pane: &str) -> bool {
    let mut buf = [0u8; 512];
    match r.read(&mut buf) {
        Ok(n) if n > 0 => {
            send_keys(t, pane, &buf[..n]);
            true
        }
        _ => false,
    }
}

/// Restyle the popup this process is running inside — the fade itself.
fn restyle(t: &dyn Tmux, style: &str) {
    t.batch(&[vec!["display-popup".into(), "-s".into(), style.into()]]);
}

/// The fade schedule: `(milliseconds since start, style)`, in order.
///
/// Three steps up over `FADE_IN_MS`, then the same three in reverse over the
/// last `FADE_OUT_MS`. A duration too short to hold both simply overlaps them;
/// the steps stay ordered, so the card still ends dim.
pub fn schedule(styles: &[String; 3], duration_ms: u64) -> Vec<(u64, String)> {
    let step_in = FADE_IN_MS / 3;
    let mut out: Vec<(u64, String)> = vec![
        (0, styles[0].clone()),
        (step_in, styles[1].clone()),
        (step_in * 2, styles[2].clone()),
    ];
    let step_out = FADE_OUT_MS / 3;
    let start_out = duration_ms.saturating_sub(FADE_OUT_MS);
    for (i, s) in styles.iter().rev().enumerate() {
        out.push((start_out + step_out * i as u64, s.clone()));
    }
    let mut last = 0;
    for step in out.iter_mut() {
        step.0 = step.0.max(last);
        last = step.0;
    }
    out
}

/// The popup's body: print the card, fade it in, hold, fade it out.
///
/// The client's active pane is read *before* raw mode, because a `tmux
/// display` from inside the popup's pty is still an ordinary command and the
/// answer is needed whether or not a key ever arrives.
pub fn body(kind: Kind, duration_ms: u64, lines: &[String], client: Option<&str>) {
    let cfg = crate::config::load();
    let t = crate::tmux::current();
    let pane = client.and_then(active_pane);

    // The popup is exactly as wide as the card, so the terminal's own size is
    // the width to center in — the launcher passes the lines, not the padding.
    let cols = crossterm::terminal::size()
        .map(|(c, _)| c as usize)
        .unwrap_or(0);
    let body: Vec<String> = lines.iter().map(|l| center(l, cols)).collect();
    let mut out = std::io::stdout();
    let _ = write!(out, "{}", body.join("\r\n"));
    let _ = out.flush();

    let raw = crossterm::terminal::enable_raw_mode().is_ok();
    // A blocking read on its own thread, sending back what it read: the
    // timing lives here, and the process exit takes the thread with it. The
    // main thread never reads stdin itself, because a read with nothing
    // queued would block past the card's own deadline.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    if pane.is_some() {
        std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            if let Ok(n) = std::io::stdin().read(&mut buf) {
                if n > 0 {
                    let _ = tx.send(buf[..n].to_vec());
                }
            }
        });
    }

    let start = Instant::now();
    let mut typed: Option<Vec<u8>> = None;
    for (at, style) in schedule(&kind.styles(&cfg), duration_ms) {
        let at = start + Duration::from_millis(at);
        if let Ok(bytes) = rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
            typed = Some(bytes);
            break;
        }
        restyle(&*t, &style);
    }
    if typed.is_none() {
        let end = start + Duration::from_millis(duration_ms);
        typed = rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
            .ok();
    }
    dismiss(raw, &*t, pane.as_deref(), typed.as_deref());
}

/// Leave the card: forward whatever the user typed, restore the tty.
fn dismiss(raw: bool, t: &dyn Tmux, pane: Option<&str>, typed: Option<&[u8]>) {
    if let (Some(pane), Some(bytes)) = (pane, typed) {
        send_keys(t, pane, bytes);
    }
    if raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Harness;
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

    fn rec() -> PaneRecord {
        let mut r = PaneRecord::new("%12", Harness::Claude, "2026-01-01T00:00:00Z");
        r.project = Some("perch".into());
        r.branch = Some("main".into());
        r.location = Some("editor.1".into());
        r.last_message = Some("Added the reducer and its tests.".into());
        r
    }

    #[test]
    fn the_card_says_what_finished_where_and_what_it_said() {
        let c = card(Kind::Done, &rec());
        assert_eq!(c.lines[0], "✓ done  perch (main)");
        assert_eq!(c.lines[1], "editor.1  claude");
        assert_eq!(c.lines[2], "Added the reducer and its tests.");
        assert_eq!(c.width, width(&c.lines[2]) + CHROME);

        let c = card(Kind::NeedsInput, &rec());
        assert!(c.lines[0].starts_with("⚑ needs input"));
    }

    /// Missing fields degrade rather than draw an empty box.
    #[test]
    fn a_bare_record_still_makes_a_card() {
        let c = card(Kind::Done, &PaneRecord::new("%9", Harness::Codex, "t"));
        assert_eq!(c.lines[0], "✓ done  —");
        assert_eq!(
            c.lines[1], "%9  codex",
            "no location: the pane id stands in"
        );
        assert_eq!(c.lines[2], "");
        assert_eq!(c.width, MIN_WIDTH, "a short card is still a card");
    }

    #[test]
    fn the_width_is_the_widest_line_clamped_and_the_message_is_cut_to_it() {
        let mut r = rec();
        r.last_message = Some("x".repeat(400));
        let c = card(Kind::Done, &r);
        assert_eq!(c.width, MAX_WIDTH);
        assert_eq!(width(&c.lines[2]), MAX_WIDTH - CHROME);
        assert!(c.lines[2].ends_with('…'));
        // A newline in the last message would break the three-line layout.
        r.last_message = Some("one\ntwo".into());
        assert_eq!(card(Kind::Done, &r).lines[2], "one two");
    }

    #[test]
    fn ellipsis_only_appears_when_something_was_cut() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("abcdef", 6), "abcdef");
        assert_eq!(ellipsize("abcdef", 5), "abcd…");
        assert_eq!(ellipsize("abc", 0), "");
    }

    #[test]
    fn every_line_is_centered_in_the_popup() {
        let c = Card {
            lines: ["ab".into(), "abcd".into(), "".into()],
            width: 10,
        };
        assert_eq!(c.rendered(), "    ab\n   abcd\n     ");
    }

    #[test]
    fn the_launch_command_is_a_borderless_centered_popup_per_client() {
        let c = Card {
            lines: ["a".into(), "b".into(), "c".into()],
            width: 30,
        };
        let argv = launch_command("/bin/perch", "/dev/ttys001", Kind::Done, 3500, &c, "bg=x");
        assert_eq!(
            argv.join(" "),
            "display-popup -c /dev/ttys001 -B -E -x C -y C -w 30 -h 3 -s bg=x \
             -- /bin/perch notify-body done 3500 --client /dev/ttys001 \
             --line a --line b --line c"
        );
    }

    #[test]
    fn a_disabled_card_is_never_drawn() {
        let mut cfg = Config::default();
        cfg.notify.enabled = false;
        let t = Rec::default();
        show(&t, &cfg, Kind::Done, &rec());
        assert!(t.0.borrow().is_empty());
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
    fn the_fade_goes_up_in_three_steps_and_back_down_in_three() {
        let styles = ["a".to_string(), "b".to_string(), "c".to_string()];
        let s = schedule(&styles, 3500);
        let names: Vec<&str> = s.iter().map(|(_, st)| st.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "c", "b", "a"]);
        assert_eq!(s[0].0, 0);
        assert_eq!(s[2].0, 200, "full brightness within the 300ms fade-in");
        assert_eq!(s[3].0, 3000, "the fade out starts 500ms before the end");
        assert!(s.last().unwrap().0 < 3500);
        // Ordered even when the duration is shorter than both fades.
        let s = schedule(&styles, 100);
        assert!(s.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn only_done_and_needs_input_deserve_a_card() {
        assert_eq!(kind_for(State::Done), Some(Kind::Done));
        assert_eq!(kind_for(State::NeedsInput), Some(Kind::NeedsInput));
        assert_eq!(kind_for(State::Working), None);
        assert_eq!(kind_for(State::Idle), None);
        assert_eq!(Kind::parse("needs-input"), Some(Kind::NeedsInput));
        assert_eq!(Kind::parse("nope"), None);
    }

    #[test]
    fn each_client_is_named_once() {
        struct Two;
        impl Tmux for Two {
            fn list_panes(&self) -> Vec<LivePane> {
                Vec::new()
            }
            fn client_views(&self) -> Vec<crate::tmux::ClientView> {
                ["/dev/a", "/dev/b", "/dev/a"]
                    .iter()
                    .map(|c| crate::tmux::ClientView {
                        pane: "%1".into(),
                        client: (*c).into(),
                        focused: false,
                    })
                    .collect()
            }
        }
        assert_eq!(clients(&Two), vec!["/dev/a", "/dev/b"]);
    }
}
