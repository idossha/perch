//! The TUI, drawn by the real binary into a real tmux pane and read back with
//! `capture-pane`. No screenshots: every assertion is on text tmux reports.

#[path = "e2e/mod.rs"]
mod e2e;

use std::time::Duration;

use e2e::{seed_record, wait_for, Server, PERCH_BIN};

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
            &format!("{PERCH_BIN} tui --client '{}'", client.name),
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
