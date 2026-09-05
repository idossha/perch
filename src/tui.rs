//! The ratatui dashboard, run inside `tmux display-popup`.
//!
//! Rows are grouped by project by default and each state carries a glyph as
//! well as a colour, so the board still reads on a monochrome terminal.

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};

use crate::model::{PaneRecord, State, Subagent};
use crate::sound;
use crate::store;
use crate::tmux::Tmux;

// ---------------------------------------------------------------- theme

/// Which palette the dashboard paints with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Dark,
    Light,
}

/// Colours for one palette. Everything is an indexed colour so the board looks
/// the same on a nightfox/carbonfox-style terminal as on a plain xterm.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub kind: ThemeKind,
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
    needs_input: Color,
    done: Color,
    working: Color,
    starting: Color,
    idle: Color,
    ended: Color,
}

pub const DARK: Theme = Theme {
    kind: ThemeKind::Dark,
    text: Color::Indexed(252),
    dim: Color::Indexed(245),
    accent: Color::Indexed(111),
    needs_input: Color::Indexed(203),
    done: Color::Indexed(114),
    working: Color::Indexed(179),
    starting: Color::Indexed(80),
    idle: Color::Indexed(68),
    ended: Color::Indexed(240),
};

pub const LIGHT: Theme = Theme {
    kind: ThemeKind::Light,
    text: Color::Indexed(235),
    dim: Color::Indexed(243),
    accent: Color::Indexed(25),
    needs_input: Color::Indexed(160),
    done: Color::Indexed(28),
    working: Color::Indexed(130),
    starting: Color::Indexed(30),
    idle: Color::Indexed(25),
    ended: Color::Indexed(246),
};

impl Default for Theme {
    fn default() -> Self {
        DARK
    }
}

impl Theme {
    pub fn by_name(name: &str) -> Theme {
        match name.trim().to_ascii_lowercase().as_str() {
            "light" => LIGHT,
            _ => DARK,
        }
    }

    pub fn color(&self, state: State) -> Color {
        match state {
            State::NeedsInput => self.needs_input,
            State::Done => self.done,
            State::Working => self.working,
            State::Starting => self.starting,
            State::Idle => self.idle,
            State::Ended => self.ended,
        }
    }

    /// Style for the glyph + state name cell.
    pub fn state_style(&self, state: State) -> Style {
        let base = Style::default().fg(self.color(state));
        match state {
            State::NeedsInput | State::Done => base.add_modifier(Modifier::BOLD),
            State::Idle | State::Ended => base.add_modifier(Modifier::DIM),
            _ => base,
        }
    }
}

/// The glyph for a state; `working` animates through a 4-frame spinner.
pub fn state_glyph(state: State, tick: u64) -> &'static str {
    const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];
    match state {
        State::NeedsInput => "⚑",
        State::Done => "✓",
        State::Working => {
            if tick == 0 {
                "▶"
            } else {
                SPIN[(tick as usize) % 4]
            }
        }
        State::Starting => "…",
        State::Idle => "·",
        State::Ended => "✕",
    }
}

/// Theme chosen by `PERCH_THEME`, else `[tui] theme` in config.toml, else dark.
///
/// The config file is parsed leniently as a raw toml value so this never fights
/// with the typed `Config` struct.
pub fn load_theme() -> Theme {
    if let Ok(v) = std::env::var("PERCH_THEME") {
        if !v.trim().is_empty() {
            return Theme::by_name(&v);
        }
    }
    let name = std::fs::read_to_string(crate::config::config_path())
        .ok()
        .and_then(|s| s.parse::<toml::Value>().ok())
        .and_then(|v| {
            v.get("tui")
                .and_then(|t| t.get("theme"))
                .and_then(|t| t.as_str())
                .map(String::from)
        });
    name.map(|n| Theme::by_name(&n)).unwrap_or(DARK)
}

// ---------------------------------------------------------------- rows

/// One line of the board. Only `Pane` and `Child` can hold the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// Project group header, with the index of every record in the group.
    Header {
        project: String,
        members: Vec<usize>,
    },
    Pane(usize),
    /// (record index, child index)
    Child(usize, usize),
    /// "N ended, press e to show"
    EndedNote(usize),
}

impl RowKind {
    pub fn selectable(&self) -> bool {
        matches!(self, RowKind::Pane(_) | RowKind::Child(_, _))
    }

    /// Index of the record this row belongs to; a child resolves to its parent.
    pub fn record(&self) -> Option<usize> {
        match self {
            RowKind::Pane(i) | RowKind::Child(i, _) => Some(*i),
            _ => None,
        }
    }
}

/// Project label for a record: `project`, else the cwd basename, else a stub.
pub fn project_of(rec: &PaneRecord) -> String {
    if let Some(p) = rec.project.as_ref().filter(|p| !p.is_empty()) {
        return p.clone();
    }
    if let Some(cwd) = rec.cwd.as_ref() {
        if let Some(base) = cwd.trim_end_matches('/').rsplit('/').next() {
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    "(no project)".to_string()
}

pub struct App {
    pub records: Vec<PaneRecord>,
    /// Index into [`App::rows`], not into `records`.
    pub selected: usize,
    pub muted: bool,
    /// Detected harnesses with no perch hook, shown as a top banner.
    pub unwired: Vec<String>,
    pub grouped: bool,
    pub show_ended: bool,
    pub show_help: bool,
    pub theme: Theme,
    /// Refresh counter, drives the working spinner.
    pub tick: u64,
}

impl App {
    /// A pure constructor: no config, sound or setup probing. Used by tests.
    pub fn with_records(records: Vec<PaneRecord>) -> Self {
        App {
            records,
            selected: 0,
            muted: false,
            unwired: Vec::new(),
            grouped: true,
            show_ended: false,
            show_help: false,
            theme: DARK,
            tick: 0,
        }
    }

    pub fn new(records: Vec<PaneRecord>) -> Self {
        let paths = crate::paths::Paths::from_env();
        let prefs = load_prefs();
        let mut app = App::with_records(records);
        app.muted = sound::is_muted();
        app.theme = load_theme();
        app.grouped = prefs.0;
        app.show_ended = prefs.1;
        app.unwired = if crate::setup::needs_nudge(&paths) {
            crate::setup::unwired_harnesses(&paths)
                .into_iter()
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };
        app.selected = 0;
        app
    }

    fn visible_children<'a>(&self, rec: &'a PaneRecord) -> Vec<(usize, &'a Subagent)> {
        rec.children
            .iter()
            .enumerate()
            .filter(|(_, c)| self.show_ended || c.state != State::Ended)
            .collect()
    }

    fn shown_records(&self) -> Vec<usize> {
        (0..self.records.len())
            .filter(|i| self.show_ended || self.records[*i].state != State::Ended)
            .collect()
    }

    pub fn hidden_ended(&self) -> usize {
        if self.show_ended {
            return 0;
        }
        self.records
            .iter()
            .filter(|r| r.state == State::Ended)
            .count()
    }

    /// The board as lines, in display order.
    pub fn rows(&self) -> Vec<RowKind> {
        let mut out = Vec::new();
        let mut idx = self.shown_records();
        if self.grouped {
            // Group by project, groups ordered by their most urgent member.
            let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
            for i in idx {
                let p = project_of(&self.records[i]);
                match groups.iter_mut().find(|(name, _)| *name == p) {
                    Some((_, v)) => v.push(i),
                    None => groups.push((p, vec![i])),
                }
            }
            for (_, members) in groups.iter_mut() {
                members.sort_by(|a, b| self.order(*a, *b));
            }
            groups.sort_by(|a, b| {
                let ra = a.1.iter().map(|i| self.records[*i].state.rank()).min();
                let rb = b.1.iter().map(|i| self.records[*i].state.rank()).min();
                ra.cmp(&rb).then_with(|| a.0.cmp(&b.0))
            });
            for (project, members) in groups {
                out.push(RowKind::Header {
                    project,
                    members: members.clone(),
                });
                for i in members {
                    self.push_pane(&mut out, i);
                }
            }
        } else {
            // Flat and chronological: newest state change first.
            idx.sort_by(|a, b| {
                self.records[*b]
                    .since
                    .cmp(&self.records[*a].since)
                    .then_with(|| self.records[*a].pane.cmp(&self.records[*b].pane))
            });
            for i in idx {
                self.push_pane(&mut out, i);
            }
        }
        let hidden = self.hidden_ended();
        if hidden > 0 {
            out.push(RowKind::EndedNote(hidden));
        }
        out
    }

    /// Within a group: urgency first, then the most recent change first.
    fn order(&self, a: usize, b: usize) -> std::cmp::Ordering {
        let (x, y) = (&self.records[a], &self.records[b]);
        x.state
            .rank()
            .cmp(&y.state.rank())
            .then_with(|| y.since.cmp(&x.since))
            .then_with(|| x.pane.cmp(&y.pane))
    }

    fn push_pane(&self, out: &mut Vec<RowKind>, i: usize) {
        out.push(RowKind::Pane(i));
        for (ci, _) in self.visible_children(&self.records[i]) {
            out.push(RowKind::Child(i, ci));
        }
    }

    fn selectable_positions(&self, rows: &[RowKind]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, r)| r.selectable())
            .map(|(i, _)| i)
            .collect()
    }

    /// Move the cursor by `delta` selectable rows, skipping headers and notes.
    pub fn move_by(&mut self, delta: isize) {
        let rows = self.rows();
        let sel = self.selectable_positions(&rows);
        if sel.is_empty() {
            self.selected = 0;
            return;
        }
        let cur = sel.iter().position(|p| *p == self.selected).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(sel.len() as isize) as usize;
        self.selected = sel[next];
    }

    /// Put the cursor on the first selectable row if it is not on one already.
    pub fn normalize(&mut self) {
        let rows = self.rows();
        let sel = self.selectable_positions(&rows);
        if !sel.contains(&self.selected) {
            self.selected = sel.first().copied().unwrap_or(0);
        }
    }

    /// The record under the cursor; a subagent row resolves to its pane.
    pub fn current(&self) -> Option<&PaneRecord> {
        let rows = self.rows();
        rows.get(self.selected)
            .and_then(|r| r.record())
            .and_then(|i| self.records.get(i))
    }

    /// Row index of the oldest pane waiting on the human (needs_input, done).
    pub fn next_waiting(&self) -> Option<usize> {
        let rows = self.rows();
        rows.iter().enumerate().position(|(_, r)| match r {
            RowKind::Pane(i) => matches!(self.records[*i].state, State::NeedsInput | State::Done),
            _ => false,
        })
    }

    /// Replace the record list, keeping the cursor on the same pane if it lives.
    pub fn refresh(&mut self, records: Vec<PaneRecord>) {
        let keep = self.current().map(|r| r.pane.clone());
        self.records = records;
        self.tick = self.tick.wrapping_add(1);
        let rows = self.rows();
        self.selected = keep
            .and_then(|p| {
                rows.iter().position(|r| {
                    matches!(r, RowKind::Pane(i) if self.records.get(*i).map(|x| &x.pane) == Some(&p))
                })
            })
            .unwrap_or(0);
        self.normalize();
    }
}

// ---------------------------------------------------------------- prefs

fn prefs_path() -> std::path::PathBuf {
    store::state_dir().join("tui.json")
}

/// `(grouped, show_ended)`, defaulting to grouped with ended hidden.
fn load_prefs() -> (bool, bool) {
    let v = std::fs::read_to_string(prefs_path())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    match v {
        Some(v) => (
            v.get("grouped").and_then(|b| b.as_bool()).unwrap_or(true),
            v.get("show_ended")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
        ),
        None => (true, false),
    }
}

/// Best effort: a failure here must never take the dashboard down.
fn save_prefs(app: &App) {
    let path = prefs_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = serde_json::json!({ "grouped": app.grouped, "show_ended": app.show_ended });
    let _ = std::fs::write(path, body.to_string());
}

// ---------------------------------------------------------------- render

/// The cells of one pane row, in column order. Kept for `perch status`.
pub fn row_cells(rec: &PaneRecord, now: DateTime<Utc>) -> [String; 6] {
    let project = match (&rec.project, &rec.branch) {
        (Some(p), Some(b)) => format!("{p} ({b})"),
        (Some(p), None) => p.clone(),
        _ => "-".to_string(),
    };
    [
        rec.pane.clone(),
        project,
        rec.harness.as_str().to_string(),
        rec.state.as_str().to_string(),
        store::fmt_age(store::age_secs(&rec.since, now)),
        rec.last_message.clone().unwrap_or_default(),
    ]
}

const W_PANE: usize = 6;
const W_PROJECT: usize = 20;
const W_HARNESS: usize = 10;
const W_STATE: usize = 13;
const W_AGE: usize = 5;

/// Pad to `w`, or truncate with an ellipsis.
fn fit(s: &str, w: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= w {
        format!("{s:<w$}")
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = chars[..w - 1].iter().collect();
        out.push('…');
        out
    }
}

/// One line, no newlines, truncated with an ellipsis.
fn one_line(s: &str, w: usize) -> String {
    let flat: String = s
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let chars: Vec<char> = flat.trim().chars().collect();
    if chars.len() <= w {
        chars.into_iter().collect()
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = chars[..w - 1].iter().collect();
        out.push('…');
        out
    }
}

fn counts_label(app: &App, members: &[usize]) -> String {
    let mut parts = Vec::new();
    for st in [
        State::NeedsInput,
        State::Done,
        State::Working,
        State::Starting,
        State::Idle,
        State::Ended,
    ] {
        let n = members
            .iter()
            .filter(|i| app.records[**i].state == st)
            .count();
        if n > 0 {
            parts.push(format!("{}{}", state_glyph(st, 0), n));
        }
    }
    parts.join(" ")
}

fn header_line(theme: &Theme, msg_w: usize) -> Line<'static> {
    let dim = Style::default().fg(theme.dim).add_modifier(Modifier::DIM);
    let text = format!(
        "  {}{}{}{}{:>aw$} {}",
        fit("pane", W_PANE),
        fit("project", W_PROJECT),
        fit("harness", W_HARNESS),
        fit("state", W_STATE),
        "age",
        fit("last message", msg_w),
        aw = W_AGE,
    );
    Line::from(Span::styled(text, dim))
}

fn pane_line(
    app: &App,
    i: usize,
    selected: bool,
    now: DateTime<Utc>,
    msg_w: usize,
) -> Line<'static> {
    let rec = &app.records[i];
    let t = &app.theme;
    let marker = if selected { "▌ " } else { "  " };
    let pick = Style::default().fg(t.text);
    let sel = if selected {
        pick.add_modifier(Modifier::REVERSED)
    } else {
        pick
    };
    let kids = rec.children.len();
    let harness = if kids > 0 {
        format!("{} +{}", rec.harness.as_str(), kids)
    } else {
        rec.harness.as_str().to_string()
    };
    let state_txt = format!(
        "{} {}",
        state_glyph(
            rec.state,
            if rec.state == State::Working {
                app.tick
            } else {
                0
            }
        ),
        rec.state.as_str()
    );
    Line::from(vec![
        Span::styled(marker.to_string(), Style::default().fg(t.accent)),
        Span::styled(fit(&rec.pane, W_PANE), sel),
        Span::styled(fit(&project_of(rec), W_PROJECT), sel),
        Span::styled(fit(&harness, W_HARNESS), Style::default().fg(t.dim)),
        Span::styled(fit(&state_txt, W_STATE), t.state_style(rec.state)),
        Span::styled(
            format!(
                "{:>w$} ",
                store::fmt_age(store::age_secs(&rec.since, now)),
                w = W_AGE
            ),
            Style::default().fg(t.dim),
        ),
        Span::styled(
            one_line(rec.last_message.as_deref().unwrap_or(""), msg_w),
            Style::default().fg(t.text),
        ),
    ])
}

fn child_line(
    app: &App,
    i: usize,
    ci: usize,
    selected: bool,
    now: DateTime<Utc>,
    msg_w: usize,
) -> Line<'static> {
    let kid = &app.records[i].children[ci];
    let t = &app.theme;
    let name = kid
        .agent_type
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| kid.id.chars().take(8).collect());
    let marker = if selected { "▌ " } else { "  " };
    let label = format!("└ {name}");
    let lstyle = {
        let b = Style::default().fg(t.dim);
        if selected {
            b.add_modifier(Modifier::REVERSED)
        } else {
            b
        }
    };
    Line::from(vec![
        Span::styled(marker.to_string(), Style::default().fg(t.accent)),
        Span::styled(fit(&label, W_PANE + W_PROJECT + W_HARNESS), lstyle),
        Span::styled(
            fit(
                &format!("{} {}", state_glyph(kid.state, 0), kid.state.as_str()),
                W_STATE,
            ),
            t.state_style(kid.state),
        ),
        Span::styled(
            format!(
                "{:>w$} ",
                store::fmt_age(store::age_secs(&kid.since, now)),
                w = W_AGE
            ),
            Style::default().fg(t.dim),
        ),
        Span::styled(
            one_line(kid.last_message.as_deref().unwrap_or(""), msg_w),
            Style::default().fg(t.dim),
        ),
    ])
}

pub fn render(f: &mut Frame, app: &App, now: DateTime<Utc>) {
    let banner = u16::from(!app.unwired.is_empty());
    let help = 1 + u16::from(app.show_help);
    let areas = Layout::vertical([
        Constraint::Length(banner),
        Constraint::Min(3),
        Constraint::Length(help),
    ])
    .split(f.area());
    let t = &app.theme;

    if banner == 1 {
        let text = format!(
            "perch is not wired into {}: press S to run setup",
            app.unwired.join(", ")
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default()
                    .fg(t.needs_input)
                    .add_modifier(Modifier::BOLD),
            ))),
            areas[0],
        );
    }

    let inner_w = areas[1].width.saturating_sub(2) as usize;
    let fixed = 2 + W_PANE + W_PROJECT + W_HARNESS + W_STATE + W_AGE + 1;
    let msg_w = inner_w.saturating_sub(fixed).max(1);

    let rows = app.rows();
    let mut lines: Vec<Line> = vec![header_line(t, msg_w)];
    for (n, row) in rows.iter().enumerate() {
        let selected = n == app.selected;
        lines.push(match row {
            RowKind::Header { project, members } => Line::from(vec![
                Span::styled(
                    format!("▸ {project}  "),
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(counts_label(app, members), Style::default().fg(t.dim)),
            ]),
            RowKind::Pane(i) => pane_line(app, *i, selected, now, msg_w),
            RowKind::Child(i, ci) => child_line(app, *i, *ci, selected, now, msg_w),
            RowKind::EndedNote(k) => Line::from(Span::styled(
                format!("  {k} ended, press e to show"),
                Style::default().fg(t.ended).add_modifier(Modifier::DIM),
            )),
        });
    }

    // Scroll so the cursor line stays on screen.
    let view = areas[1].height.saturating_sub(2) as usize;
    let cursor = app.selected + 1;
    let offset = if view > 0 && cursor >= view {
        cursor + 1 - view
    } else {
        0
    };

    let table = Paragraph::new(lines).scroll((offset as u16, 0)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.dim))
            .title(Span::styled(
                "perch",
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(table, areas[1]);

    let mute = if app.muted { "muted" } else { "sound on" };
    let mode = if app.grouped { "grouped" } else { "flat" };
    let mut foot = vec![Line::from(vec![
        Span::styled(
            "j/k move  Enter jump  n next  m mute  x dismiss  e ended  g group  r refresh  ? help  q quit  [",
            Style::default().fg(t.dim),
        ),
        Span::styled(format!("{mute} · {mode}"), Style::default().fg(t.accent)),
        Span::styled("]", Style::default().fg(t.dim)),
    ])];
    if app.show_help {
        foot.push(Line::from(Span::styled(
            "keys: j/k down/up  Enter jump to pane  n next waiting  m mute  x dismiss done  e show/hide ended  g grouped/flat  r refresh  S setup  q quit",
            Style::default().fg(t.dim),
        )));
    }
    f.render_widget(Paragraph::new(foot), areas[2]);
}

/// Run the dashboard until the user quits or jumps.
pub fn run(tmux: &dyn Tmux) -> Result<()> {
    let mut app = App::new(store::snapshot(tmux));
    app.normalize();

    enable_raw_mode()?;
    let mut out = io::stdout();
    crossterm::execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(ratatui::backend::CrosstermBackend::new(out))?;

    let result = event_loop(&mut term, &mut app, tmux);

    disable_raw_mode()?;
    crossterm::execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    result
}

/// Move the calling client to a pane and mark it seen.
///
/// The pane may live in another session, so its session is resolved from the
/// live pane list and `jump` is used — `switch-client` from inside a
/// `display-popup` targets the popup's own client, which is the one the user
/// is sitting at. Everything runs synchronously: this returns straight into
/// the popup closing, and a spawned tmux child would die with the pty.
pub fn jump_to(tmux: &dyn Tmux, pane: &str) {
    match tmux
        .list_panes()
        .into_iter()
        .find(|p| p.pane == pane)
        .map(|p| p.session)
    {
        Some(session) => tmux.jump(&session, pane),
        None => tmux.focus(pane),
    }
    crate::hook::seen(pane);
}

fn event_loop<B: Backend>(term: &mut Terminal<B>, app: &mut App, tmux: &dyn Tmux) -> Result<()> {
    let mut last_refresh = Instant::now();
    loop {
        term.draw(|f| render(f, app, Utc::now()))?;

        if event::poll(Duration::from_millis(250))? {
            if let CEvent::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('j') | KeyCode::Down => app.move_by(1),
                    KeyCode::Char('k') | KeyCode::Up => app.move_by(-1),
                    KeyCode::Char('n') => {
                        if let Some(i) = app.next_waiting() {
                            app.selected = i;
                        }
                    }
                    KeyCode::Char('m') => app.muted = sound::toggle_mute(),
                    KeyCode::Char('e') => {
                        app.show_ended = !app.show_ended;
                        app.normalize();
                        save_prefs(app);
                    }
                    KeyCode::Char('g') => {
                        app.grouped = !app.grouped;
                        app.normalize();
                        save_prefs(app);
                    }
                    KeyCode::Char('?') => app.show_help = !app.show_help,
                    KeyCode::Char('x') => {
                        if let Some(rec) = app.current() {
                            if rec.state == State::Done {
                                let mut r = rec.clone();
                                r.state = State::Idle;
                                r.since = store::now_rfc3339();
                                let _ = store::save(&r);
                                app.refresh(store::snapshot(tmux));
                            }
                        }
                    }
                    KeyCode::Char('S') => {
                        let paths = crate::paths::Paths::from_env();
                        let _ = crate::setup::run(&paths, &crate::setup::SetupOpts::default());
                        app.unwired = if crate::setup::needs_nudge(&paths) {
                            crate::setup::unwired_harnesses(&paths)
                                .into_iter()
                                .map(String::from)
                                .collect()
                        } else {
                            Vec::new()
                        };
                        app.refresh(store::snapshot(tmux));
                    }
                    KeyCode::Char('r') => app.refresh(store::snapshot(tmux)),
                    KeyCode::Enter => {
                        if let Some(rec) = app.current() {
                            jump_to(tmux, &rec.pane.clone());
                        }
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
        if last_refresh.elapsed() >= Duration::from_secs(1) {
            app.refresh(store::snapshot(tmux));
            app.muted = sound::is_muted();
            last_refresh = Instant::now();
        }
    }
}
