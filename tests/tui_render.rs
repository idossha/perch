//! Offscreen TUI test: no terminal is opened, only ratatui's TestBackend.

use chrono::{Duration, Utc};
use perch::model::{Harness, PaneRecord, State};
use perch::tui::{render, App};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn rec(pane: &str, project: &str, state: State, age_secs: i64, msg: &str) -> PaneRecord {
    let mut r = PaneRecord::new(pane, Harness::Claude, "");
    r.since = (Utc::now() - Duration::seconds(age_secs)).to_rfc3339();
    r.state = state;
    r.project = Some(project.into());
    r.last_message = Some(msg.into());
    r
}

/// Trimmed text of each rendered row.
fn lines(app: &App) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(90, 12)).unwrap();
    let now = Utc::now();
    term.draw(|f| render(f, app, now)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn sorted_app() -> App {
    // The store hands the TUI records already sorted; mirror that here.
    let mut records = vec![
        rec("%2", "luna", State::NeedsInput, 30, "approve git push?"),
        rec("%1", "perch", State::Done, 120, "wrote the reducer"),
        rec("%3", "quill", State::Working, 5, "editing"),
        rec("%4", "duet", State::Idle, 900, ""),
    ];
    records.sort_by_key(|r| r.state.rank());
    App {
        records,
        selected: 0,
        muted: false,
        unwired: Vec::new(),
    }
}

#[test]
fn header_and_rows_render_in_state_order() {
    let out = lines(&sorted_app());
    let joined = out.join("\n");
    assert!(joined.contains("perch"), "title:\n{joined}");

    let header = out
        .iter()
        .position(|l| l.contains("pane") && l.contains("harness"));
    assert!(header.is_some(), "no header row:\n{joined}");
    let h = header.unwrap();

    // Rows follow the header, one per record, in needs_input/done/working/idle order.
    let expect = [
        ("%2", "luna", "needs_input", "30s", "approve git push?"),
        ("%1", "perch", "done", "2m", "wrote the reducer"),
        ("%3", "quill", "working", "5s", "editing"),
        ("%4", "duet", "idle", "15m", ""),
    ];
    for (i, (pane, project, state, age, msg)) in expect.iter().enumerate() {
        let row = &out[h + 1 + i];
        assert!(row.contains(pane), "row {i} missing {pane}: {row}");
        assert!(row.contains(project), "row {i} missing {project}: {row}");
        assert!(row.contains(state), "row {i} missing {state}: {row}");
        assert!(row.contains(age), "row {i} missing age {age}: {row}");
        assert!(row.contains(msg), "row {i} missing message: {row}");
    }
}

#[test]
fn footer_shows_the_keys_and_mute_state() {
    let out = lines(&sorted_app());
    let footer = out.last().unwrap();
    assert!(footer.contains("Enter jump"), "{footer}");
    assert!(footer.contains("sound on"), "{footer}");

    let mut app = sorted_app();
    app.muted = true;
    assert!(lines(&app).last().unwrap().contains("muted"));
}

#[test]
fn selection_wraps_and_next_waiting_picks_the_first_flag() {
    let mut app = sorted_app();
    assert_eq!(app.next_waiting(), Some(0));
    app.move_by(-1);
    assert_eq!(app.selected, 3);
    app.move_by(1);
    assert_eq!(app.selected, 0);
    assert_eq!(app.current().unwrap().pane, "%2");
}

#[test]
fn empty_store_renders_without_panicking() {
    let app = App {
        records: vec![],
        selected: 0,
        muted: false,
        unwired: Vec::new(),
    };
    let out = lines(&app);
    assert!(out.join("\n").contains("perch"));
    let mut app = app;
    app.move_by(1);
    assert_eq!(app.selected, 0);
    assert!(app.current().is_none());
}

#[test]
fn banner_shows_only_when_a_harness_is_unwired() {
    let app = sorted_app();
    assert!(!lines(&app).join("\n").contains("press S to run setup"));

    let mut app = sorted_app();
    app.unwired = vec!["claude".into(), "codex".into()];
    let out = lines(&app);
    assert!(
        out[0].contains("perch is not wired into claude, codex: press S to run setup"),
        "{out:?}"
    );
    // The table still renders below the banner.
    assert!(out.join("\n").contains("needs_input"));
}
