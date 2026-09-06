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

#[test]
fn the_card_carries_project_location_harness_and_the_last_message() {
    let c = notify::card(Kind::Done, &record());
    assert_eq!(c.lines[0], "✓ done  perch (main)");
    assert_eq!(c.lines[1], "editor.1  claude");
    assert_eq!(c.lines[2], "Added the reducer and its tests.");
    assert_eq!(c.lines.len(), 3, "three lines, and the popup is three tall");
}

#[test]
fn the_width_is_clamped_at_both_ends() {
    // Nothing to say: still a card of a readable minimum width.
    let mut r = record();
    r.project = Some("p".into());
    r.branch = None;
    r.location = Some("w".into());
    r.last_message = None;
    assert_eq!(notify::card(Kind::Done, &r).width, notify::MIN_WIDTH);

    // A novel for a last message: capped, and cut with an ellipsis to fit.
    r.last_message = Some("word ".repeat(200));
    let c = notify::card(Kind::Done, &r);
    assert_eq!(c.width, notify::MAX_WIDTH);
    assert!(c.lines[2].ends_with('…'));
    assert!(c.lines.iter().all(|l| notify::width(l) <= c.width - 4));
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
            argv.starts_with(&format!("display-popup -c {client} -B -E -x C -y C -w ")),
            "borderless and centered on its own client: {argv}"
        );
        assert!(argv.contains(" -h 3 -s bg=colour235,fg=colour240 -- "));
        assert!(
            argv.contains(&format!("notify-body done 3500 --client {client} --line ")),
            "the body is told its client and its lines: {argv}"
        );
        assert_eq!(argv.matches("--line ").count(), 3);
        assert!(argv.contains("--line ✓ done  perch (main)"));
    }
}

#[test]
fn a_needs_input_card_fades_up_through_its_own_colours() {
    let t = Fake::with(&["/dev/ttys001"]);
    let mut r = record();
    r.state = State::NeedsInput;
    notify::show(&t, &Config::default(), Kind::NeedsInput, &r);
    let argv = t.calls().remove(0);
    assert!(argv.contains("notify-body needs_input 3500"));
    assert!(argv.contains("--line ⚑ needs input  perch (main)"));

    let cfg = Config::default();
    let up = notify::Kind::NeedsInput.styles(&cfg);
    assert_eq!(up[0], "bg=colour235,fg=colour240", "starts dim");
    assert_eq!(up[2], "bg=colour160,fg=colour255,bold", "ends full");
    let steps = notify::schedule(&up, 3500);
    assert_eq!(steps.len(), 6, "three up, three back down");
    assert_eq!(steps[2].1, up[2]);
    assert_eq!(steps[5].1, up[0], "the card ends as dim as it began");
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
