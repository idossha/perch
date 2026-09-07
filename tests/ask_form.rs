//! The answer form, offscreen: what a pending question looks like on the
//! board, how the keys fill it in, and the answer it produces.

use chrono::Utc;
use crossterm::event::KeyCode;
use perch::model::{Harness, PaneRecord, PendingQuestion, Question, QuestionOption, State};
use perch::tui::{render_form_to_string, render_to_string, App, FormEvent, Nav};
use std::time::Instant;

fn opt(label: &str, desc: &str) -> QuestionOption {
    QuestionOption {
        label: label.into(),
        description: desc.into(),
    }
}

fn pending(deadline_secs: i64) -> PendingQuestion {
    PendingQuestion {
        tool_use_id: "toolu_01ask".into(),
        asked_at: Utc::now().to_rfc3339(),
        deadline: (Utc::now() + chrono::Duration::seconds(deadline_secs)).to_rfc3339(),
        questions: vec![
            Question {
                question: "Which store backend should perch use?".into(),
                header: "Backend".into(),
                kind: "choice".into(),
                options: vec![
                    opt("Files", "One JSON file per pane, as today"),
                    opt("SQLite", "A single database file"),
                    opt("Memory", "Nothing persisted"),
                ],
                multi_select: false,
            },
            Question {
                question: "Which harnesses need the change?".into(),
                header: "Harnesses".into(),
                kind: "choice".into(),
                options: vec![opt("claude", "Claude Code"), opt("codex", "OpenAI Codex")],
                multi_select: true,
            },
        ],
    }
}

fn board(deadline_secs: i64) -> App {
    let mut asked = PaneRecord::new("%1", Harness::Claude, &Utc::now().to_rfc3339());
    asked.state = State::NeedsInput;
    asked.project = Some("perch".into());
    asked.last_message = Some("Which store backend should perch use?".into());
    asked.question = Some(pending(deadline_secs));
    let mut other = PaneRecord::new("%2", Harness::Claude, &Utc::now().to_rfc3339());
    other.state = State::Done;
    other.project = Some("luna".into());
    let mut app = App::with_records(vec![asked, other]);
    app.normalize();
    app
}

/// The popup body's screen.
fn screen(app: &App) -> String {
    render_form_to_string(app, 120, 30)
}

/// The board's screen.
fn board_screen(app: &App) -> String {
    render_to_string(app, 120, 30, Utc::now())
}

fn key(app: &mut App, nav: &mut Nav, c: KeyCode) -> FormEvent {
    if app.form.is_some() {
        return app.form_key(c);
    }
    nav.key(app, c, Instant::now());
    FormEvent::None
}

/// The popup body opens the form; the board never does.
fn open(app: &mut App, _nav: &mut Nav) {
    app.open_form();
    assert!(
        app.form.is_some(),
        "the fixture's first row has a live question"
    );
}

/// Single choice: `j` to the second option, Enter picks it and moves on.
/// Multi choice: Space toggles, Enter confirms the set and, on the last
/// question, submits. Answers are keyed by the question text; a multi
/// answer is the labels joined with `, `.
#[test]
fn the_form_collects_a_single_and_a_multi_answer() {
    let mut app = board(600);
    let mut nav = Nav::new();
    open(&mut app, &mut nav);
    assert_eq!(key(&mut app, &mut nav, KeyCode::Char('j')), FormEvent::None);
    assert_eq!(key(&mut app, &mut nav, KeyCode::Enter), FormEvent::None);
    let s = screen(&app);
    assert!(
        s.contains("2/2") && s.contains("Harnesses"),
        "on to the second:\n{s}"
    );
    assert!(s.contains("Space toggle"), "{s}");

    key(&mut app, &mut nav, KeyCode::Char(' '));
    key(&mut app, &mut nav, KeyCode::Char('j'));
    key(&mut app, &mut nav, KeyCode::Char(' '));
    let s = screen(&app);
    assert_eq!(s.matches("◼").count(), 2, "both ticked:\n{s}");
    let ev = key(&mut app, &mut nav, KeyCode::Enter);
    let FormEvent::Submit(ans) = ev else {
        panic!("expected a submit, got {ev:?}");
    };
    assert_eq!(ans.tool_use_id, "toolu_01ask");
    assert!(!ans.defer);
    assert_eq!(
        ans.answers["Which store backend should perch use?"],
        "SQLite"
    );
    assert_eq!(
        ans.answers["Which harnesses need the change?"],
        "claude, codex"
    );
    assert!(app.form.is_none(), "submitted, closed");
}

/// `Other` opens a text line; what is typed is the answer, verbatim.
#[test]
fn other_takes_free_text() {
    let mut app = board(600);
    let mut nav = Nav::new();
    open(&mut app, &mut nav);
    key(&mut app, &mut nav, KeyCode::Char('G'));
    assert_eq!(key(&mut app, &mut nav, KeyCode::Enter), FormEvent::None);
    for c in "redis please".chars() {
        key(&mut app, &mut nav, KeyCode::Char(c));
    }
    let s = screen(&app);
    assert!(s.contains("redis please"), "{s}");
    key(&mut app, &mut nav, KeyCode::Backspace);
    key(&mut app, &mut nav, KeyCode::Char('!'));
    key(&mut app, &mut nav, KeyCode::Enter);
    // Second question: leave it unanswered.
    let ev = key(&mut app, &mut nav, KeyCode::Enter);
    let FormEvent::Submit(ans) = ev else {
        panic!("{ev:?}");
    };
    assert_eq!(
        ans.answers["Which store backend should perch use?"],
        "redis pleas!"
    );
    assert!(
        !ans.answers.contains_key("Which harnesses need the change?"),
        "an unanswered question is simply absent: {:?}",
        ans.answers
    );
}

/// `j`/`k` in the form move its own cursor, not the board's; typing `j` in
/// the text line is a letter, not a move.
#[test]
fn form_keys_never_reach_the_board() {
    let mut app = board(600);
    let mut nav = Nav::new();
    open(&mut app, &mut nav);
    key(&mut app, &mut nav, KeyCode::Char('j'));
    key(&mut app, &mut nav, KeyCode::Char('j'));
    assert_eq!(
        app.selected.as_ref().unwrap().pane,
        "%1",
        "the board did not move"
    );
    key(&mut app, &mut nav, KeyCode::Char('G'));
    key(&mut app, &mut nav, KeyCode::Enter); // Other
    key(&mut app, &mut nav, KeyCode::Char('j'));
    key(&mut app, &mut nav, KeyCode::Char('q'));
    assert!(
        render_form_to_string(&app, 100, 14).contains("jq"),
        "letters, not keys"
    );
    assert_eq!(app.selected.as_ref().unwrap().pane, "%1");
}

/// Esc hands the question to the agent's own dialog and stays where you are;
/// `p` hands it over and jumps to the pane. Both are defers; nothing else is.
#[test]
fn esc_hands_back_in_place_and_p_hands_back_and_jumps() {
    let mut app = board(600);
    let mut nav = Nav::new();
    open(&mut app, &mut nav);
    key(&mut app, &mut nav, KeyCode::Char('j'));
    let ev = key(&mut app, &mut nav, KeyCode::Esc);
    let FormEvent::HandBack(ans) = ev else {
        panic!("{ev:?}");
    };
    assert!(ans.defer);
    assert_eq!(ans.tool_use_id, "toolu_01ask");
    assert_eq!(ans.pane, "%1");
    assert!(app.form.is_none());

    open(&mut app, &mut nav);
    let ev = key(&mut app, &mut nav, KeyCode::Char('p'));
    let FormEvent::Defer(ans) = ev else {
        panic!("{ev:?}");
    };
    assert!(ans.defer);
    assert_eq!(ans.pane, "%1");
}

/// Esc inside the text line goes back to the options, not out of the form.
#[test]
fn esc_in_the_text_line_returns_to_the_options() {
    let mut app = board(600);
    let mut nav = Nav::new();
    open(&mut app, &mut nav);
    key(&mut app, &mut nav, KeyCode::Char('G'));
    key(&mut app, &mut nav, KeyCode::Enter);
    key(&mut app, &mut nav, KeyCode::Char('x'));
    assert_eq!(key(&mut app, &mut nav, KeyCode::Esc), FormEvent::None);
    assert!(app.form.is_some());
    assert!(!render_form_to_string(&app, 100, 14).contains("x_"));
}

/// A refresh while the form is open keeps the form; the question vanishing
/// (answered in the pane, or expired) closes it.
#[test]
fn the_form_survives_a_refresh_and_closes_when_the_question_is_gone() {
    let mut app = board(600);
    let mut nav = Nav::new();
    open(&mut app, &mut nav);
    let same = app.records.clone();
    app.refresh(same);
    assert!(app.form.is_some());
    let mut gone = app.records.clone();
    gone[0].question = None;
    gone[0].state = State::Working;
    app.refresh(gone);
    assert!(app.form.is_none(), "nothing left to answer");
}

/// The popup body draws the form alone, filling its window: no board, no
/// footer, the question and its options edge to edge.
#[test]
fn the_standalone_form_fills_the_frame_without_the_board() {
    let mut app = board(600);
    app.open_form();
    let s = render_form_to_string(&app, 80, 12);
    assert!(s.contains("1/2") && s.contains("Backend"), "{s}");
    assert!(s.contains("Which store backend should perch use?"), "{s}");
    assert!(s.contains("SQLite"), "{s}");
    assert!(!s.contains("location"), "no board header:\n{s}");
    assert!(!s.contains("q quit"), "no board footer:\n{s}");
    let first = s.lines().next().unwrap_or("");
    assert!(
        first.starts_with('┌'),
        "the frame is the popup's edge:\n{s}"
    );
}

/// Every question is a tab across the top: the current one highlighted, an
/// answered one ticked. Tab/`l`/Right and Shift-Tab/`h`/Left move between
/// them without losing anything, and Enter on the last tab is what sends.
#[test]
fn tabs_show_every_question_and_move_back_and_forth_keeping_answers() {
    let mut app = board(600);
    let mut nav = Nav::new();
    open(&mut app, &mut nav);
    let s = screen(&app);
    assert!(
        s.contains("Backend") && s.contains("Harnesses"),
        "both tabs:\n{s}"
    );
    let tab_line = s
        .lines()
        .find(|l| l.contains("Backend") && l.contains("Harnesses"))
        .expect("a line carrying both headers");
    assert!(!tab_line.contains('✓'), "nothing answered yet: {tab_line}");

    // Pick SQLite, which moves on to the second tab.
    key(&mut app, &mut nav, KeyCode::Char('j'));
    key(&mut app, &mut nav, KeyCode::Enter);
    let s = screen(&app);
    let tab_line = s
        .lines()
        .find(|l| l.contains("Backend") && l.contains("Harnesses"))
        .unwrap();
    assert!(
        tab_line.contains("Backend ✓"),
        "first tab ticked: {tab_line}"
    );
    assert!(s.contains("2/2"), "{s}");

    // Back with Left: the pick is still there and the cursor sits on it.
    assert_eq!(key(&mut app, &mut nav, KeyCode::Left), FormEvent::None);
    let s = screen(&app);
    assert!(s.contains("1/2"), "{s}");
    let sqlite = s.lines().find(|l| l.contains("SQLite")).unwrap();
    assert!(
        sqlite.contains("▌") && sqlite.contains("●"),
        "picked and under the cursor: {sqlite}"
    );
    // Change the mind: Memory.
    key(&mut app, &mut nav, KeyCode::Char('j'));
    key(&mut app, &mut nav, KeyCode::Char(' '));
    // Forward again with Tab, then `h` back, then `l` forward: still 2/2.
    key(&mut app, &mut nav, KeyCode::Tab);
    assert!(screen(&app).contains("2/2"));
    key(&mut app, &mut nav, KeyCode::Char('h'));
    assert!(screen(&app).contains("1/2"));
    key(&mut app, &mut nav, KeyCode::Char('l'));
    assert!(screen(&app).contains("2/2"));
    // Forward past the last tab stays on the last: moving is not sending.
    assert_eq!(key(&mut app, &mut nav, KeyCode::Right), FormEvent::None);
    assert!(app.form.is_some());
    assert!(screen(&app).contains("2/2"));

    key(&mut app, &mut nav, KeyCode::Char(' '));
    let ev = key(&mut app, &mut nav, KeyCode::Enter);
    let FormEvent::Submit(ans) = ev else {
        panic!("{ev:?}");
    };
    assert_eq!(
        ans.answers["Which store backend should perch use?"],
        "Memory"
    );
    assert_eq!(ans.answers["Which harnesses need the change?"], "claude");
}

/// Left on the first tab stays put; `h`/`l` typed in the text line are letters.
#[test]
fn tab_keys_have_edges_and_do_not_leak_into_the_text_line() {
    let mut app = board(600);
    let mut nav = Nav::new();
    open(&mut app, &mut nav);
    key(&mut app, &mut nav, KeyCode::Left);
    key(&mut app, &mut nav, KeyCode::BackTab);
    assert!(screen(&app).contains("1/2"));
    key(&mut app, &mut nav, KeyCode::Char('G'));
    key(&mut app, &mut nav, KeyCode::Enter);
    for c in "hl".chars() {
        key(&mut app, &mut nav, KeyCode::Char(c));
    }
    assert!(screen(&app).contains("hl_"), "{}", screen(&app));
    assert!(screen(&app).contains("1/2"));
}

/// The popup says whose question this is: project and branch, harness, and
/// where the pane is — enough to tell two sessions apart — and how many
/// other agents are waiting their turn behind it.
#[test]
fn the_form_title_names_the_project_agent_and_pane_and_the_queue_behind_it() {
    let mut app = board(600);
    app.records[0].branch = Some("main".into());
    app.records[0].location = Some("api".into());
    app.open_form();
    let s = render_form_to_string(&app, 90, 12);
    let title = s.lines().next().unwrap();
    for want in ["perch (main)", "claude", "api"] {
        assert!(title.contains(want), "missing {want} in title: {title}");
    }
    assert!(!title.contains("waiting"), "{title}");
    assert!(s.contains("1/2"), "position is still shown:\n{s}");

    app.waiting = 2;
    let s = render_form_to_string(&app, 90, 12);
    assert!(s.lines().next().unwrap().contains("+2 waiting"), "{s}");
    let s = render_form_to_string(&app, 120, 24);
    assert!(s.contains("p answer in its pane"), "{s}");
}

/// The board shows a question for what it is — `⚑ question`, with the text
/// as the message — whether perch's popup or the agent's dialog holds it,
/// and offers no key of its own: Enter jumps, the popup or the dialog answers.
#[test]
fn the_board_marks_a_question_and_offers_no_answer_key() {
    let app = board(600);
    let s = board_screen(&app);
    assert!(s.contains("⚑ question"), "{s}");
    assert!(s.contains("Which store backend should perch use?"), "{s}");
    assert!(!s.contains("a answer"), "{s}");
    assert!(s.contains("Enter jump   n next"), "{s}");

    // Handed back to the agent: still a question on the board.
    let mut app = board(600);
    app.records[0].question = None;
    app.records[0].deferred_question = Some("q-abc".into());
    assert!(board_screen(&app).contains("⚑ question"));

    // A permission prompt is not.
    let mut app = board(600);
    app.records[0].question = None;
    assert!(board_screen(&app).contains("⚑ needs_input"));
}

/// The popup is sized to its content: long questions and descriptions wrap
/// instead of being cut, the height follows, and the client's own size caps
/// both.
#[test]
fn the_popup_is_sized_to_its_content_and_capped_by_the_client() {
    use perch::ask::popup_size;
    let mut q = pending(600);
    let (w, h) = popup_size(&q, Some((200, 60)));
    assert!((60..=110).contains(&w), "{w}");
    // tabs, question, blank, 3 options, Other, blank, keys + border = 11
    assert_eq!(h, 11, "{h}");

    q.questions[0].question = "x".repeat(300);
    q.questions[0].options[0].description = "long ".repeat(40);
    let (w2, h2) = popup_size(&q, Some((200, 60)));
    assert_eq!(w2, 110, "width caps at the form's maximum");
    assert!(h2 > 11, "the question and the description wrap: {h2}");
    let s = {
        let mut app = board(600);
        app.records[0].question = Some(q.clone());
        app.open_form();
        render_form_to_string(&app, w2, h2)
    };
    assert_eq!(
        s.matches("long").count(),
        40,
        "the whole description is on screen:\n{s}"
    );
    assert_eq!(
        s.matches('x').count(),
        300,
        "the whole question is on screen:\n{s}"
    );

    // A small client caps both dimensions below what the content wants.
    let (w3, h3) = popup_size(&q, Some((70, 20)));
    assert!(w3 <= 66 && h3 <= 18, "{w3}x{h3}");
}

/// The popup title names the model too, when known.
#[test]
fn the_form_title_includes_the_model_when_known() {
    let mut app = board(600);
    app.records[0].model = Some("claude-opus-5".into());
    app.records[0].effort = Some("high".into());
    app.records[0].location = Some("api".into());
    app.open_form();
    let title = render_form_to_string(&app, 90, 12)
        .lines()
        .next()
        .unwrap()
        .to_string();
    assert!(title.contains("claude opus 5 high"), "{title}");
}
