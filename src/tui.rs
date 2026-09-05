//! The ratatui dashboard, run inside `tmux display-popup`.

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
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::{Frame, Terminal};

use crate::model::{PaneRecord, State};
use crate::sound;
use crate::store;
use crate::tmux::Tmux;

pub struct App {
    pub records: Vec<PaneRecord>,
    pub selected: usize,
    pub muted: bool,
    /// Detected harnesses with no perch hook, shown as a top banner.
    pub unwired: Vec<String>,
}

impl App {
    pub fn new(records: Vec<PaneRecord>) -> Self {
        let paths = crate::paths::Paths::from_env();
        App {
            records,
            selected: 0,
            muted: sound::is_muted(),
            unwired: if crate::setup::needs_nudge(&paths) {
                crate::setup::unwired_harnesses(&paths)
                    .into_iter()
                    .map(String::from)
                    .collect()
            } else {
                Vec::new()
            },
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.records.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.records.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
    }

    pub fn current(&self) -> Option<&PaneRecord> {
        self.records.get(self.selected)
    }

    /// Index of the oldest pane waiting on the human (needs_input, then done).
    ///
    /// Records arrive already sorted by state rank then age, so it is the first.
    pub fn next_waiting(&self) -> Option<usize> {
        self.records
            .iter()
            .position(|r| matches!(r.state, State::NeedsInput | State::Done))
    }

    /// Replace the record list, keeping the cursor on the same pane if it lives.
    pub fn refresh(&mut self, records: Vec<PaneRecord>) {
        let keep = self.current().map(|r| r.pane.clone());
        self.records = records;
        self.selected = keep
            .and_then(|p| self.records.iter().position(|r| r.pane == p))
            .unwrap_or(0);
    }
}

pub fn state_color(state: State) -> Color {
    match state {
        State::NeedsInput => Color::Yellow,
        State::Done => Color::Green,
        State::Working => Color::Cyan,
        State::Starting => Color::Blue,
        State::Idle => Color::Gray,
        State::Ended => Color::DarkGray,
    }
}

/// The cells of one table row, in column order.
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

pub fn render(f: &mut Frame, app: &App, now: DateTime<Utc>) {
    let banner = u16::from(!app.unwired.is_empty());
    let areas = Layout::vertical([
        Constraint::Length(banner),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(f.area());
    if banner == 1 {
        let text = format!(
            "perch is not wired into {}: press S to run setup",
            app.unwired.join(", ")
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))),
            areas[0],
        );
    }

    let header = Row::new(["pane", "project", "harness", "state", "age", "last message"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = app
        .records
        .iter()
        .map(|rec| {
            let c = row_cells(rec, now);
            Row::new(vec![
                Cell::from(c[0].clone()),
                Cell::from(c[1].clone()),
                Cell::from(c[2].clone()),
                Cell::from(c[3].clone()).style(Style::default().fg(state_color(rec.state))),
                Cell::from(c[4].clone()),
                Cell::from(c[5].clone()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Length(22),
        Constraint::Length(8),
        Constraint::Length(11),
        Constraint::Length(5),
        Constraint::Min(10),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("perch"))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("");

    let mut st = TableState::default();
    if !app.records.is_empty() {
        st.select(Some(app.selected));
    }
    f.render_stateful_widget(table, areas[1], &mut st);

    let mute = if app.muted { "muted" } else { "sound on" };
    let help = Line::from(vec![
        Span::raw("j/k move  Enter jump  n next  m mute  x dismiss  r refresh  q quit  ["),
        Span::raw(mute),
        Span::raw("]"),
    ]);
    f.render_widget(Paragraph::new(help), areas[2]);
}

/// Run the dashboard until the user quits or jumps.
pub fn run(tmux: &dyn Tmux) -> Result<()> {
    let mut app = App::new(store::snapshot(tmux));

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
                            let pane = rec.pane.clone();
                            tmux.focus(&pane);
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
