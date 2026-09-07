//! The ratatui dashboard, run inside `tmux display-popup`.
//!
//! Rows are grouped by project by default and each state carries a glyph as
//! well as a colour, so the board still reads on a monochrome terminal.

use std::collections::HashSet;
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

use crate::model::{Answer, PaneRecord, PendingQuestion, Question, State, Subagent};
use crate::sound;
use crate::store;
use crate::tmux::{LivePane, Tmux};

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

/// The state word for a pane: `delegating` when the pane's own turn has ended
/// but its background subagents have not, `working` when it is working itself.
pub fn state_word(rec: &PaneRecord) -> &'static str {
    if rec.is_delegating() {
        "delegating"
    } else if rec.is_question() {
        "question"
    } else {
        rec.state.as_str()
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

/// What the cursor is on, keyed by identity rather than by row index.
///
/// A refresh reorders rows freely (a group jumps to the top the moment one of
/// its panes needs input); a row index would silently point at a different
/// agent, which is how "Enter sent me to the wrong pane" happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub pane: String,
    /// The subagent id, when the cursor is on a child row.
    pub child: Option<String>,
}

pub struct App {
    pub records: Vec<PaneRecord>,
    /// The live tmux panes behind the records, so a row can say where it is
    /// rather than repeating a `%id` the user has never navigated by.
    pub live: Vec<LivePane>,
    /// The pane (and child) under the cursor; the row index is derived.
    pub selected: Option<Selection>,
    /// Last row index the cursor sat on, so a vanished pane falls to a
    /// neighbouring row rather than to the top of the board.
    last_row: usize,
    /// A failed jump, shown on its own line until the next action.
    pub error: Option<String>,
    pub muted: bool,
    /// Detected harnesses with no perch hook, shown as a top banner.
    pub unwired: Vec<String>,
    pub grouped: bool,
    pub show_ended: bool,
    /// Panes whose finished subagents are unfolded, keyed by pane id. Session
    /// state: a fold is a way of looking at the board, not a preference.
    pub expanded: HashSet<String>,
    pub show_help: bool,
    pub theme: Theme,
    /// Refresh counter, drives the working spinner.
    pub tick: u64,
    /// `PERCH_DEBUG=1`: show the pane id beside the location. Read once, at
    /// construction, so rendering stays a pure function of the app.
    pub debug: bool,
    /// The answer form, while one is open — in the popup body only; the
    /// board never opens one.
    pub form: Option<Form>,
    /// Other agents' question sets queued behind the open form (popup only).
    pub waiting: usize,
}

// ---------------------------------------------------------------- form

/// The answer form for one pane's pending question set.
///
/// One question at a time. A `choice` question lists its options plus an
/// `Other…` row that opens a text line; a `text` question is only the line.
/// Answers are keyed by question text, the shape Claude's tool expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub pane: String,
    pub tool_use_id: String,
    pub questions: Vec<Question>,
    /// Which question is on screen.
    pub index: usize,
    /// Row under the cursor: an option index, or `options.len()` for Other.
    pub cursor: usize,
    /// Ticked options per question.
    pub picked: Vec<HashSet<usize>>,
    /// Free text per question, when Other was used.
    pub other: Vec<Option<String>>,
    /// The Other line is open and taking characters.
    pub typing: bool,
    pub text: String,
}

/// What a key did to the form, for the popup body to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEvent {
    None,
    /// Every question visited: this is the answer to write.
    Submit(Answer),
    /// `p`: hand the question to the harness's own dialog and jump there.
    Defer(Answer),
    /// `Esc`: hand the question to the harness's own dialog, stay here.
    HandBack(Answer),
}

impl Form {
    pub fn new(pane: &str, q: &PendingQuestion) -> Form {
        let n = q.questions.len();
        Form {
            pane: pane.to_string(),
            tool_use_id: q.tool_use_id.clone(),
            questions: q.questions.clone(),
            index: 0,
            cursor: 0,
            picked: vec![HashSet::new(); n],
            other: vec![None; n],
            typing: false,
            text: String::new(),
        }
    }

    pub fn current(&self) -> &Question {
        &self.questions[self.index]
    }

    /// Rows on screen for the current question: its options and Other.
    fn rows(&self) -> usize {
        self.current().options.len() + 1
    }

    fn on_other(&self) -> bool {
        self.cursor >= self.current().options.len()
    }

    /// `true` when the question at `i` has an answer of any kind.
    pub fn answered(&self, i: usize) -> bool {
        self.other[i].as_ref().is_some_and(|t| !t.trim().is_empty()) || !self.picked[i].is_empty()
    }

    /// Show question `i`, the cursor on its first pick (or the top). Moving
    /// between tabs is not sending: past either edge stays put.
    fn goto(&mut self, i: usize) {
        if i >= self.questions.len() {
            return;
        }
        self.typing = false;
        self.text.clear();
        self.index = i;
        self.cursor = self.picked[i].iter().min().copied().unwrap_or(0);
    }

    /// Move on to the next question, or submit after the last one.
    fn advance(&mut self) -> FormEvent {
        self.typing = false;
        self.text.clear();
        if self.index + 1 < self.questions.len() {
            self.goto(self.index + 1);
            return FormEvent::None;
        }
        FormEvent::Submit(self.answer())
    }

    /// The answers so far: free text wins over ticks; a multi-select is its
    /// labels in option order joined by `, `; an untouched question is absent.
    pub fn answer(&self) -> Answer {
        let mut answers = std::collections::BTreeMap::new();
        for (i, q) in self.questions.iter().enumerate() {
            if let Some(t) = self.other[i].as_ref().filter(|t| !t.trim().is_empty()) {
                answers.insert(q.question.clone(), t.trim().to_string());
                continue;
            }
            let labels: Vec<&str> = q
                .options
                .iter()
                .enumerate()
                .filter(|(oi, _)| self.picked[i].contains(oi))
                .map(|(_, o)| o.label.as_str())
                .collect();
            if !labels.is_empty() {
                answers.insert(q.question.clone(), labels.join(", "));
            }
        }
        Answer {
            pane: self.pane.clone(),
            tool_use_id: self.tool_use_id.clone(),
            answers,
            defer: false,
        }
    }

    /// One key, in the form's own contract.
    pub fn key(&mut self, code: KeyCode) -> FormEvent {
        if self.typing {
            return match code {
                KeyCode::Char(c) => {
                    self.text.push(c);
                    FormEvent::None
                }
                KeyCode::Backspace => {
                    self.text.pop();
                    FormEvent::None
                }
                KeyCode::Esc => {
                    self.typing = false;
                    self.text.clear();
                    FormEvent::None
                }
                KeyCode::Enter => {
                    let t = std::mem::take(&mut self.text);
                    self.other[self.index] = (!t.trim().is_empty()).then_some(t);
                    self.advance()
                }
                _ => FormEvent::None,
            };
        }
        let rows = self.rows();
        match code {
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => self.goto(self.index + 1),
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                if self.index > 0 {
                    self.goto(self.index - 1);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.cursor = (self.cursor + 1) % rows,
            KeyCode::Char('k') | KeyCode::Up => self.cursor = (self.cursor + rows - 1) % rows,
            KeyCode::Char('g') => self.cursor = 0,
            KeyCode::Char('G') => self.cursor = rows - 1,
            KeyCode::Char(' ') if !self.on_other() => {
                let multi = self.current().multi_select;
                let set = &mut self.picked[self.index];
                if multi {
                    if !set.remove(&self.cursor) {
                        set.insert(self.cursor);
                    }
                } else {
                    set.clear();
                    set.insert(self.cursor);
                }
            }
            KeyCode::Enter => {
                if self.on_other() {
                    self.typing = true;
                    self.text = self.other[self.index].clone().unwrap_or_default();
                    return FormEvent::None;
                }
                if !self.current().multi_select {
                    let set = &mut self.picked[self.index];
                    set.clear();
                    set.insert(self.cursor);
                }
                return self.advance();
            }
            KeyCode::Esc => {
                return FormEvent::HandBack(Answer::defer(&self.pane, &self.tool_use_id));
            }
            KeyCode::Char('p') => {
                return FormEvent::Defer(Answer::defer(&self.pane, &self.tool_use_id));
            }
            _ => {}
        }
        FormEvent::None
    }
}

impl App {
    /// A pure constructor: no config, sound or setup probing. Used by tests.
    pub fn with_records(records: Vec<PaneRecord>) -> Self {
        App {
            records,
            live: Vec::new(),
            selected: None,
            last_row: 0,
            error: None,
            muted: false,
            unwired: Vec::new(),
            grouped: true,
            show_ended: false,
            expanded: HashSet::new(),
            show_help: false,
            theme: DARK,
            tick: 0,
            debug: false,
            form: None,
            waiting: 0,
        }
    }

    /// The question under the cursor that can still be answered from here.
    pub fn live_question_at(&self, now: DateTime<Utc>) -> Option<(&PaneRecord, &PendingQuestion)> {
        let rec = self.current()?;
        rec.live_question(now).map(|q| (rec, q))
    }

    /// `a`: open the form for the selected pane's question, if it has one.
    pub fn open_form(&mut self) {
        self.open_form_at(Utc::now());
    }

    pub fn open_form_at(&mut self, now: DateTime<Utc>) {
        if let Some((rec, q)) = self.live_question_at(now) {
            self.form = Some(Form::new(&rec.pane, q));
        }
    }

    /// One key while the form is open. `Submit` and `Defer` close it; the
    /// caller writes the answer. `Cancel` closes it and leaves the question.
    pub fn form_key(&mut self, code: KeyCode) -> FormEvent {
        let Some(form) = self.form.as_mut() else {
            return FormEvent::None;
        };
        let ev = form.key(code);
        if ev != FormEvent::None {
            self.form = None;
        }
        ev
    }

    /// Close the form if its question is no longer on the record: answered
    /// from another client, handed back, overtaken, or expired.
    fn sync_form(&mut self) {
        let now = Utc::now();
        let Some(f) = self.form.as_ref() else {
            return;
        };
        let still = self
            .records
            .iter()
            .find(|r| r.pane == f.pane)
            .and_then(|r| r.live_question(now))
            .is_some_and(|q| q.tool_use_id == f.tool_use_id);
        if !still {
            self.form = None;
        }
    }

    pub fn new(records: Vec<PaneRecord>) -> Self {
        let paths = crate::paths::Paths::from_env();
        let prefs = load_prefs();
        let mut app = App::with_records(records);
        app.muted = sound::is_muted();
        app.theme = load_theme();
        app.debug = crate::tmux::debug();
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
        app.normalize();
        app
    }

    /// The child rows for a pane: what is still running, plus — only when the
    /// pane is expanded — what has finished, most recent first.
    ///
    /// Finished children are the bulk of a big fan-out and none of them is
    /// waiting on you; they live in the pane's badge until you ask.
    fn visible_children<'a>(&self, rec: &'a PaneRecord) -> Vec<(usize, &'a Subagent)> {
        let mut out: Vec<(usize, &Subagent)> = rec
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.state.is_finished())
            .collect();
        if self.expanded.contains(&rec.pane) {
            let mut done: Vec<(usize, &Subagent)> = rec
                .children
                .iter()
                .enumerate()
                .filter(|(_, c)| c.state.is_finished())
                .collect();
            done.sort_by(|a, b| b.1.since.cmp(&a.1.since));
            out.extend(done);
        }
        out
    }

    /// Fold or unfold the finished subagents of the pane under the cursor.
    pub fn toggle_expanded(&mut self) {
        let Some(pane) = self.selected.as_ref().map(|s| s.pane.clone()) else {
            return;
        };
        if !self.expanded.remove(&pane) {
            self.expanded.insert(pane);
        }
        self.normalize();
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
                let ra =
                    a.1.iter()
                        .map(|i| self.records[*i].effective_state().rank())
                        .min();
                let rb =
                    b.1.iter()
                        .map(|i| self.records[*i].effective_state().rank())
                        .min();
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
        x.effective_state()
            .rank()
            .cmp(&y.effective_state().rank())
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

    /// The identity of whatever is on `row`, or `None` for a header or note.
    pub fn selection_at(&self, row: &RowKind) -> Option<Selection> {
        match row {
            RowKind::Pane(i) => Some(Selection {
                pane: self.records.get(*i)?.pane.clone(),
                child: None,
            }),
            RowKind::Child(i, ci) => {
                let rec = self.records.get(*i)?;
                Some(Selection {
                    pane: rec.pane.clone(),
                    child: Some(rec.children.get(*ci)?.id.clone()),
                })
            }
            _ => None,
        }
    }

    /// Where the selection currently sits in `rows`, if it is still on screen.
    pub fn selected_row(&self, rows: &[RowKind]) -> Option<usize> {
        let want = self.selected.as_ref()?;
        rows.iter()
            .position(|r| self.selection_at(r).as_ref() == Some(want))
    }

    /// Row index for rendering; falls back to the remembered row.
    pub fn cursor_row(&self, rows: &[RowKind]) -> usize {
        self.selected_row(rows).unwrap_or(self.last_row)
    }

    fn select_row(&mut self, rows: &[RowKind], row: usize) {
        self.last_row = row;
        self.selected = rows.get(row).and_then(|r| self.selection_at(r));
    }

    /// Move the cursor by `delta` selectable rows, skipping headers and notes.
    pub fn move_by(&mut self, delta: isize) {
        let rows = self.rows();
        let sel = self.selectable_positions(&rows);
        if sel.is_empty() {
            self.selected = None;
            self.last_row = 0;
            return;
        }
        let here = self.cursor_row(&rows);
        let cur = sel.iter().position(|p| *p == here).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(sel.len() as isize) as usize;
        self.select_row(&rows, sel[next]);
    }

    /// Jump the cursor to the first (`-1`) or last (`1`) selectable row.
    pub fn move_to_edge(&mut self, last: bool) {
        let rows = self.rows();
        let sel = self.selectable_positions(&rows);
        let Some(row) = (if last { sel.last() } else { sel.first() }).copied() else {
            self.selected = None;
            return;
        };
        self.select_row(&rows, row);
    }

    /// Put the cursor on a real row: the same one if it is still there, else
    /// the nearest selectable row to where it was.
    pub fn normalize(&mut self) {
        let rows = self.rows();
        let sel = self.selectable_positions(&rows);
        if sel.is_empty() {
            self.selected = None;
            self.last_row = 0;
            return;
        }
        if let Some(row) = self.selected_row(&rows) {
            self.last_row = row;
            return;
        }
        let here = self.last_row;
        let nearest = sel
            .iter()
            .copied()
            .min_by_key(|p| p.abs_diff(here))
            .unwrap_or(sel[0]);
        self.select_row(&rows, nearest);
    }

    /// The record under the cursor; a subagent row resolves to its pane.
    pub fn current(&self) -> Option<&PaneRecord> {
        let want = &self.selected.as_ref()?.pane;
        self.records.iter().find(|r| &r.pane == want)
    }

    /// The pane id `Enter` would move the client to — a child jumps to its
    /// parent pane, because a subagent has no pane of its own.
    pub fn jump_target(&self) -> Option<String> {
        self.selected.as_ref().map(|s| s.pane.clone())
    }

    /// The oldest pane waiting on the human: `needs_input` first, then `done`.
    pub fn next_waiting(&self) -> Option<Selection> {
        for want in [State::NeedsInput, State::Done] {
            let oldest = self
                .records
                .iter()
                .filter(|r| r.effective_state() == want)
                .min_by(|a, b| a.since.cmp(&b.since));
            if let Some(r) = oldest {
                return Some(Selection {
                    pane: r.pane.clone(),
                    child: None,
                });
            }
        }
        None
    }

    /// Replace the record list. The selection is by identity, so a reorder
    /// cannot move it to another pane; only a pane that is gone moves it.
    pub fn refresh(&mut self, records: Vec<PaneRecord>) {
        self.records = records;
        self.tick = self.tick.wrapping_add(1);
        self.normalize();
        self.sync_form();
    }

    /// [`App::refresh`] with the pane list that snapshot reconciled against.
    pub fn refresh_live(&mut self, snap: (Vec<PaneRecord>, Vec<LivePane>)) {
        self.live = snap.1;
        self.refresh(snap.0);
    }

    /// `true` when the live panes span more than one session, which is the
    /// only time a location needs a `<session>/` prefix to be unambiguous.
    pub fn multi_session(&self) -> bool {
        let mut first: Option<&str> = None;
        for p in &self.live {
            match first {
                None => first = Some(&p.session),
                Some(s) if s != p.session => return true,
                Some(_) => {}
            }
        }
        false
    }

    /// Where a record's pane sits in tmux: the live location, else the last
    /// one the hook stored, else nothing to say.
    pub fn location_of(&self, rec: &PaneRecord) -> String {
        if let Some(p) = self.live.iter().find(|p| p.pane == rec.pane) {
            return p.location(self.multi_session());
        }
        rec.location.clone().unwrap_or_else(|| "—".to_string())
    }

    /// Width of the harness column: wide enough for the widest badge.
    fn harness_width(&self) -> usize {
        let w = self
            .records
            .iter()
            .map(|r| badge_text(r).chars().count())
            .max()
            .unwrap_or(0);
        w.max(W_HARNESS - 2) + 2
    }

    /// Width of the model column: the widest label plus a gap, and nothing
    /// at all when no pane has reported a model.
    fn model_width(&self) -> usize {
        let w = self
            .records
            .iter()
            .map(|r| r.model_cell().chars().count())
            .max()
            .unwrap_or(0);
        if w == 0 {
            0
        } else {
            w.min(W_MODEL_MAX) + 2
        }
    }

    /// Width of the location column: the widest one, capped so a long window
    /// name cannot eat the message.
    fn location_width(&self) -> usize {
        let w = self
            .records
            .iter()
            .map(|r| {
                let id = if self.debug {
                    r.pane.chars().count() + 1
                } else {
                    0
                };
                self.location_of(r).chars().count() + id
            })
            .max()
            .unwrap_or(0);
        w.clamp(W_LOC_MIN, W_LOC_MAX) + 2
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
pub fn row_cells(rec: &PaneRecord, now: DateTime<Utc>) -> [String; 7] {
    let project = match (&rec.project, &rec.branch) {
        (Some(p), Some(b)) => format!("{p} ({b})"),
        (Some(p), None) => p.clone(),
        _ => "-".to_string(),
    };
    [
        rec.pane.clone(),
        project,
        rec.harness.as_str().to_string(),
        rec.model_cell(),
        state_word(rec).to_string(),
        store::fmt_age(store::age_secs(&rec.since, now)),
        rec.last_message.clone().unwrap_or_default(),
    ]
}

/// The location column is adaptive: as wide as its widest row, within these
/// bounds, plus a two-column gap.
const W_LOC_MIN: usize = 8;
const W_LOC_MAX: usize = 24;
const W_PROJECT: usize = 20;
const W_HARNESS: usize = 10;
const W_MODEL_MAX: usize = 22;
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

/// A group header: `<project> (<branch>)`, and the branch only when every
/// member of the group agrees on one — a worktree per branch is common.
fn group_label(app: &App, project: &str, members: &[usize]) -> String {
    let mut branch: Option<&str> = None;
    for i in members {
        match (
            branch,
            app.records[*i].branch.as_deref().filter(|b| !b.is_empty()),
        ) {
            (_, None) => return project.to_string(),
            (None, Some(b)) => branch = Some(b),
            (Some(a), Some(b)) if a != b => return project.to_string(),
            (Some(_), Some(_)) => {}
        }
    }
    match branch {
        Some(b) => format!("{project} ({b})"),
        None => project.to_string(),
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
            .filter(|i| app.records[**i].effective_state() == st)
            .count();
        if n > 0 {
            parts.push(format!("{}{}", state_glyph(st, 0), n));
        }
    }
    parts.join(" ")
}

/// Counts of a pane's subagents: `(needs_input, working, finished)`.
fn child_counts(rec: &PaneRecord) -> (usize, usize, usize) {
    let mut n = (0, 0, 0);
    for c in &rec.children {
        match c.state {
            State::NeedsInput => n.0 += 1,
            State::Working | State::Starting => n.1 += 1,
            // `idle` is not a state a subagent reaches; count it with the rest.
            State::Done | State::Ended | State::Idle => n.2 += 1,
        }
    }
    n
}

/// The harness cell as plain text: `claude ⚑1 ▶4 ✓48`, zero parts omitted.
///
/// Finished subagents are a count, not forty-eight rows. Only the two urgent
/// parts are ever news; the rest is history, one keystroke away.
pub fn badge_text(rec: &PaneRecord) -> String {
    let (blocked, working, done) = child_counts(rec);
    let mut out = rec.harness.as_str().to_string();
    for (n, glyph) in [(blocked, "⚑"), (working, "▶"), (done, "✓")] {
        if n > 0 {
            out.push_str(&format!(" {glyph}{n}"));
        }
    }
    out
}

/// [`badge_text`] as coloured spans, padded to `w`.
fn badge_spans(t: &Theme, rec: &PaneRecord, w: usize) -> Vec<Span<'static>> {
    let (blocked, working, done) = child_counts(rec);
    let dim = Style::default().fg(t.dim);
    let mut spans = vec![Span::styled(rec.harness.as_str().to_string(), dim)];
    for (n, glyph, style) in [
        (
            blocked,
            "⚑",
            Style::default()
                .fg(t.needs_input)
                .add_modifier(Modifier::BOLD),
        ),
        (working, "▶", Style::default().fg(t.working)),
        (done, "✓", dim.add_modifier(Modifier::DIM)),
    ] {
        if n > 0 {
            spans.push(Span::styled(format!(" {glyph}{n}"), style));
        }
    }
    let used = badge_text(rec).chars().count();
    spans.push(Span::raw(" ".repeat(w.saturating_sub(used))));
    spans
}

/// The computed column widths of one draw, passed around as a unit.
#[derive(Debug, Clone, Copy)]
struct Cols {
    loc: usize,
    project: usize,
    harness: usize,
    model: usize,
    msg: usize,
}

fn header_line(theme: &Theme, c: Cols) -> Line<'static> {
    let dim = Style::default().fg(theme.dim).add_modifier(Modifier::DIM);
    let text = format!(
        "  {}{}{}{}{}{:>aw$} {}",
        fit("location", c.loc),
        fit("project", c.project),
        fit("harness", c.harness),
        fit("model", c.model),
        fit("state", W_STATE),
        "age",
        fit("last message", c.msg),
        aw = W_AGE,
    );
    Line::from(Span::styled(text, dim))
}

fn pane_line(app: &App, i: usize, selected: bool, now: DateTime<Utc>, c: Cols) -> Line<'static> {
    let rec = &app.records[i];
    let t = &app.theme;
    let marker = if selected { "▌ " } else { "  " };
    let pick = Style::default().fg(t.text);
    let sel = if selected {
        pick.add_modifier(Modifier::REVERSED)
    } else {
        pick
    };
    let shown = rec.effective_state();
    let state_txt = format!(
        "{} {}",
        state_glyph(shown, if shown == State::Working { app.tick } else { 0 }),
        state_word(rec)
    );
    // The pane id is the internal key, not something a user navigates by; it
    // is on screen only when they asked to debug, and then inside the
    // location cell so the columns to its right do not move.
    let location = if app.debug {
        format!("{} {}", app.location_of(rec), rec.pane)
    } else {
        app.location_of(rec)
    };
    let mut spans = vec![
        Span::styled(marker.to_string(), Style::default().fg(t.accent)),
        Span::styled(fit(&location, c.loc), sel),
    ];
    if c.project > 0 {
        spans.push(Span::styled(fit(&project_of(rec), c.project), sel));
    }
    spans.extend(badge_spans(t, rec, c.harness));
    spans.extend([
        Span::styled(fit(&rec.model_cell(), c.model), Style::default().fg(t.dim)),
        Span::styled(fit(&state_txt, W_STATE), t.state_style(shown)),
        Span::styled(
            format!(
                "{:>w$} ",
                store::fmt_age(store::age_secs(&rec.since, now)),
                w = W_AGE
            ),
            Style::default().fg(t.dim),
        ),
        Span::styled(
            one_line(rec.last_message.as_deref().unwrap_or(""), c.msg),
            Style::default().fg(t.text),
        ),
    ]);
    Line::from(spans)
}

fn child_line(
    app: &App,
    i: usize,
    ci: usize,
    selected: bool,
    now: DateTime<Utc>,
    label_w: usize,
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
        Span::styled(fit(&label, label_w), lstyle),
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

/// The help overlay's contents: sections of aligned `key -> action` pairs.
pub const HELP_SECTIONS: [(&str, &[(&str, &str)]); 3] = [
    (
        "Navigate",
        &[
            ("j/k  ↓/↑", "move"),
            ("gg / G", "top / bottom"),
            ("Enter", "jump to pane"),
            ("n", "next waiting"),
        ],
    ),
    (
        "View",
        &[
            ("v", "grouped / flat"),
            ("Space", "expand subagents"),
            ("e", "show ended"),
            ("r", "refresh"),
            ("?", "close"),
        ],
    ),
    (
        "Act",
        &[
            ("x", "dismiss done"),
            ("m", "mute"),
            ("S", "setup"),
            ("q", "quit"),
        ],
    ),
];

/// One line per state: the state whose glyph and colour to use, the word
/// shown, and what it actually means. `delegating` is `working` in effect,
/// so it borrows that row's look.
pub const HELP_LEGEND: [(State, &str, &str); 7] = [
    (State::Working, "working", "a turn is in progress"),
    (
        State::Working,
        "delegating",
        "its turn ended, but subagents it started still run",
    ),
    (
        State::NeedsInput,
        "needs_input",
        "a permission prompt is blocking the agent",
    ),
    (
        State::NeedsInput,
        "question",
        "the agent asked you something: perch's popup, or its own dialog",
    ),
    (State::Done, "done", "finished while you were elsewhere"),
    (State::Idle, "idle", "finished and seen"),
    (State::Ended, "ended", "the pane or session is gone"),
];

/// The one line that explains the harness-column badge.
pub const HELP_BADGE: &str = "⚑n ▶n ✓n after the harness: subagents blocked / running / finished";

/// Centre a `w` x `h` box inside `area`, shrinking to fit.
fn centered(area: ratatui::layout::Rect, w: u16, h: u16) -> ratatui::layout::Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    ratatui::layout::Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Width of one help column, and of the key half inside it.
const HELP_COL: usize = 30;
const HELP_KEY: usize = 10;

fn help_overlay(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let t = &app.theme;
    let title = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
    let key = Style::default().fg(t.text);
    let dim = Style::default().fg(t.dim);

    // The three sections side by side, as aligned key -> action pairs.
    let mut lines: Vec<Line> = vec![Line::from(
        HELP_SECTIONS
            .iter()
            .map(|(name, _)| Span::styled(format!("{name:<HELP_COL$}"), title))
            .collect::<Vec<_>>(),
    )];
    let rows = HELP_SECTIONS
        .iter()
        .map(|(_, k)| k.len())
        .max()
        .unwrap_or(0);
    for r in 0..rows {
        let mut spans = Vec::new();
        for (_, keys) in HELP_SECTIONS {
            match keys.get(r) {
                Some((k, action)) => {
                    spans.push(Span::styled(format!(" {k:<w$}", w = HELP_KEY), key));
                    spans.push(Span::styled(fit(action, HELP_COL - HELP_KEY - 1), dim));
                }
                None => spans.push(Span::raw(" ".repeat(HELP_COL))),
            }
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("States", title)));
    for (state, word, meaning) in HELP_LEGEND {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} {:<12}", state_glyph(state, 0), word),
                t.state_style(state),
            ),
            Span::styled(meaning.to_string(), dim),
        ]));
    }
    lines.push(Line::from(Span::styled(format!(" {HELP_BADGE}"), dim)));

    let rect = centered(area, (HELP_COL * 3 + 2) as u16, lines.len() as u16 + 2);
    f.render_widget(ratatui::widgets::Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.accent))
                .title(Span::styled("help — any key closes", title)),
        ),
        rect,
    );
}

/// Word-wrap `text` to `width` columns; a word longer than the width is cut.
/// Never returns an empty list: an empty text is one empty line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word: String = word
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        while word.chars().count() > width {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            out.push(head);
        }
        let need = word.chars().count() + usize::from(!line.is_empty());
        if line.chars().count() + need > width && !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// Width of the option labels column for a question.
fn label_width(q: &Question) -> usize {
    q.options
        .iter()
        .map(|o| o.label.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(5, 24)
}

/// Indent of an option's description: marker, glyph, label, gap.
fn desc_indent(q: &Question) -> usize {
    2 + 2 + label_width(q) + 2
}

/// Rows one question needs inside a form `width` columns wide (border
/// included in `width`, excluded from the count): the tab row, the wrapped
/// question, a blank, one row per option plus wrapped description overflow,
/// the Other row, a blank, and the key hints. The popup is sized from this,
/// so nothing is cut.
pub fn form_rows(q: &Question, width: usize) -> usize {
    let inner = width.saturating_sub(4).max(10);
    let mut rows = 1 + wrap(&q.question, inner).len() + 1;
    let desc_w = inner.saturating_sub(desc_indent(q)).max(10);
    for o in &q.options {
        rows += wrap(&o.description, desc_w).len().max(1);
    }
    rows + 1 + 1 + 1
}

/// The form, filling the frame: the popup body's whole screen.
pub fn render_form(f: &mut Frame, app: &App) {
    form_box(f, app, f.area());
}

/// [`render_form`] offscreen, as text, for tests.
pub fn render_form_to_string(app: &App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(ratatui::backend::TestBackend::new(w, h))
        .expect("TestBackend never fails to build");
    term.draw(|f| render_form(f, app)).expect("offscreen draw");
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// The form box, filling `area`.
fn form_box(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let Some(form) = app.form.as_ref() else {
        return;
    };
    let t = &app.theme;
    let q = form.current();
    let title_style = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
    let text = Style::default().fg(t.text);
    let dim = Style::default().fg(t.dim);
    let pick = Style::default()
        .fg(t.needs_input)
        .add_modifier(Modifier::BOLD);

    let width = area.width as usize;
    let inner = width.saturating_sub(4);
    // The tab row: every question's header, the current one lit, answered
    // ones ticked. This is how you see there is more, and go back.
    let mut tabs: Vec<Span> = Vec::new();
    for (i, question) in form.questions.iter().enumerate() {
        let name = if question.header.is_empty() {
            format!("{}", i + 1)
        } else {
            question.header.clone()
        };
        let label = if form.answered(i) {
            format!(" {name} ✓ ")
        } else {
            format!(" {name} ")
        };
        let style = if i == form.index {
            title_style.add_modifier(Modifier::REVERSED)
        } else if form.answered(i) {
            text
        } else {
            dim
        };
        tabs.push(Span::styled(label, style));
        tabs.push(Span::raw(" "));
    }
    // Position at the right end of the tab row.
    let pos = format!("{}/{}", form.index + 1, form.questions.len());
    let used: usize = tabs.iter().map(|s| s.content.chars().count()).sum();
    tabs.push(Span::styled(
        format!("{:>w$}", pos, w = inner.saturating_sub(used).max(pos.len())),
        dim,
    ));
    let mut lines: Vec<Line> = vec![Line::from(tabs)];
    for l in wrap(&q.question, inner) {
        lines.push(Line::from(Span::styled(l, text)));
    }
    lines.push(Line::from(""));
    let label_w = label_width(q);
    let desc_w = inner.saturating_sub(desc_indent(q)).max(10);
    for (i, o) in q.options.iter().enumerate() {
        let on = form.picked[form.index].contains(&i);
        let glyph = match (q.multi_select, on) {
            (true, true) => "◼",
            (true, false) => "◻",
            (false, true) => "●",
            (false, false) => "○",
        };
        let here = i == form.cursor && !form.typing;
        let marker = if here { "▌ " } else { "  " };
        let label_style = if here {
            text.add_modifier(Modifier::REVERSED)
        } else {
            text
        };
        // The description wraps under itself, so a long one is read in full
        // rather than cut; the popup is sized for it (`form_rows`).
        let mut desc = wrap(&o.description, desc_w).into_iter();
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), Style::default().fg(t.accent)),
            Span::styled(format!("{glyph} "), if on { pick } else { dim }),
            Span::styled(fit(&o.label, label_w), label_style),
            Span::styled(format!("  {}", desc.next().unwrap_or_default()), dim),
        ]));
        for more in desc {
            lines.push(Line::from(Span::styled(
                format!("{}{more}", " ".repeat(desc_indent(q))),
                dim,
            )));
        }
    }
    // The Other row, or the text line it opens.
    let on_other = form.cursor >= q.options.len();
    let other_label = if q.options.is_empty() {
        "Type an answer"
    } else {
        "Other…"
    };
    if form.typing {
        lines.push(Line::from(vec![
            Span::styled("▌ ".to_string(), Style::default().fg(t.accent)),
            Span::styled("▶ ", pick),
            Span::styled(
                fit(
                    &format!("{}_", one_line(&form.text, inner.saturating_sub(6))),
                    inner.saturating_sub(4),
                ),
                text,
            ),
        ]));
    } else {
        let marker = if on_other { "▌ " } else { "  " };
        let style = if on_other {
            text.add_modifier(Modifier::REVERSED)
        } else {
            dim
        };
        let typed = form.other[form.index]
            .as_deref()
            .map(|s| format!("  {}", one_line(s, inner.saturating_sub(14))))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), Style::default().fg(t.accent)),
            Span::styled("○ ".to_string(), dim),
            Span::styled(fit(other_label, label_w), style),
            Span::styled(typed, dim),
        ]));
    }
    lines.push(Line::from(""));
    let keys = if form.typing {
        "Enter done   Esc back"
    } else if form.index + 1 == form.questions.len() {
        if q.multi_select {
            "Enter send   Space toggle   ←/→ tabs   p answer in its pane   Esc leave it to the agent"
        } else {
            "Enter send   ←/→ tabs   p answer in its pane   Esc leave it to the agent"
        }
    } else if q.multi_select {
        "Enter next   Space toggle   ←/→ tabs   p answer in its pane   Esc leave it to the agent"
    } else {
        "Enter select   ←/→ tabs   p answer in its pane   Esc leave it to the agent"
    };
    lines.push(Line::from(Span::styled(keys.to_string(), dim)));

    // Whose question: project (branch) · harness · where the pane is, so two
    // sessions asking at once are told apart at a glance; then the queue.
    let title = match app.records.iter().find(|r| r.pane == form.pane) {
        Some(rec) => {
            let mut parts = vec![match (&rec.project, &rec.branch) {
                (Some(p), Some(b)) if !b.is_empty() => format!("{p} ({b})"),
                (Some(p), _) => p.clone(),
                _ => project_of(rec),
            }];
            let model = rec.model_cell();
            parts.push(if model.is_empty() {
                rec.harness.as_str().to_string()
            } else {
                format!("{} {model}", rec.harness.as_str())
            });
            if let Some(loc) = rec.location.as_ref().filter(|l| !l.is_empty()) {
                parts.push(loc.clone());
            }
            let mut t = format!("⚑ {}", parts.join(" · "));
            if app.waiting > 0 {
                t.push_str(&format!(" · +{} waiting", app.waiting));
            }
            t
        }
        None => "⚑ question".to_string(),
    };
    let rect = area;
    f.render_widget(ratatui::widgets::Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.needs_input))
                .title(Span::styled(title, title_style)),
        ),
        rect,
    );
}

/// Render `app` offscreen into plain text: one line per terminal row, with
/// trailing spaces stripped.
///
/// This is what the golden snapshots compare, so the dashboard's layout is
/// asserted as a whole picture rather than as a bag of `contains`. `now` is
/// passed in, so the age column is a fixed function of the fixture.
pub fn render_to_string(app: &App, w: u16, h: u16, now: DateTime<Utc>) -> String {
    let mut term = Terminal::new(ratatui::backend::TestBackend::new(w, h))
        .expect("TestBackend never fails to build");
    term.draw(|f| render(f, app, now)).expect("offscreen draw");
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

pub fn render(f: &mut Frame, app: &App, now: DateTime<Utc>) {
    let banner = u16::from(!app.unwired.is_empty());
    let err = u16::from(app.error.is_some());
    let areas = Layout::vertical([
        Constraint::Length(banner),
        Constraint::Min(3),
        Constraint::Length(err),
        Constraint::Length(1),
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
    let loc_w = app.location_width();
    // Grouped rows sit under a `▸ project (branch)` header, so repeating the
    // project on every row says nothing; flat rows still need it.
    let project_w = if app.grouped { 0 } else { W_PROJECT };
    let harness_w = app.harness_width();
    let model_w = app.model_width();
    let fixed = 2 + loc_w + project_w + harness_w + model_w + W_STATE + W_AGE + 1;
    let cols = Cols {
        loc: loc_w,
        project: project_w,
        harness: harness_w,
        model: model_w,
        msg: inner_w.saturating_sub(fixed).max(1),
    };

    let rows = app.rows();
    let cursor_row = app.cursor_row(&rows);
    let mut lines: Vec<Line> = vec![header_line(t, cols)];
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no agents yet — start claude/codex/pi in a tmux pane",
            Style::default().fg(t.dim),
        )));
    }
    for (n, row) in rows.iter().enumerate() {
        let selected = n == cursor_row;
        lines.push(match row {
            RowKind::Header { project, members } => Line::from(vec![
                Span::styled(
                    format!("▸ {}  ", group_label(app, project, members)),
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(counts_label(app, members), Style::default().fg(t.dim)),
            ]),
            RowKind::Pane(i) => pane_line(app, *i, selected, now, cols),
            RowKind::Child(i, ci) => child_line(
                app,
                *i,
                *ci,
                selected,
                now,
                cols.loc + cols.project + cols.harness + cols.model,
                cols.msg,
            ),
            RowKind::EndedNote(k) => Line::from(Span::styled(
                format!("  {k} ended, press e to show"),
                Style::default().fg(t.ended).add_modifier(Modifier::DIM),
            )),
        });
    }

    // Scroll so the cursor line stays on screen.
    let view = areas[1].height.saturating_sub(2) as usize;
    let cursor = cursor_row + 1;
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

    if err == 1 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                app.error.clone().unwrap_or_default(),
                Style::default()
                    .fg(t.needs_input)
                    .add_modifier(Modifier::BOLD),
            ))),
            areas[2],
        );
    }

    f.render_widget(Paragraph::new(footer_line(app, areas[3].width)), areas[3]);

    if app.show_help {
        help_overlay(f, app, areas[1]);
    }
}

/// The footer: the four keys that matter, and the board's status, right-aligned.
///
/// One line, always. Everything else lives behind `?`.
fn footer_line(app: &App, width: u16) -> Line<'static> {
    let t = &app.theme;
    let keys = "Enter jump   n next   ? help   q quit";
    let status = format!(
        "[{}] [{}] {} panes",
        if app.muted { "muted" } else { "sound on" },
        if app.grouped { "grouped" } else { "flat" },
        app.records.len()
    );
    let pad = (width as usize)
        .saturating_sub(keys.chars().count() + status.chars().count())
        .max(1);
    Line::from(vec![
        Span::styled(keys.to_string(), Style::default().fg(t.dim)),
        Span::raw(" ".repeat(pad)),
        Span::styled(status, Style::default().fg(t.accent)),
    ])
}

// ---------------------------------------------------------------- keys

/// How long after a `g` a second `g` still means "top".
pub const GG_WINDOW: Duration = Duration::from_millis(500);

/// The keys that only move the cursor or change the view, and the one bit of
/// state they need: a pending `g`.
///
/// Split out of the event loop so the whole `gg` / `G` / `v` contract is
/// testable without a terminal. `g` on its own does nothing: it is only ever
/// the first half of `gg`, and the grouped/flat toggle lives on `v`.
#[derive(Debug, Default)]
pub struct Nav {
    pending_g: Option<Instant>,
    /// Set when a handled key changed a preference worth persisting; the
    /// caller clears it. Kept out of here so tests never touch the disk.
    pub prefs_dirty: bool,
}

impl Nav {
    pub fn new() -> Self {
        Nav::default()
    }

    /// Handle one key, returning whether it was consumed.
    ///
    /// `now` is injected so the `gg` window is testable.
    pub fn key(&mut self, app: &mut App, code: KeyCode, now: Instant) -> bool {
        let pending = self
            .pending_g
            .take()
            .is_some_and(|t| now.duration_since(t) < GG_WINDOW);
        match code {
            KeyCode::Char('j') | KeyCode::Down => app.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => app.move_by(-1),
            KeyCode::Char('G') => app.move_to_edge(true),
            KeyCode::Char('g') => {
                if pending {
                    app.move_to_edge(false);
                } else {
                    self.pending_g = Some(now);
                }
            }
            KeyCode::Char('v') => {
                app.grouped = !app.grouped;
                app.normalize();
                self.prefs_dirty = true;
            }
            KeyCode::Char('e') => {
                app.show_ended = !app.show_ended;
                app.normalize();
                self.prefs_dirty = true;
            }
            KeyCode::Char('n') => {
                if let Some(sel) = app.next_waiting() {
                    app.selected = Some(sel);
                    app.normalize();
                }
            }
            // A pane's finished subagents are folded into its badge; this is
            // how you look at them.
            KeyCode::Char(' ') | KeyCode::Tab => app.toggle_expanded(),
            KeyCode::Char('?') => app.show_help = true,
            // Anything else cancels a pending `g` (already taken above).
            _ => return false,
        }
        true
    }
}

/// Run the dashboard until the user quits or jumps.
pub fn run(tmux: &dyn Tmux, client: &str) -> Result<()> {
    let mut app = App::new(Vec::new());
    app.refresh_live(store::snapshot_with_live(tmux));

    enable_raw_mode()?;
    let mut out = io::stdout();
    crossterm::execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(ratatui::backend::CrosstermBackend::new(out))?;

    let result = event_loop(&mut term, &mut app, tmux, client);

    disable_raw_mode()?;
    crossterm::execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    result
}

/// Move one named client to a pane and mark that pane seen.
///
/// The client is always explicit — see `Tmux::jump`. The jump is synchronous
/// and checked; the pane is confirmed live first, so a stale record cannot
/// send tmux (and the user) somewhere that no longer exists.
///
/// `Err(msg)` is a message fit to show in the dashboard.
pub fn jump_to(tmux: &dyn Tmux, client: &str, pane: &str) -> std::result::Result<(), String> {
    if !tmux.pane_exists(pane) {
        return Err(format!("pane {pane} is gone"));
    }
    if !tmux.jump(client, pane) {
        return Err(format!("tmux refused the jump to {pane}"));
    }
    crate::hook::seen(pane);
    // A question perch is holding stays perch's: the popup comes along, over
    // the pane you just arrived at, so you answer it there or hand it to
    // Claude with `p` on purpose. This is deliberate even for a question put
    // away with Esc — jumping to the pane is asking for it — and it is the
    // one thing that keeps `a` working after you have looked and left.
    if store::load(pane)
        .and_then(|r| r.live_question(Utc::now()).cloned())
        .is_some()
    {
        crate::ask::show_on_client(tmux, pane, client);
    }
    Ok(())
}

fn event_loop<B: Backend>(
    term: &mut Terminal<B>,
    app: &mut App,
    tmux: &dyn Tmux,
    client: &str,
) -> Result<()> {
    let mut last_refresh = Instant::now();
    let mut nav = Nav::new();
    loop {
        term.draw(|f| render(f, app, Utc::now()))?;

        if event::poll(Duration::from_millis(250))? {
            if let CEvent::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                // The overlay is modal and any key closes it.
                if app.show_help {
                    app.show_help = false;
                    continue;
                }
                app.error = None;
                if nav.key(app, k.code, Instant::now()) {
                    if std::mem::take(&mut nav.prefs_dirty) {
                        save_prefs(app);
                    }
                    continue;
                }
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('m') => app.muted = sound::toggle_mute(),
                    KeyCode::Char('x') => {
                        if let Some(rec) = app.current() {
                            if rec.state == State::Done {
                                let mut r = rec.clone();
                                r.state = State::Idle;
                                r.since = store::now_rfc3339();
                                let _ = store::save(&r);
                                app.refresh_live(store::snapshot_with_live(tmux));
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
                        app.refresh_live(store::snapshot_with_live(tmux));
                    }
                    KeyCode::Char('r') => app.refresh(store::snapshot(tmux)),
                    KeyCode::Enter => {
                        if let Some(pane) = app.jump_target() {
                            // A failed jump stays in the dashboard with the
                            // reason on screen; only a real move closes it.
                            match jump_to(tmux, client, &pane) {
                                Ok(()) => return Ok(()),
                                Err(msg) => {
                                    app.error = Some(msg);
                                    app.refresh_live(store::snapshot_with_live(tmux));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if last_refresh.elapsed() >= Duration::from_secs(1) {
            app.refresh_live(store::snapshot_with_live(tmux));
            app.muted = sound::is_muted();
            last_refresh = Instant::now();
        }
    }
}
