//! The question popup: the answer form drawn by `display-popup` on every
//! attached client the moment a question arrives, so it is answered where
//! the user is rather than in the pane that asked.
//!
//! Same shape as the card in `notify`: the hook spawns a detached
//! `perch ask-popup --pane <pane>`, which opens one popup per client running
//! `perch ask-form --pane <pane> --client <client>`. The body is the
//! dashboard's form with nothing else around it; it writes the answer file
//! the waiting hook is polling, and exits — closing the popup — when the
//! question is answered anywhere, deferred, or gone.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crossterm::event::{self, Event as CEvent, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use crate::model::{PaneRecord, PendingQuestion};
use crate::store;
use crate::tmux::Tmux;
use crate::tui::{self, App, FormEvent};

/// Border colour of the popup: the dashboard's `needs_input`.
const BORDER: &str = "fg=colour203";
/// Widest the form gets, plus the two border columns.
const MAX_WIDTH: u16 = 110;
const MIN_WIDTH: u16 = 60;

/// Popup size for a question set, from its content: as wide as the longest
/// line wants, between `MIN_WIDTH` and `MAX_WIDTH`, and as tall as the
/// tallest question needs at that width — `tui::form_rows`, with wrapped
/// question and descriptions — plus the border. `client` (columns, rows)
/// caps both, leaving a margin, so the popup always fits the screen it is on.
pub fn popup_size(q: &PendingQuestion, client: Option<(u16, u16)>) -> (u16, u16) {
    let mut longest = 0usize;
    for question in &q.questions {
        longest = longest.max(question.question.chars().count());
        let labels: usize = question
            .options
            .iter()
            .map(|o| o.label.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(5, 24);
        for o in &question.options {
            longest = longest.max(labels + 8 + o.description.chars().count());
        }
    }
    let mut w = (longest as u16 + 4).clamp(MIN_WIDTH, MAX_WIDTH);
    if let Some((cw, _)) = client {
        w = w.min(cw.saturating_sub(4)).max(40);
    }
    let rows = q
        .questions
        .iter()
        .map(|question| tui::form_rows(question, w as usize))
        .max()
        .unwrap_or(6);
    let mut h = rows as u16 + 2;
    if let Some((_, ch)) = client {
        h = h.min(ch.saturating_sub(2)).max(8);
    }
    (w, h)
}

/// The `display-popup` argv that draws the form on one client.
pub fn launch_command(exe: &str, client: &str, pane: &str, w: u16, h: u16) -> Vec<String> {
    vec![
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
        w.to_string(),
        "-h".into(),
        h.to_string(),
        "-s".into(),
        "bg=default,fg=default".into(),
        "-S".into(),
        BORDER.into(),
        "--".into(),
        exe.into(),
        "ask-form".into(),
        "--pane".into(),
        pane.into(),
        "--client".into(),
        client.into(),
    ]
}

/// Directory of per-client popup claims: `<state>/ask-popups/<client>`.
pub fn popups_dir() -> PathBuf {
    store::state_dir().join("ask-popups")
}

fn claim_path(dir: &Path, client: &str) -> PathBuf {
    let key: String = client
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    dir.join(key)
}

/// `true` when a process with this pid exists (best effort; Unix).
fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists() || {
        // macOS has no /proc: `kill -0` through the shell-free libc call.
        #[cfg(unix)]
        {
            extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            // SAFETY: signal 0 delivers nothing; it only checks existence.
            unsafe { kill(pid as i32, 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

/// Claim the popup slot of one client for `pid`: one popup per client at a
/// time. A claim whose owner is gone is stale and is taken over.
pub fn claim_popup(dir: &Path, client: &str, pid: u32) -> bool {
    let _ = fs::create_dir_all(dir);
    let path = claim_path(dir, client);
    if let Ok(body) = fs::read_to_string(&path) {
        if let Ok(owner) = body.trim().parse::<u32>() {
            if owner != pid && pid_alive(owner) {
                return false;
            }
        }
    }
    fs::write(&path, pid.to_string()).is_ok()
}

pub fn release_popup(dir: &Path, client: &str) {
    let _ = fs::remove_file(claim_path(dir, client));
}

fn client_has_popup(client: &str) -> bool {
    let path = claim_path(&popups_dir(), client);
    fs::read_to_string(path)
        .ok()
        .and_then(|b| b.trim().parse::<u32>().ok())
        .is_some_and(pid_alive)
}

/// The next question set a popup should show: the oldest live one not
/// already handled in this popup.
pub fn next_pending<'a>(
    records: &'a [PaneRecord],
    now: DateTime<Utc>,
    handled: &HashSet<String>,
) -> Option<&'a PaneRecord> {
    records
        .iter()
        .filter(|r| !handled.contains(&r.pane))
        .filter(|r| r.live_question(now).is_some())
        .min_by(|a, b| {
            let ka = a
                .question
                .as_ref()
                .map(|q| q.asked_at.as_str())
                .unwrap_or("");
            let kb = b
                .question
                .as_ref()
                .map(|q| q.asked_at.as_str())
                .unwrap_or("");
            ka.cmp(kb).then_with(|| a.pane.cmp(&b.pane))
        })
}

/// Open the form popup for the pane's live question on every attached client
/// that has no popup yet and is not looking at a pane that is itself blocked
/// on input. A client that has a popup will show this question when its
/// current set is done: that is the queue. A client on a `needs_input` pane
/// is dealing with that agent's own dialog — the one it just went to with
/// `p`, typically — and a popup over it would defeat the trip.
pub fn show(t: &dyn Tmux, pane: &str) {
    show_from(t, pane, &store::load_all());
}

fn show_from(t: &dyn Tmux, pane: &str, records: &[PaneRecord]) {
    let Some(q) = records
        .iter()
        .find(|r| r.pane == pane)
        .and_then(|r| r.question.clone())
    else {
        return;
    };
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "perch".into());
    // A client on some *other* pane that is blocked on input is dealing with
    // that agent's own dialog; the asking pane's own client gets the popup —
    // that is the dialog, right where the question was asked.
    let blocked = |p: &str| {
        p != pane
            && records
                .iter()
                .any(|r| r.pane == p && r.state == crate::model::State::NeedsInput)
    };
    let mut done: HashSet<String> = HashSet::new();
    for v in t.client_views() {
        if v.client.is_empty() || !done.insert(v.client.clone()) {
            continue;
        }
        if client_has_popup(&v.client) || blocked(&v.pane) {
            continue;
        }
        let (w, h) = popup_size(&q, t.client_size(&v.client));
        t.batch(&[launch_command(&exe, &v.client, pane, w, h)]);
    }
}

/// Open the form popup for `pane`'s question on one named client: the caller
/// has just moved that client to the pane. Skipped when that client already
/// has a popup (which will show the question itself).
pub fn show_on_client(t: &dyn Tmux, pane: &str, client: &str) {
    if !crate::config::load().ask.enabled {
        return;
    }
    let Some(q) = store::load(pane).and_then(|r| r.question) else {
        return;
    };
    if client_has_popup(client) {
        return;
    }
    let (w, h) = popup_size(&q, t.client_size(client));
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "perch".into());
    t.batch(&[launch_command(&exe, client, pane, w, h)]);
}

/// Reopen a popup for the oldest question still waiting for one — live, with
/// no popup showing it — on clients that have none and are free. Called from `perch
/// seen`, i.e. from the tmux hooks on every window or pane switch, after the
/// seen rule has handed back what you are looking at.
pub fn reopen(t: &dyn Tmux) {
    let records = store::load_all();
    if let Some(rec) = next_pending(&records, Utc::now(), &HashSet::new()) {
        let pane = rec.pane.clone();
        show_from(t, &pane, &records);
    }
}

/// Ask a detached `perch ask-popup` to open the form, off the hook's path.
/// Under `PERCH_NO_TMUX` the request is logged, like the card's.
pub fn spawn_detached(pane: &str) {
    if !crate::tmux::enabled() {
        crate::tmux::NullTmux::empty().batch(&[vec!["ask-popup".into(), pane.into()]]);
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .args(["ask-popup", "--pane", pane])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// How one question set ended in the popup.
enum Outcome {
    /// Answered, handed back, or gone: on to the next set.
    Next,
    /// The user went to the pane: the popup is over.
    Left,
}

/// The popup body: the form for `pane`'s question, then for every other set
/// that arrives or is already waiting, one after another in the same popup,
/// until the queue is empty, the user goes to a pane, or the popup is closed.
pub fn form_body(t: &dyn Tmux, pane: &str, client: &str) -> anyhow::Result<()> {
    let dir = popups_dir();
    if !claim_popup(&dir, client, std::process::id()) {
        // Another popup owns this client; it will pick the question up.
        return Ok(());
    }
    let result = (|| {
        enable_raw_mode()?;
        let mut out = io::stdout();
        crossterm::execute!(out, EnterAlternateScreen)?;
        let mut term = Terminal::new(ratatui::backend::CrosstermBackend::new(out))?;
        let mut handled: HashSet<String> = HashSet::new();
        let mut current = Some(pane.to_string());
        while let Some(p) = current.take() {
            handled.insert(p.clone());
            match form_loop(&mut term, t, &p, client, &handled)? {
                Outcome::Left => break,
                Outcome::Next => {}
            }
            current =
                next_pending(&store::load_all(), Utc::now(), &handled).map(|r| r.pane.clone());
        }
        disable_raw_mode()?;
        crossterm::execute!(term.backend_mut(), LeaveAlternateScreen)?;
        term.show_cursor()?;
        Ok(())
    })();
    release_popup(&dir, client);
    result
}

/// One question set: draw until it is answered, put away, deferred, or gone.
fn form_loop<B: ratatui::backend::Backend>(
    term: &mut Terminal<B>,
    t: &dyn Tmux,
    pane: &str,
    client: &str,
    handled: &HashSet<String>,
) -> anyhow::Result<Outcome> {
    let Some(rec) = store::load(pane) else {
        return Ok(Outcome::Next);
    };
    let mut app = App::with_records(vec![rec]);
    app.theme = tui::load_theme();
    app.normalize();
    app.open_form();
    if app.form.is_none() {
        return Ok(Outcome::Next);
    }
    let count_waiting = |handled: &HashSet<String>| {
        let now = Utc::now();
        store::load_all()
            .iter()
            .filter(|r| !handled.contains(&r.pane))
            .filter(|r| r.live_question(now).is_some())
            .count()
    };
    app.waiting = count_waiting(handled);
    let mut last_check = Instant::now();
    loop {
        term.draw(|f| tui::render_form(f, &app))?;
        if event::poll(Duration::from_millis(200))? {
            if let CEvent::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match app.form_key(k.code) {
                    FormEvent::Submit(ans) => {
                        store::write_answer(&ans)?;
                        return Ok(Outcome::Next);
                    }
                    // `p`: hand it to Claude and go there.
                    FormEvent::Defer(ans) => {
                        store::defer_and_wait(&ans.pane, &ans.tool_use_id);
                        let _ = tui::jump_to(t, client, &ans.pane);
                        return Ok(Outcome::Left);
                    }
                    // Esc: hand it to Claude, stay here, on to the next set.
                    FormEvent::HandBack(ans) => {
                        store::defer_and_wait(&ans.pane, &ans.tool_use_id);
                        return Ok(Outcome::Next);
                    }
                    FormEvent::None => {}
                }
            }
        }
        if last_check.elapsed() >= Duration::from_millis(250) {
            last_check = Instant::now();
            match store::load(pane) {
                Some(rec) => app.refresh(vec![rec]),
                None => return Ok(Outcome::Next),
            }
            if app.form.is_none() {
                return Ok(Outcome::Next);
            }
            app.waiting = count_waiting(handled);
        }
    }
}
