//! Golden text snapshots of the dashboard.
//!
//! One fixture board, a fixed clock and ratatui's `TestBackend`: what these
//! assert is the whole picture — column widths, ordering, glyphs, the badge,
//! the footer — which no number of `contains` checks adds up to.
//!
//! **Policy.** Comparison is verbatim. `UPDATE_GOLDENS=1` rewrites the files
//! and then *fails* the test, so a blessing run can never be mistaken for a
//! passing one; the rewritten files must be read and committed deliberately.

use chrono::{DateTime, Duration, Utc};
use perch::model::{Harness, PaneRecord, State, Subagent};
use perch::tmux::LivePane;
use perch::tui::{render_to_string, App, LIGHT};

/// The one clock every golden is rendered against.
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn ago(secs: i64) -> String {
    (now() - Duration::seconds(secs)).to_rfc3339()
}

fn rec(
    pane: &str,
    project: &str,
    branch: Option<&str>,
    state: State,
    age: i64,
    msg: &str,
) -> PaneRecord {
    let mut r = PaneRecord::new(pane, Harness::Claude, "");
    r.since = ago(age);
    r.state = state;
    r.project = Some(project.into());
    r.branch = branch.map(String::from);
    r.last_message = (!msg.is_empty()).then(|| msg.to_string());
    r
}

fn kid(id: &str, agent_type: &str, state: State, age: i64, msg: &str) -> Subagent {
    Subagent {
        id: id.into(),
        agent_type: Some(agent_type.into()),
        state,
        since: ago(age),
        last_message: (!msg.is_empty()).then(|| msg.to_string()),
    }
}

fn live(pane: &str, window_name: &str) -> LivePane {
    LivePane {
        pane: pane.into(),
        session: "main".into(),
        window: "0".into(),
        window_name: window_name.into(),
        pane_index: 0,
        window_panes: 1,
        session_attached: true,
        command: "claude".into(),
        pid: None,
    }
}

/// The fixture board: one row of every state, a blocked subagent, a
/// delegating pane and its finished fan-out, and an ended pane.
fn board() -> App {
    let mut blocked = rec(
        "%1",
        "luna",
        Some("main"),
        State::NeedsInput,
        45,
        "permission_prompt",
    );
    blocked.children.push(kid(
        "ag-1",
        "Explore",
        State::NeedsInput,
        20,
        "write to src/main.rs?",
    ));

    let mut delegating = rec(
        "%5",
        "duet",
        None,
        State::Done,
        130,
        "handed off to 2 agents",
    );
    delegating
        .children
        .push(kid("ag-2", "Explore", State::Working, 110, "reading tests"));
    delegating
        .children
        .push(kid("ag-3", "Plan", State::Done, 200, "drafted the plan"));
    delegating
        .children
        .push(kid("ag-4", "Explore", State::Done, 300, "found the caller"));

    let records = vec![
        blocked,
        rec(
            "%2",
            "luna",
            Some("main"),
            State::Done,
            300,
            "wrote the reducer and its tests",
        ),
        rec(
            "%3",
            "perch",
            None,
            State::Working,
            12,
            "editing src/tui.rs",
        ),
        rec("%4", "perch", None, State::Idle, 1800, ""),
        delegating,
        rec("%6", "quill", None, State::Ended, 240, "bye"),
    ];
    let mut app = App::with_records(records);
    app.live = vec![
        live("%1", "luna"),
        live("%2", "review"),
        live("%3", "perch"),
        live("%4", "docs"),
        live("%5", "duet"),
    ];
    app
}

/// Compare `actual` with `tests/golden/<name>.txt`, verbatim.
///
/// With `UPDATE_GOLDENS=1` the file is rewritten and the test fails anyway:
/// re-blessing is an edit to review, never a green run.
fn golden(name: &str, actual: &str) {
    let path = format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::create_dir_all(format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::write(&path, actual).unwrap();
        panic!(
            "golden {name} was rewritten from this run; \
             review the diff and re-run without UPDATE_GOLDENS=1"
        );
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e} (run with UPDATE_GOLDENS=1 to create it)"));
    assert_eq!(
        actual, expected,
        "\n--- {name} changed ---\nactual:\n{actual}\nexpected:\n{expected}"
    );
}

#[test]
fn grouped_board_120x24() {
    golden(
        "grouped_120x24",
        &render_to_string(&board(), 120, 24, now()),
    );
}

#[test]
fn grouped_board_80x24() {
    golden("grouped_80x24", &render_to_string(&board(), 80, 24, now()));
}

#[test]
fn flat_board_120x24() {
    let mut app = board();
    app.grouped = false;
    golden("flat_120x24", &render_to_string(&app, 120, 24, now()));
}

#[test]
fn finished_children_unfold_with_space() {
    let mut app = board();
    app.expanded.insert("%5".into());
    golden("expanded_120x24", &render_to_string(&app, 120, 24, now()));
}

#[test]
fn ended_rows_appear_with_e() {
    let mut app = board();
    app.show_ended = true;
    golden("ended_120x24", &render_to_string(&app, 120, 24, now()));
}

#[test]
fn help_overlay_120x24() {
    let mut app = board();
    app.show_help = true;
    golden("help_120x24", &render_to_string(&app, 120, 24, now()));
}

#[test]
fn empty_state_80x24() {
    let app = App::with_records(Vec::new());
    golden("empty_80x24", &render_to_string(&app, 80, 24, now()));
}

#[test]
fn light_theme_120x24() {
    let mut app = board();
    app.theme = LIGHT;
    golden("light_120x24", &render_to_string(&app, 120, 24, now()));
}

#[test]
fn a_gone_pane_error_line_120x24() {
    let mut app = board();
    app.error = Some("pane %9 is gone".into());
    golden("gone_pane_120x24", &render_to_string(&app, 120, 24, now()));
}

#[test]
fn perch_debug_shows_pane_ids_120x24() {
    let mut app = board();
    app.debug = true;
    golden("debug_ids_120x24", &render_to_string(&app, 120, 24, now()));
}

/// The `model` column, present only once a pane has reported a model: label
/// and effort for Claude, the slug for Codex, nothing for the rest.
#[test]
fn model_column_120x24() {
    let mut app = board();
    app.records[0].model = Some("claude-opus-5".into());
    app.records[0].effort = Some("high".into());
    app.records[2].model = Some("claude-fable-5-1".into());
    app.records[2].effort = Some("max".into());
    app.records[3].harness = Harness::Codex;
    app.records[3].model = Some("gpt-5.6-sol".into());
    app.normalize();
    golden(
        "model_column_120x24",
        &render_to_string(&app, 120, 24, now()),
    );
}
