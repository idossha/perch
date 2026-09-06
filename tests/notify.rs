//! The notification card, through the public API: what it says, how wide it
//! gets, one popup per client, and the keystroke it must never eat.

use std::cell::RefCell;

use perch::config::Config;
use perch::model::{Harness, PaneRecord, State};
use perch::notify::{self, Kind};
use perch::tmux::{ClientView, LivePane, Tmux};

/// A tmux that records every invocation and reports the clients it was made
/// with — `batch` is what `show` uses, `run` what a forwarded key uses.
struct Fake {
    clients: Vec<&'static str>,
    calls: RefCell<Vec<String>>,
}

impl Fake {
    fn with(clients: &[&'static str]) -> Fake {
        Fake {
            clients: clients.to_vec(),
            calls: RefCell::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl Tmux for Fake {
    fn list_panes(&self) -> Vec<LivePane> {
        Vec::new()
    }
    fn client_views(&self) -> Vec<ClientView> {
        self.clients
            .iter()
            .map(|c| ClientView {
                pane: "%1".into(),
                client: (*c).into(),
                focused: false,
            })
            .collect()
    }
    fn batch(&self, cmds: &[Vec<String>]) {
        self.calls.borrow_mut().push(cmds[0].join(" "));
    }
    fn run(&self, cmds: &[Vec<String>]) {
        self.batch(cmds);
    }
}

fn record() -> PaneRecord {
    let mut r = PaneRecord::new("%12", Harness::Claude, "2026-01-01T00:00:00Z");
    r.project = Some("perch".into());
    r.branch = Some("main".into());
    r.location = Some("editor.1".into());
    r.state = State::Done;
    r.last_message = Some("Added the reducer and its tests.".into());
    r
}

/// The card as the user reads it: SGR stripped, padding kept.
const DONE_CARD: &str = "\
\x20 ✓ done  perch (main)
  editor.1  ·  claude
  Added the reducer and its tests.";

const NEEDS_INPUT_CARD: &str = "\
\x20 ⚑ needs input  perch (main)
  editor.1  ·  claude
  Waiting on you: allow the write?";

#[test]
fn the_rendered_card_reads_the_same_for_both_kinds() {
    let cfg = Config::default();
    let c = notify::card(Kind::Done, &record(), &cfg);
    assert_eq!(notify::render_plain(&c), DONE_CARD);

    let mut r = record();
    r.state = State::NeedsInput;
    r.last_message = Some("Waiting on you: allow the write?".into());
    let c = notify::card(Kind::NeedsInput, &r, &cfg);
    assert_eq!(notify::render_plain(&c), NEEDS_INPUT_CARD);
}

#[test]
fn the_card_carries_project_location_harness_and_the_last_message() {
    let c = notify::card(Kind::Done, &record(), &Config::default());
    assert_eq!(
        c.lines[0], "\x1b[1;38;5;114m✓ done\x1b[0m  perch\x1b[2m (main)\x1b[0m",
        "the heading is the dashboard's done colour, bold; the branch is dim"
    );
    assert_eq!(c.lines[1], "\x1b[2meditor.1  ·  claude\x1b[0m");
    assert_eq!(c.lines[2], "\x1b[0mAdded the reducer and its tests.\x1b[0m");
    assert_eq!(c.lines.len(), 3, "three lines inside the rounded border");
}

#[test]
fn the_width_is_clamped_at_both_ends() {
    // Nothing to say: still a card of a readable minimum width.
    let mut r = record();
    r.project = Some("p".into());
    r.branch = None;
    r.location = Some("w".into());
    r.last_message = None;
    assert_eq!(
        notify::card(Kind::Done, &r, &Config::default()).width,
        notify::MIN_WIDTH
    );

    // A novel for a last message: capped, and cut with an ellipsis to fit.
    r.last_message = Some("word ".repeat(200));
    let c = notify::card(Kind::Done, &r, &Config::default());
    assert_eq!(c.width, notify::MAX_WIDTH);
    let plain = notify::strip_sgr(&c.lines[2]);
    assert!(plain.ends_with('…'));
    assert_eq!(notify::width(&plain), c.width - 2 * notify::PAD);
    for line in notify::render_plain(&c).lines() {
        assert!(
            notify::width(line) <= c.width,
            "{line:?} overflows the card"
        );
    }
}

#[test]
fn one_popup_is_drawn_per_attached_client() {
    let t = Fake::with(&["/dev/ttys001", "/dev/ttys002"]);
    notify::show(&t, &Config::default(), Kind::Done, &record());
    let calls = t.calls();
    assert_eq!(calls.len(), 2, "one popup per client: {calls:?}");
    for (i, client) in ["/dev/ttys001", "/dev/ttys002"].iter().enumerate() {
        let argv = &calls[i];
        assert!(
            argv.starts_with(&format!(
                "display-popup -c {client} -b rounded -E -x C -y C -w "
            )),
            "a rounded box centered on its own client: {argv}"
        );
        assert!(
            argv.contains(" -h 5 -s bg=default,fg=default -S fg=colour240,dim -- "),
            "the terminal's own background, a dim grey border: {argv}"
        );
        assert!(
            argv.contains(&format!("notify-body done 3500 --client {client} --line ")),
            "the body is told its client and its lines: {argv}"
        );
        assert_eq!(argv.matches("--line ").count(), 3);
        assert!(argv.contains("--line \x1b[1;38;5;114m✓ done\x1b[0m  perch"));
    }
}

/// The popup is two columns wider than the text field: the border's own.
#[test]
fn the_popup_is_the_text_field_plus_its_border() {
    let t = Fake::with(&["/dev/ttys001"]);
    notify::show(&t, &Config::default(), Kind::Done, &record());
    let card = notify::card(Kind::Done, &record(), &Config::default());
    assert!(t.calls()[0].contains(&format!(" -w {} -h 5 ", card.width + 2)));
}

#[test]
fn a_needs_input_card_wears_its_own_accent() {
    let t = Fake::with(&["/dev/ttys001"]);
    let mut r = record();
    r.state = State::NeedsInput;
    notify::show(&t, &Config::default(), Kind::NeedsInput, &r);
    let argv = t.calls().remove(0);
    assert!(argv.contains("notify-body needs_input 3500"));
    assert!(argv.contains("--line \x1b[1;38;5;203m⚑ needs input\x1b[0m  perch"));

    let cfg = Config::default();
    assert_eq!(Kind::NeedsInput.accent(&cfg), "colour203");
    assert_eq!(Kind::Done.accent(&cfg), "colour114");

    // The fade dims the text and the border together, and ends where it began.
    let steps = notify::schedule(3500);
    assert_eq!(
        steps.iter().map(|(_, l)| *l).collect::<Vec<u8>>(),
        [0, 1, 2, 2, 1, 0],
        "three up, three back down"
    );
    assert_eq!(notify::border_style(&cfg, 0), "fg=colour240,dim");
    assert_eq!(notify::border_style(&cfg, 2), "fg=colour240");
    let line = &notify::card(Kind::NeedsInput, &r, &cfg).lines[0];
    assert!(
        notify::fade(line, 0).starts_with("\x1b[2;38;5;203m"),
        "dim first"
    );
    assert_eq!(
        notify::fade(line, 2),
        *line,
        "and full at the top of the fade"
    );
    assert_eq!(
        notify::strip_sgr(&notify::fade(line, 0)),
        notify::strip_sgr(line)
    );
}

/// An accent override reaches the card without any other key being set.
#[test]
fn the_accent_and_border_colours_come_from_the_config() {
    let mut cfg = Config::default();
    cfg.notify.accent_done = "green".into();
    cfg.notify.border = "colour238".into();
    let c = notify::card(Kind::Done, &record(), &cfg);
    assert!(c.lines[0].starts_with("\x1b[1;32m✓ done"));
    assert_eq!(notify::border_style(&cfg, 1), "fg=colour238");
}

/// The passthrough guarantee: a key typed while the card is up dismisses it
/// *and* lands in the pane the user thought they were typing at.
#[test]
fn a_keystroke_is_forwarded_verbatim_to_the_clients_pane() {
    let t = Fake::with(&[]);
    let mut input = std::io::Cursor::new(b"-hello".to_vec());
    assert!(notify::forward_key(&mut input, &t, "%7"));
    assert_eq!(
        t.calls(),
        vec!["send-keys -t %7 -l -- -hello".to_string()],
        "-- keeps a leading dash out of tmux's flag parsing, -l keeps it literal"
    );
    // The reader is drained: no phantom second keystroke.
    assert!(!notify::forward_key(&mut input, &t, "%7"));
    assert_eq!(t.calls().len(), 1);
}

#[test]
fn the_sample_card_needs_no_record_and_ignores_enabled() {
    let mut cfg = Config::default();
    cfg.notify.enabled = false;
    let t = Fake::with(&["/dev/ttys001"]);
    notify::show(&t, &cfg, Kind::Test, &notify::sample_record());
    assert_eq!(t.calls().len(), 1, "`perch notify test` always draws");

    let t = Fake::with(&["/dev/ttys001"]);
    notify::show(&t, &cfg, Kind::Done, &record());
    assert!(t.calls().is_empty(), "a disabled card is never drawn");
}
