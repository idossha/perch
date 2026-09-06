//! The centered notification card: a small bordered `display-popup` per client.
//!
//! The sound says *something* happened; the card says *what*, on every screen
//! attached to this server, and then fades away. It is three lines in a
//! rounded grey box over the terminal's own background — who finished, where
//! it lives, and what it said — drawn dead center, faded in over 300 ms and
//! out over 500 ms by restyling the border and reprinting the text.
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

/// Blank columns between the border and the text, each side.
pub const PAD: usize = 2;
/// Padding around the widest line: [`PAD`] columns each side.
const CHROME: usize = PAD * 2;
/// The two columns the rounded border itself costs.
const BORDER_COLS: usize = 2;
/// The card never gets narrower than this, however short its lines.
pub const MIN_WIDTH: usize = 36;
/// …nor wider than this, however long the last message.
pub const MAX_WIDTH: usize = 72;
/// Popup height: three lines of text between the two border rows.
pub const HEIGHT: usize = 5;
/// How long the fade-in takes, in three steps.
const FADE_IN_MS: u64 = 300;
/// How long the fade-out takes, in the reverse three steps.
const FADE_OUT_MS: u64 = 500;

/// SGR parameters for text in the terminal's own foreground.
const NORMAL: &str = "0";
/// …and for the quiet half of a line.
const DIM: &str = "2";

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

    /// The glyph and the words that open line one — the dashboard's glyphs.
    pub fn heading(self) -> &'static str {
        match self {
            Kind::NeedsInput => "⚑ needs input",
            _ => "✓ done",
        }
    }

    /// The configured colour name for this kind's heading.
    pub fn accent(self, cfg: &Config) -> &str {
        match self {
            Kind::NeedsInput => &cfg.notify.accent_needs_input,
            _ => &cfg.notify.accent_done,
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

/// SGR parameters for a colour name: `colour114`, `red`, or a raw number.
///
/// tmux style names are what the config speaks, but the body prints raw text
/// into a pty — `#[fg=…]` means nothing there, so the name is turned into the
/// escape the terminal understands.
pub fn fg_params(colour: &str) -> String {
    let name = colour.trim();
    let n = name
        .strip_prefix("colour")
        .or_else(|| name.strip_prefix("color"))
        .unwrap_or(name);
    if let Ok(i) = n.parse::<u8>() {
        return format!("38;5;{i}");
    }
    let base = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    if let Some(i) = base.iter().position(|b| *b == name) {
        return (30 + i).to_string();
    }
    if let Some(rest) = name.strip_prefix("bright") {
        if let Some(i) = base.iter().position(|b| *b == rest.trim_start_matches('-')) {
            return (90 + i).to_string();
        }
    }
    "39".to_string()
}

/// One run of text on a card line, with the SGR parameters it is drawn in.
type Seg = (String, String);

/// The three lines of a card — each already carrying its SGR — and the width
/// of the text field they sit in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub lines: [String; 3],
    pub width: usize,
}

/// Join segments into one styled line, cut to `budget` display columns.
fn line_of(segs: &[Seg], budget: usize) -> String {
    let mut out = String::new();
    let mut left = budget;
    for (text, params) in segs {
        if left == 0 {
            break;
        }
        let cut = ellipsize(text, left);
        if cut.is_empty() {
            continue;
        }
        left -= width(&cut);
        out.push_str(&format!("\x1b[{params}m{cut}"));
    }
    out.push_str("\x1b[0m");
    out
}

/// The plain text of a styled line: every escape removed.
pub fn strip_sgr(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Rewrite every escape in a styled line for one step of the fade.
///
/// Level 2 is the card as designed, level 1 the same without bold, level 0
/// the whole line dim. Reprinting these three is the fade: no background
/// changes hands, so nothing flashes.
pub fn fade(line: &str, level: u8) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find('\x1b') {
        out.push_str(&rest[..i]);
        let Some(end) = rest[i..].find('m') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let params = &rest[i + 2..i + end];
        out.push_str(&format!("\x1b[{}m", faded_params(params, level)));
        rest = &rest[i + end + 1..];
    }
    out.push_str(rest);
    out
}

fn faded_params(params: &str, level: u8) -> String {
    let unbold = params.strip_prefix("1;").unwrap_or(params);
    if level >= 2 {
        return params.to_string();
    }
    if level == 1 {
        return unbold.to_string();
    }
    if unbold == NORMAL || unbold.starts_with(DIM) {
        return DIM.to_string();
    }
    format!("{DIM};{unbold}")
}

impl Card {
    /// The card as the popup prints it at `level`: padded and cut to width.
    pub fn frame(&self, level: u8) -> String {
        self.lines
            .iter()
            .map(|l| format!("{}{}", " ".repeat(PAD), fade(l, level)))
            .collect::<Vec<_>>()
            .join("\r\n")
    }
}

/// The card as plain text, padding included and every escape stripped: what
/// the user reads, and what a snapshot test can assert verbatim.
pub fn render_plain(card: &Card) -> String {
    card.lines
        .iter()
        .map(|l| format!("{}{}", " ".repeat(PAD), strip_sgr(l)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the card for one pane record.
///
/// Line 1 is the accented heading, the project, and the branch dim; line 2 the
/// tmux location and the harness, both dim, joined by a middle dot; line 3 the
/// agent's last message. The width is the widest line plus the padding,
/// clamped, and every line is then cut to fit.
pub fn card(kind: Kind, rec: &PaneRecord, cfg: &Config) -> Card {
    let accent = fg_params(kind.accent(cfg));
    let project = rec.project.clone().unwrap_or_else(|| "—".into());
    let mut head: Vec<Seg> = vec![
        (kind.heading().to_string(), format!("1;{accent}")),
        (format!("  {project}"), NORMAL.into()),
    ];
    if let Some(b) = rec.branch.as_deref().filter(|b| !b.is_empty()) {
        head.push((format!(" ({b})"), DIM.into()));
    }

    let where_ = rec.location.clone().unwrap_or_else(|| rec.pane.clone());
    let mid: Vec<Seg> = vec![(format!("{where_}  ·  {}", rec.harness.as_str()), DIM.into())];

    let msg = rec
        .last_message
        .clone()
        .unwrap_or_default()
        .replace(['\n', '\r', '\t'], " ")
        .trim()
        .to_string();
    let tail: Vec<Seg> = vec![(msg, NORMAL.into())];

    let plain = |segs: &[Seg]| segs.iter().map(|(t, _)| width(t)).sum::<usize>();
    let widest = plain(&head).max(plain(&mid)).max(plain(&tail));
    let w = (widest + CHROME).clamp(MIN_WIDTH, MAX_WIDTH);
    let budget = w - CHROME;
    Card {
        lines: [
            line_of(&head, budget),
            line_of(&mid, budget),
            line_of(&tail, budget),
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

/// The border style at one step of the fade: grey, and dim before the card
/// has arrived. The popup itself keeps the terminal's own background.
pub fn border_style(cfg: &Config, level: u8) -> String {
    let base = format!("fg={}", cfg.notify.border);
    if level == 0 {
        format!("{base},dim")
    } else {
        base
    }
}

/// The `display-popup` argv that draws one card on one client.
///
/// `-b rounded` with a grey `-S` is the whole chrome; `-s bg=default` leaves
/// the popup on the terminal's own background so it reads as part of the
/// screen. `-x C -y C` centers it on that client and `-E` closes the popup
/// when the body exits. The popup is two columns wider than the text field,
/// for the border it draws itself.
pub fn launch_command(
    exe: &str,
    client: &str,
    kind: Kind,
    duration_ms: u64,
    card: &Card,
    border: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "display-popup".into(),
        "-c".into(),
        client.into(),
        "-b".into(),
        "rounded".into(),
        "-E".into(),
        "-x".into(),
        "C".into(),
        "-y".into(),
        "C".into(),
        "-w".into(),
        (card.width + BORDER_COLS).to_string(),
        "-h".into(),
        HEIGHT.to_string(),
        "-s".into(),
        "bg=default,fg=default".into(),
        "-S".into(),
        border.into(),
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
    let card = card(kind, rec, cfg);
    let border = border_style(cfg, 0);
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
            &border,
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

/// Restyle the border of the popup this process is running inside.
fn restyle(t: &dyn Tmux, style: &str) {
    t.batch(&[vec!["display-popup".into(), "-S".into(), style.into()]]);
}

/// The fade schedule: `(milliseconds since start, level)`, in order.
///
/// Three steps up over `FADE_IN_MS`, then the same three in reverse over the
/// last `FADE_OUT_MS`. A duration too short to hold both simply overlaps them;
/// the steps stay ordered, so the card still ends dim.
pub fn schedule(duration_ms: u64) -> Vec<(u64, u8)> {
    let step_in = FADE_IN_MS / 3;
    let mut out: Vec<(u64, u8)> = vec![(0, 0), (step_in, 1), (step_in * 2, 2)];
    let step_out = FADE_OUT_MS / 3;
    let start_out = duration_ms.saturating_sub(FADE_OUT_MS);
    for (i, level) in [2u8, 1, 0].into_iter().enumerate() {
        out.push((start_out + step_out * i as u64, level));
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
pub fn body(_kind: Kind, duration_ms: u64, lines: &[String], client: Option<&str>) {
    let cfg = crate::config::load();
    let t = crate::tmux::current();
    let pane = client.and_then(active_pane);

    let mut card = Card {
        lines: [String::new(), String::new(), String::new()],
        width: 0,
    };
    for (i, l) in lines.iter().take(3).enumerate() {
        card.lines[i] = l.clone();
    }
    let mut out = std::io::stdout();
    let mut draw = |level: u8| {
        // Home first: the fade reprints the same three lines in place.
        let _ = write!(out, "\x1b[H{}", card.frame(level));
        let _ = out.flush();
    };
    draw(0);

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
    for (at, level) in schedule(duration_ms) {
        let at = start + Duration::from_millis(at);
        if let Ok(bytes) = rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
            typed = Some(bytes);
            break;
        }
        restyle(&*t, &border_style(&cfg, level));
        draw(level);
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

    fn plain(c: &Card) -> Vec<String> {
        c.lines.iter().map(|l| strip_sgr(l)).collect()
    }

    #[test]
    fn the_card_says_what_finished_where_and_what_it_said() {
        let cfg = Config::default();
        let c = card(Kind::Done, &rec(), &cfg);
        assert_eq!(
            plain(&c),
            [
                "✓ done  perch (main)",
                "editor.1  ·  claude",
                "Added the reducer and its tests."
            ]
        );
        assert_eq!(c.width, width(&plain(&c)[2]) + CHROME);

        let c = card(Kind::NeedsInput, &rec(), &cfg);
        assert!(plain(&c)[0].starts_with("⚑ needs input"));
    }

    /// The heading carries the dashboard's colour, bold; the branch is dim.
    #[test]
    fn the_lines_carry_the_dashboard_colours_as_sgr() {
        let cfg = Config::default();
        let c = card(Kind::Done, &rec(), &cfg);
        assert_eq!(
            c.lines[0],
            "\x1b[1;38;5;114m✓ done\x1b[0m  perch\x1b[2m (main)\x1b[0m"
        );
        assert_eq!(c.lines[1], "\x1b[2meditor.1  ·  claude\x1b[0m");
        assert_eq!(c.lines[2], "\x1b[0mAdded the reducer and its tests.\x1b[0m");
        let c = card(Kind::NeedsInput, &rec(), &cfg);
        assert!(c.lines[0].starts_with("\x1b[1;38;5;203m⚑ needs input"));
    }

    #[test]
    fn a_colour_name_becomes_the_escape_the_terminal_understands() {
        assert_eq!(fg_params("colour114"), "38;5;114");
        assert_eq!(fg_params("color9"), "38;5;9");
        assert_eq!(fg_params("114"), "38;5;114");
        assert_eq!(fg_params("green"), "32");
        assert_eq!(fg_params("brightred"), "91");
        assert_eq!(fg_params("nonsense"), "39", "a typo is the default fg");
    }

    /// Missing fields degrade rather than draw an empty box.
    #[test]
    fn a_bare_record_still_makes_a_card() {
        let c = card(
            Kind::Done,
            &PaneRecord::new("%9", Harness::Codex, "t"),
            &Config::default(),
        );
        assert_eq!(plain(&c)[0], "✓ done  —");
        assert_eq!(
            plain(&c)[1],
            "%9  ·  codex",
            "no location: the pane id stands in"
        );
        assert_eq!(plain(&c)[2], "");
        assert_eq!(c.width, MIN_WIDTH, "a short card is still a card");
    }

    #[test]
    fn the_width_is_the_widest_line_clamped_and_the_message_is_cut_to_it() {
        let mut r = rec();
        r.last_message = Some("x".repeat(400));
        let c = card(Kind::Done, &r, &Config::default());
        assert_eq!(c.width, MAX_WIDTH);
        assert_eq!(width(&plain(&c)[2]), MAX_WIDTH - CHROME);
        assert!(plain(&c)[2].ends_with('…'));
        // A newline in the last message would break the three-line layout.
        r.last_message = Some("one\ntwo".into());
        assert_eq!(
            plain(&card(Kind::Done, &r, &Config::default()))[2],
            "one two"
        );
    }

    /// A long project cuts the *last* segment of line one, not the heading.
    #[test]
    fn a_line_is_cut_segment_by_segment() {
        let mut r = rec();
        r.project = Some("p".repeat(80));
        let c = card(Kind::Done, &r, &Config::default());
        let head = &plain(&c)[0];
        assert!(head.starts_with("✓ done  ppp"));
        assert!(head.ends_with('…'));
        assert_eq!(width(head), MAX_WIDTH - CHROME);
    }

    #[test]
    fn ellipsis_only_appears_when_something_was_cut() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("abcdef", 6), "abcdef");
        assert_eq!(ellipsize("abcdef", 5), "abcd…");
        assert_eq!(ellipsize("abc", 0), "");
    }

    #[test]
    fn every_line_is_printed_two_columns_in() {
        let c = Card {
            lines: [
                "\x1b[0mab\x1b[0m".into(),
                "\x1b[2mcd\x1b[0m".into(),
                "".into(),
            ],
            width: 10,
        };
        assert_eq!(render_plain(&c), "  ab\n  cd\n  ");
        assert_eq!(c.frame(2), "  \x1b[0mab\x1b[0m\r\n  \x1b[2mcd\x1b[0m\r\n  ");
    }

    /// The fade is a rewrite of the line's own escapes: nothing else moves.
    #[test]
    fn the_fade_unbolds_then_dims_every_run() {
        let line = "\x1b[1;38;5;114m✓ done\x1b[0m  perch\x1b[2m (main)\x1b[0m";
        assert_eq!(fade(line, 2), line);
        assert_eq!(
            fade(line, 1),
            "\x1b[38;5;114m✓ done\x1b[0m  perch\x1b[2m (main)\x1b[0m"
        );
        assert_eq!(
            fade(line, 0),
            "\x1b[2;38;5;114m✓ done\x1b[2m  perch\x1b[2m (main)\x1b[2m"
        );
        assert_eq!(
            strip_sgr(&fade(line, 0)),
            strip_sgr(line),
            "text is untouched"
        );
    }

    #[test]
    fn the_launch_command_is_a_rounded_centered_popup_per_client() {
        let c = Card {
            lines: ["a".into(), "b".into(), "c".into()],
            width: 36,
        };
        let argv = launch_command(
            "/bin/perch",
            "/dev/ttys001",
            Kind::Done,
            3500,
            &c,
            "fg=colour240,dim",
        );
        assert_eq!(
            argv.join(" "),
            "display-popup -c /dev/ttys001 -b rounded -E -x C -y C -w 38 -h 5 \
             -s bg=default,fg=default -S fg=colour240,dim \
             -- /bin/perch notify-body done 3500 --client /dev/ttys001 \
             --line a --line b --line c"
        );
    }

    #[test]
    fn the_border_is_grey_and_starts_dim() {
        let cfg = Config::default();
        assert_eq!(border_style(&cfg, 0), "fg=colour240,dim");
        assert_eq!(border_style(&cfg, 1), "fg=colour240");
        assert_eq!(border_style(&cfg, 2), "fg=colour240");
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
        let s = schedule(3500);
        let levels: Vec<u8> = s.iter().map(|(_, l)| *l).collect();
        assert_eq!(levels, [0, 1, 2, 2, 1, 0]);
        assert_eq!(s[0].0, 0);
        assert_eq!(s[2].0, 200, "full brightness within the 300ms fade-in");
        assert_eq!(s[3].0, 3000, "the fade out starts 500ms before the end");
        assert!(s.last().unwrap().0 < 3500);
        // Ordered even when the duration is shorter than both fades.
        let s = schedule(100);
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
