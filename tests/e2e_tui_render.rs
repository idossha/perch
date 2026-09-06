//! The TUI, drawn by the real binary into a real tmux pane and read back with
//! `capture-pane`. No screenshots: every assertion is on text tmux reports.

#[path = "e2e/mod.rs"]
mod e2e;

use std::time::Duration;

use e2e::{seed_record, wait_for, Server};

#[test]
fn the_tui_renders_the_seeded_rows_in_a_real_pane() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let a = s.pane_of("one:0");
    let b = s.new_window("one", "editor");
    let host = s.new_shell_window("one", "host");
    let client = s.attach("one");

    seed_record(
        &s.state_dir,
        &a,
        "alpha",
        "needs_input",
        "2026-01-01T00:00:00Z",
    );
    seed_record(&s.state_dir, &b, "beta", "done", "2026-01-01T00:01:00Z");

    // Run the dashboard exactly where the binding runs it: inside a pane of
    // this server, told which client it is acting for.
    s.tmux(&["select-window", "-t", "one:2"]);
    s.send_keys(
        &host,
        &[
            &e2e::perch_in_pane(&format!("tui --client '{}'", client.name)),
            "Enter",
        ],
    );

    let seen = wait_for(
        || {
            let c = s.capture(&host);
            c.contains("perch") && c.contains("alpha") && c.contains("beta")
        },
        Duration::from_secs(5),
    );
    assert!(seen, "TUI never drew its rows:\n{}", s.capture(&host));

    let screen = s.capture(&host);
    // Column header, group headers, state glyphs, one-line footer.
    assert!(screen.contains("last message"), "{screen}");
    // Rows say where the pane is on the top rail, not its `%id`.
    assert!(screen.contains("location"), "{screen}");
    assert!(
        screen.contains("editor"),
        "the real window name is missing:\n{screen}"
    );
    assert!(
        !screen.contains('%'),
        "a pane id reached the screen:\n{screen}"
    );
    assert!(
        screen.contains("▸ alpha"),
        "grouping header missing:\n{screen}"
    );
    assert!(
        screen.contains("▸ beta"),
        "grouping header missing:\n{screen}"
    );
    assert!(screen.contains('⚑'), "needs_input glyph missing:\n{screen}");
    assert!(screen.contains('✓'), "done glyph missing:\n{screen}");
    let footers: Vec<&str> = screen
        .lines()
        .filter(|l| l.contains("? help") && l.contains("q quit"))
        .collect();
    assert_eq!(footers.len(), 1, "the footer is one line:\n{screen}");
    assert!(
        !screen.contains("dismiss done"),
        "help must start hidden:\n{screen}"
    );

    // `?` opens the help overlay.
    s.send_keys(&host, &["?"]);
    assert!(
        wait_for(
            || {
                let c = s.capture(&host);
                c.contains("dismiss done") && c.contains("next waiting")
            },
            Duration::from_secs(3)
        ),
        "? did not open help:\n{}",
        s.capture(&host)
    );

    // `?` closes it again, and `q` quits back to the shell prompt.
    s.send_keys(&host, &["?"]);
    assert!(wait_for(
        || !s.capture(&host).contains("dismiss done"),
        Duration::from_secs(3)
    ));
    s.send_keys(&host, &["q"]);
    assert!(
        wait_for(
            || !s.capture(&host).contains("? help"),
            Duration::from_secs(3)
        ),
        "q did not exit the TUI:\n{}",
        s.capture(&host)
    );
    // The pane is still alive, i.e. we returned to the shell, not died.
    assert!(!s.pane_of("one:2").is_empty());
}

/// The whole dashboard, drawn by the real binary in a real 120x40 pane and
/// compared verbatim with `tests/golden/e2e_dashboard_120x40.txt`.
///
/// Everything that could drift is pinned rather than normalised: the ages come
/// from `since` timestamps three and four hours in the past, so they render as
/// `3h` / `4h` for any run inside the same hour; `PERCH_NO_PATH_PROBE=1` keeps
/// the setup banner out (the fake home has no harness); the window names are
/// this test's own. The age column is normalised anyway, as a belt: a run that
/// straddles the hour boundary must fail on a layout change, not on the clock.
///
/// Same policy as the offscreen goldens: `UPDATE_GOLDENS=1` rewrites the file
/// and fails.
#[test]
fn the_real_pane_render_matches_the_golden() {
    if e2e::no_tmux() {
        return;
    }
    let s = Server::start();
    let alpha = s.new_window("one", "alpha");
    let beta = s.new_window("one", "beta");
    let host = s.new_shell_window("one", "host");
    let client = s.attach("one");

    let ago = |hours: i64| (chrono::Utc::now() - chrono::Duration::hours(hours)).to_rfc3339();
    seed_record(&s.state_dir, &alpha, "luna", "needs_input", &ago(3));
    seed_record(&s.state_dir, &beta, "perch", "done", &ago(4));

    s.tmux(&["select-window", "-t", "one:3"]);
    s.send_keys(
        &host,
        &[
            &format!(
                "env PERCH_NO_PATH_PROBE=1 {}",
                e2e::perch_in_pane(&format!("tui --client '{}'", client.name))
            ),
            "Enter",
        ],
    );
    assert!(
        wait_for(
            || {
                let c = s.capture(&host);
                c.contains("luna") && c.contains("perch") && c.contains("q quit")
            },
            Duration::from_secs(5)
        ),
        "the dashboard never drew:\n{}",
        s.capture(&host)
    );

    let screen = normalise(&s.capture(&host));
    let path = format!(
        "{}/tests/golden/e2e_dashboard_120x40.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var("UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(&path, &screen).unwrap();
        panic!("golden e2e_dashboard_120x40 was rewritten; review it and re-run without UPDATE_GOLDENS=1");
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e} (run with UPDATE_GOLDENS=1 to create it)"));
    assert_eq!(
        screen, expected,
        "\nthe real-pane render changed:\n{screen}"
    );
}

/// Trailing spaces off every line, trailing blank lines off the end, and the
/// digits of every age token (`45s`, `3h`) masked to `N`, so the comparison is
/// about layout rather than about when the test ran. Masking keeps the token's
/// width, so the columns still have to line up.
fn normalise(capture: &str) -> String {
    let mut lines: Vec<String> = capture
        .lines()
        .map(|l| {
            l.split(' ')
                .map(|tok| {
                    let is_age = tok.len() >= 2
                        && tok.chars().next_back().is_some_and(|c| "smhd".contains(c))
                        && tok[..tok.len() - 1].chars().all(|c| c.is_ascii_digit());
                    if is_age {
                        format!("{}{}", "N".repeat(tok.len() - 1), &tok[tok.len() - 1..])
                    } else {
                        tok.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
                .trim_end()
                .to_string()
        })
        .collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n") + "\n"
}
