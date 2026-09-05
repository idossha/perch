//! Offscreen TUI test: no terminal is opened, only ratatui's TestBackend.

use chrono::{Duration, Utc};
use perch::model::{Harness, PaneRecord, State, Subagent};
use perch::tui::{render, App, RowKind, LIGHT};
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

fn kid(id: &str, agent_type: Option<&str>, state: State, msg: &str) -> Subagent {
    Subagent {
        id: id.into(),
        agent_type: agent_type.map(String::from),
        state,
        since: Utc::now().to_rfc3339(),
        last_message: Some(msg.into()),
    }
}

/// Trimmed text of each rendered line.
fn lines(app: &App) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(110, 20)).unwrap();
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

fn board() -> App {
    let records = vec![
        rec("%2", "luna", State::NeedsInput, 30, "approve git push?"),
        rec("%1", "perch", State::Done, 120, "wrote the reducer"),
        rec("%3", "perch", State::Working, 5, "editing\nsecond line"),
        rec("%4", "duet", State::Idle, 900, ""),
        rec("%5", "quill", State::Ended, 60, "bye"),
    ];
    App::with_records(records)
}

#[test]
fn every_state_renders_its_glyph_and_name() {
    let mut app = board();
    app.show_ended = true;
    let joined = lines(&app).join("\n");
    for (glyph, name) in [
        ("⚑", "needs_input"),
        ("✓", "done"),
        ("▶", "working"),
        ("·", "idle"),
        ("✕", "ended"),
    ] {
        assert!(
            joined.contains(&format!("{glyph} {name}")),
            "missing {glyph} {name}:\n{joined}"
        );
    }
}

#[test]
fn ended_rows_are_hidden_until_e() {
    let app = board();
    let out = lines(&app).join("\n");
    assert!(!out.contains("✕ ended"), "{out}");
    assert!(out.contains("1 ended, press e to show"), "{out}");

    let mut app = board();
    app.show_ended = true;
    let out = lines(&app).join("\n");
    assert!(out.contains("✕ ended"), "{out}");
    assert!(!out.contains("press e to show"), "{out}");
}

#[test]
fn group_headers_are_ordered_by_urgency_with_counts() {
    let app = board();
    let out = lines(&app);
    let heads: Vec<&String> = out.iter().filter(|l| l.contains('▸')).collect();
    let names: Vec<String> = heads
        .iter()
        .map(|l| l.trim_matches(|c| c == '│' || c == ' ').to_string())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names[0].starts_with("▸ luna"), "{names:?}"); // needs_input
    assert!(names[1].starts_with("▸ perch"), "{names:?}"); // done + working
    assert!(names[2].starts_with("▸ duet"), "{names:?}"); // idle
    assert!(
        names[1].contains("✓1") && names[1].contains("▶1"),
        "{names:?}"
    );
}

#[test]
fn grouping_toggles_to_flat_newest_first() {
    let mut app = board();
    app.grouped = false;
    let out = lines(&app);
    assert!(!out.iter().any(|l| l.contains('▸')));
    let panes: Vec<&String> = out
        .iter()
        .filter(|l| l.contains("%1") || l.contains("%2") || l.contains("%3") || l.contains("%4"))
        .collect();
    // Newest state change first: %3 (5s), %2 (30s), %1 (2m), %4 (15m).
    assert!(panes[0].contains("%3"), "{panes:?}");
    assert!(panes[1].contains("%2"), "{panes:?}");
    assert!(panes[3].contains("%4"), "{panes:?}");
}

#[test]
fn subagent_rows_render_and_enter_resolves_to_the_parent_pane() {
    let mut app = board();
    app.records[0].children.push(kid(
        "abcdef0123456789",
        Some("Explore"),
        State::Working,
        "reading src",
    ));
    app.records[0]
        .children
        .push(kid("ff00ff00ff00", None, State::Done, "found it"));
    app.records[0]
        .children
        .push(kid("deaddead", None, State::Ended, "gone"));
    let out = lines(&app).join("\n");
    assert!(out.contains("└ Explore"), "{out}");
    assert!(out.contains("└ ff00ff00"), "{out}"); // id[:8] when no agent_type
    assert!(!out.contains("└ deaddead"), "ended child shown:\n{out}");
    assert!(out.contains("claude +3"), "no child badge:\n{out}");

    // The cursor on a child resolves to the parent's pane.
    let rows = app.rows();
    let child_row = rows
        .iter()
        .position(|r| matches!(r, RowKind::Child(_, _)))
        .unwrap();
    app.selected = child_row;
    assert_eq!(app.current().unwrap().pane, "%2");
}

#[test]
fn navigation_skips_headers_and_notes() {
    let mut app = board();
    app.normalize();
    let rows = app.rows();
    assert!(rows[app.selected].selectable());
    assert_eq!(app.current().unwrap().pane, "%2");

    let mut seen = Vec::new();
    for _ in 0..4 {
        assert!(app.rows()[app.selected].selectable(), "landed on a header");
        seen.push(app.current().unwrap().pane.clone());
        app.move_by(1);
    }
    assert_eq!(seen, vec!["%2", "%1", "%3", "%4"]);
    // Wrapped back round to the first selectable row.
    assert_eq!(app.current().unwrap().pane, "%2");
    app.move_by(-1);
    assert_eq!(app.current().unwrap().pane, "%4");
}

#[test]
fn next_waiting_lands_on_the_first_flag() {
    let mut app = board();
    let i = app.next_waiting().unwrap();
    app.selected = i;
    assert_eq!(app.current().unwrap().pane, "%2");
}

#[test]
fn long_message_is_one_truncated_line() {
    let mut app = App::with_records(vec![rec(
        "%1",
        "perch",
        State::Working,
        1,
        &format!("{}\nsecond line", "x".repeat(300)),
    )]);
    app.normalize();
    let out = lines(&app);
    assert!(!out.iter().any(|l| l.contains("second line")), "{out:?}");
    assert!(out.iter().any(|l| l.contains('…')), "{out:?}");
}

#[test]
fn light_theme_renders_without_panicking() {
    let mut app = board();
    app.theme = LIGHT;
    app.show_ended = true;
    let out = lines(&app).join("\n");
    assert!(out.contains("⚑ needs_input"), "{out}");
}

#[test]
fn footer_shows_the_keys_and_mute_state() {
    let app = board();
    let out = lines(&app);
    let footer = out.last().unwrap();
    assert!(footer.contains("Enter jump"), "{footer}");
    assert!(
        footer.contains("e ended") && footer.contains("g group"),
        "{footer}"
    );
    assert!(footer.contains("sound on"), "{footer}");

    let mut app = board();
    app.muted = true;
    app.show_help = true;
    let out = lines(&app);
    assert!(out[out.len() - 2].contains("muted"), "{out:?}");
    assert!(out.last().unwrap().contains("keys:"), "{out:?}");
}

#[test]
fn empty_store_renders_without_panicking() {
    let mut app = App::with_records(vec![]);
    let out = lines(&app);
    assert!(out.join("\n").contains("perch"));
    app.move_by(1);
    assert_eq!(app.selected, 0);
    assert!(app.current().is_none());
}

#[test]
fn banner_shows_only_when_a_harness_is_unwired() {
    let app = board();
    assert!(!lines(&app).join("\n").contains("press S to run setup"));

    let mut app = board();
    app.unwired = vec!["claude".into(), "codex".into()];
    let out = lines(&app);
    assert!(
        out[0].contains("perch is not wired into claude, codex: press S to run setup"),
        "{out:?}"
    );
    assert!(out.join("\n").contains("needs_input"));
}

#[test]
fn project_falls_back_to_the_cwd_basename_then_a_stub() {
    let mut a = rec("%1", "", State::Idle, 1, "");
    a.project = None;
    a.cwd = Some("/Users/x/00_development/perch/".into());
    let mut b = rec("%2", "", State::Idle, 1, "");
    b.project = None;
    let app = App::with_records(vec![a, b]);
    let out = lines(&app).join("\n");
    assert!(out.contains("perch"), "{out}");
    assert!(out.contains("(no project)"), "{out}");
}
