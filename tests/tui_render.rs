//! Offscreen TUI test: no terminal is opened, only ratatui's TestBackend.

use chrono::{Duration, Utc};
use crossterm::event::KeyCode;
use perch::model::{Harness, PaneRecord, State, Subagent};
use perch::tmux::LivePane;
use perch::tui::{render, App, Nav, RowKind, Selection, LIGHT};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::Instant;

fn rec(pane: &str, project: &str, state: State, age_secs: i64, msg: &str) -> PaneRecord {
    let mut r = PaneRecord::new(pane, Harness::Claude, "");
    r.since = (Utc::now() - Duration::seconds(age_secs)).to_rfc3339();
    r.state = state;
    r.project = Some(project.into());
    r.last_message = Some(msg.into());
    r
}

/// One live tmux pane, as `list-panes` would report it.
fn live(pane: &str, session: &str, window_name: &str, index: u32, panes: u32) -> LivePane {
    LivePane {
        pane: pane.into(),
        session: session.into(),
        window: "0".into(),
        window_name: window_name.into(),
        pane_index: index,
        window_panes: panes,
        session_attached: true,
        command: "claude".into(),
        pid: None,
    }
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
    let mut app = App::with_records(records);
    app.live = (1..=5)
        .map(|n| live(&format!("%{n}"), "main", &format!("w{n}"), 0, 1))
        .collect();
    app
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
    let rows: Vec<&String> = out
        .iter()
        .filter(|l| ["w1", "w2", "w3", "w4"].iter().any(|w| l.contains(w)))
        .collect();
    // Newest state change first: %3 (5s), %2 (30s), %1 (2m), %4 (15m).
    assert!(rows[0].contains("w3"), "{rows:?}");
    assert!(rows[1].contains("w2"), "{rows:?}");
    assert!(rows[3].contains("w4"), "{rows:?}");
    // Flat rows carry the project, since there is no group header to say it.
    assert!(rows[0].contains("perch"), "{rows:?}");
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
    app.selected = app.selection_at(&rows[child_row]);
    assert_eq!(app.current().unwrap().pane, "%2");
    assert_eq!(app.jump_target().as_deref(), Some("%2"));
}

#[test]
fn navigation_skips_headers_and_notes() {
    let mut app = board();
    app.normalize();
    let rows = app.rows();
    assert!(rows[app.cursor_row(&rows)].selectable());
    assert_eq!(app.current().unwrap().pane, "%2");

    let mut seen = Vec::new();
    for _ in 0..4 {
        let rows = app.rows();
        assert!(
            rows[app.cursor_row(&rows)].selectable(),
            "landed on a header"
        );
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
fn next_waiting_is_the_oldest_needs_input_then_the_oldest_done() {
    let mut app = board();
    app.selected = app.next_waiting();
    assert_eq!(app.current().unwrap().pane, "%2");

    // With nothing flagged, the oldest `done` is next.
    let mut app = board();
    app.records[0].state = State::Working;
    assert_eq!(app.next_waiting().unwrap().pane, "%1");

    let mut app = board();
    for r in app.records.iter_mut() {
        r.state = State::Idle;
    }
    assert!(app.next_waiting().is_none());
}

/// Selection is keyed by pane id, so a refresh that reorders the board — a
/// group jumping to the top because one of its panes needs input — leaves the
/// cursor on the same agent. This is the bug the navigation contract exists
/// for: `Enter` must never move a client to a pane the user was not on.
#[test]
fn the_selection_survives_a_reordering_refresh() {
    let mut app = board();
    app.grouped = false;
    app.normalize();
    app.move_by(1);
    let want = app.jump_target().unwrap();
    assert_eq!(want, "%2");

    // Reverse the record order and make a different pane the most urgent.
    let mut records = app.records.clone();
    records.reverse();
    records[0].state = State::NeedsInput;
    records[0].since = Utc::now().to_rfc3339();
    app.refresh(records);

    assert_eq!(app.jump_target().as_deref(), Some("%2"));
    assert_eq!(
        app.selected,
        Some(Selection {
            pane: "%2".into(),
            child: None
        })
    );
}

/// When the selected pane is gone the cursor falls to a neighbouring row
/// rather than snapping to the top of the board.
#[test]
fn a_vanished_pane_moves_the_cursor_to_the_nearest_row() {
    let mut app = board();
    app.grouped = false;
    app.normalize();
    app.move_by(2);
    assert_eq!(app.jump_target().as_deref(), Some("%1"));

    let records: Vec<_> = app
        .records
        .iter()
        .filter(|r| r.pane != "%1")
        .cloned()
        .collect();
    app.refresh(records);
    let target = app.jump_target().unwrap();
    assert_ne!(target, "%1");
    assert!(app.records.iter().any(|r| r.pane == target));
}

#[test]
fn gg_and_shift_g_go_to_the_ends() {
    let mut app = board();
    app.grouped = false;
    app.move_to_edge(true);
    assert_eq!(app.jump_target().as_deref(), Some("%4"));
    app.move_to_edge(false);
    assert_eq!(app.jump_target().as_deref(), Some("%3"));
}

/// `g` is only ever the first half of `gg`; the view toggle lives on `v`.
#[test]
fn gg_goes_to_the_top_and_a_lone_g_does_nothing() {
    let mut app = board();
    app.grouped = false;
    app.normalize();
    let mut nav = Nav::new();
    let t0 = Instant::now();

    // One `g` moves nothing and changes no view.
    nav.key(&mut app, KeyCode::Char('g'), t0);
    assert_eq!(app.jump_target().as_deref(), Some("%3"));
    assert!(!app.grouped && !nav.prefs_dirty, "g touched the view");

    // ... and it does not linger: `g` then `j` is just `j`.
    nav.key(&mut app, KeyCode::Char('j'), t0);
    assert_eq!(app.jump_target().as_deref(), Some("%2"));
    nav.key(&mut app, KeyCode::Char('g'), t0);
    assert_eq!(app.jump_target().as_deref(), Some("%2"));

    // `G` to the bottom, `gg` back to the top.
    nav.key(&mut app, KeyCode::Char('G'), t0);
    assert_eq!(app.jump_target().as_deref(), Some("%4"));
    nav.key(&mut app, KeyCode::Char('g'), t0);
    nav.key(
        &mut app,
        KeyCode::Char('g'),
        t0 + std::time::Duration::from_millis(100),
    );
    assert_eq!(app.jump_target().as_deref(), Some("%3"));

    // A second `g` after the window is a fresh first `g`, not a jump.
    nav.key(&mut app, KeyCode::Char('G'), t0);
    nav.key(&mut app, KeyCode::Char('g'), t0);
    nav.key(
        &mut app,
        KeyCode::Char('g'),
        t0 + std::time::Duration::from_millis(900),
    );
    assert_eq!(app.jump_target().as_deref(), Some("%4"));
}

#[test]
fn v_toggles_grouped_and_flat() {
    let mut app = board();
    let mut nav = Nav::new();
    assert!(app.grouped);
    assert!(nav.key(&mut app, KeyCode::Char('v'), Instant::now()));
    assert!(!app.grouped);
    assert!(nav.prefs_dirty, "the view toggle must be persisted");
    nav.prefs_dirty = false;
    nav.key(&mut app, KeyCode::Char('v'), Instant::now());
    assert!(app.grouped);
    assert!(lines(&app).iter().any(|l| l.contains('▸')));
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
fn the_footer_is_one_line_of_essentials_and_a_status() {
    let app = board();
    let out = lines(&app);
    let footer = out.last().unwrap();
    for part in ["Enter jump", "n next", "? help", "q quit"] {
        assert!(footer.contains(part), "{footer}");
    }
    assert!(footer.contains("[sound on]"), "{footer}");
    assert!(footer.contains("[grouped]"), "{footer}");
    assert!(footer.contains("5 panes"), "{footer}");
    // Never two stacked lines: the line above the footer is the board border.
    assert!(!out[out.len() - 2].contains("Enter jump"), "{out:?}");

    let mut app = board();
    app.muted = true;
    app.grouped = false;
    let footer = lines(&app).last().unwrap().clone();
    assert!(
        footer.contains("[muted]") && footer.contains("[flat]"),
        "{footer}"
    );
}

#[test]
fn the_help_overlay_shows_every_key_and_the_state_legend() {
    let mut app = board();
    app.show_help = true;
    let out = lines(&app).join("\n");
    for section in ["Navigate", "View", "Act"] {
        assert!(out.contains(section), "{out}");
    }
    for key in [
        "j/k",
        "gg / G",
        "Enter",
        "next waiting",
        "grouped / flat",
        "v",
        "show ended",
        "refresh",
        "dismiss done",
        "mute",
        "setup",
    ] {
        assert!(out.contains(key), "missing {key}:\n{out}");
    }
    for (glyph, name) in [
        ("▶", "working"),
        ("⚑", "needs_input"),
        ("✓", "done"),
        ("·", "idle"),
        ("✕", "ended"),
    ] {
        assert!(
            out.contains(&format!("{glyph} {name}")),
            "legend {name}:\n{out}"
        );
    }
    assert!(out.contains("blocking the agent"), "{out}");
    assert!(out.contains("while you were elsewhere"), "{out}");
    // The footer is still one line under the overlay.
    let last = lines(&app).last().unwrap().clone();
    assert!(last.contains("q quit"), "{last}");
}

/// An error from a jump is its own line and the dashboard stays open.
#[test]
fn a_gone_pane_renders_an_error_line() {
    let mut app = board();
    app.error = Some("pane %9 is gone".into());
    let out = lines(&app);
    assert!(out.iter().any(|l| l.contains("pane %9 is gone")), "{out:?}");
    assert!(out.last().unwrap().contains("Enter jump"), "{out:?}");
}

#[test]
fn empty_store_renders_without_panicking() {
    let mut app = App::with_records(vec![]);
    let out = lines(&app);
    assert!(out
        .join("\n")
        .contains("no agents yet — start claude/codex/pi in a tmux pane"));
    app.move_by(1);
    assert_eq!(app.selected, None);
    assert!(app.current().is_none());
    assert!(app.jump_target().is_none());
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

/// What `Enter` does: one `switch-client -c <client> -t <pane_id>`, waited on
/// and checked. The popup closes the instant `jump_to` returns, so anything
/// merely spawned would be killed with the popup's pty before tmux ran it.
#[test]
fn enter_switches_the_named_client_to_the_pane_id() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("tmux.log");
    std::env::set_var("PERCH_TMUX_LOG", &log);
    std::env::set_var("PERCH_STATE_DIR", dir.path());

    let tmux = perch::tmux::NullTmux {
        panes: vec![live("%7", "other", "editor", 0, 1)],
    };
    assert_eq!(perch::tui::jump_to(&tmux, "/dev/ttys004", "%7"), Ok(()));
    let body = std::fs::read_to_string(&log).unwrap();
    std::env::remove_var("PERCH_TMUX_LOG");
    std::env::remove_var("PERCH_STATE_DIR");

    assert_eq!(
        body.lines().next().unwrap(),
        "switch-client -c /dev/ttys004 -t %7"
    );

    // A pane tmux no longer knows about is an error, not a wrong jump.
    assert_eq!(
        perch::tui::jump_to(&tmux, "/dev/ttys004", "%99"),
        Err("pane %99 is gone".to_string())
    );
}

// ------------------------------------------------------------- location

/// Rows say where the pane is on the tmux top rail. A `%444` is perch's key,
/// not something the user has ever navigated by, so it stays off the screen.
#[test]
fn rows_show_the_window_name_and_never_a_pane_id() {
    let app = board();
    let out = lines(&app).join("\n");
    assert!(out.contains("location"), "no location header:\n{out}");
    assert!(!out.contains("pane    "), "old pane header:\n{out}");
    for w in ["w1", "w2", "w3", "w4"] {
        assert!(out.contains(w), "missing window {w}:\n{out}");
    }
    assert!(!out.contains('%'), "a pane id reached the screen:\n{out}");
}

/// A location only carries what it needs to be unambiguous.
#[test]
fn the_session_prefix_appears_only_with_more_than_one_session() {
    let mut app = board();
    assert!(lines(&app).join("\n").contains(" w1"));
    assert!(!lines(&app).join("\n").contains("main/w1"));

    app.live[0] = live("%1", "other", "w1", 0, 1);
    assert!(
        lines(&app).join("\n").contains("other/w1"),
        "{:?}",
        lines(&app)
    );
}

#[test]
fn the_pane_index_appears_only_in_a_split_window() {
    let mut app = board();
    assert!(!lines(&app).join("\n").contains("w1."));
    app.live[0] = live("%1", "main", "w1", 2, 3);
    assert!(lines(&app).join("\n").contains("w1.2"), "{:?}", lines(&app));
}

/// A pane tmux no longer knows about falls back to the location the hook
/// stored, and to an em dash when there is none.
#[test]
fn an_ended_pane_shows_its_last_known_location() {
    let mut app = board();
    app.show_ended = true;
    app.live.clear();
    app.records[4].location = Some("gone-window".into());
    let out = lines(&app).join("\n");
    assert!(out.contains("gone-window"), "{out}");
    assert!(
        out.contains('—'),
        "no placeholder for an unknown location:\n{out}"
    );
}

/// Under a `▸ project` header the project on every row is noise.
#[test]
fn grouped_rows_drop_the_project_column() {
    let app = board();
    let out = lines(&app);
    let row = out
        .iter()
        .find(|l| l.contains("wrote the reducer"))
        .expect("the done row");
    assert!(row.contains("w1"), "{row}");
    assert!(
        !row.contains("perch"),
        "the project is repeated on the row:\n{row}"
    );
    // ... and the header carries the project with its branch.
    assert!(out.iter().any(|l| l.contains("▸ perch")), "{out:?}");
}

#[test]
fn a_group_header_carries_the_branch_when_the_group_agrees() {
    let mut app = board();
    for r in app
        .records
        .iter_mut()
        .filter(|r| r.project.as_deref() == Some("perch"))
    {
        r.branch = Some("main".into());
    }
    assert!(
        lines(&app).iter().any(|l| l.contains("▸ perch (main)")),
        "{:?}",
        lines(&app)
    );

    // Two branches in one project: the header says the project only.
    app.records[1].branch = Some("wip".into());
    let out = lines(&app);
    assert!(out.iter().any(|l| l.contains("▸ perch  ")), "{out:?}");
    assert!(!out.iter().any(|l| l.contains("(main)")), "{out:?}");
}

/// The pane id is still reachable, but only when the user asked for it.
#[test]
fn perch_debug_appends_the_pane_id() {
    let mut app = board();
    app.debug = true;
    let out = lines(&app).join("\n");
    assert!(out.contains("%1"), "{out}");
}

/// A long window name is capped rather than eating the message column.
#[test]
fn the_location_column_is_adaptive_and_capped() {
    let mut app = App::with_records(vec![rec("%1", "perch", State::Working, 1, "hello there")]);
    app.live = vec![live("%1", "main", &"n".repeat(60), 0, 1)];
    app.normalize();
    let out = lines(&app).join("\n");
    assert!(out.contains("hello there"), "message lost:\n{out}");
    assert!(out.contains('…'), "location not ellipsized:\n{out}");
}
