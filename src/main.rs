use std::process::ExitCode;

use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};

use perch::model::{Harness, State};
use perch::paths::Paths;
use perch::setup::{self, SetupOpts, UninstallOpts};
use perch::{config, hook, install, notify, sound, store, tmux, tui};

#[derive(Parser)]
#[command(
    name = "perch",
    version,
    about = "tmux-native home base for coding agents"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum HarnessArg {
    Claude,
    Codex,
    Pi,
}

impl From<HarnessArg> for Harness {
    fn from(h: HarnessArg) -> Self {
        match h {
            HarnessArg::Claude => Harness::Claude,
            HarnessArg::Codex => Harness::Codex,
            HarnessArg::Pi => Harness::Pi,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum InstallTarget {
    Claude,
    Codex,
    Pi,
    Tmux,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum StatusFormat {
    Plain,
    Tmux,
}

#[derive(Subcommand)]
enum Cmd {
    /// Read a hook payload on stdin and update this pane's record.
    Hook {
        harness: HarnessArg,
        /// The `AskUserQuestion` variant: record the question, then wait for
        /// an answer from the dashboard and hand it back to the harness.
        #[arg(long)]
        ask: bool,
    },
    /// Snapshot of every tracked pane.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One-line summary, for the tmux status line.
    Status {
        #[arg(long, value_enum, default_value = "plain")]
        format: StatusFormat,
    },
    /// Jump a client to the oldest waiting pane.
    Next {
        /// The client to move. Always pass it: `#{client_name}` from a binding.
        #[arg(long)]
        client: Option<String>,
    },
    /// Mark seen panes idle. With no argument, every `done` pane a focused
    /// client is showing; with one, that pane unconditionally.
    Seen { pane: Option<String> },
    /// The dashboard (run inside `tmux display-popup`; see `perch open`).
    Tui {
        /// The client Enter moves. `display-popup` cannot expand `#{…}`, so
        /// the launcher has to pass it in.
        #[arg(long)]
        client: Option<String>,
    },
    /// Open the dashboard in a popup on a client.
    Open {
        /// The client to draw on and to move. Pass `#{client_name}`.
        #[arg(long)]
        client: Option<String>,
    },
    /// Draw the centered notification card for a pane on every client.
    ///
    /// Spawned detached by the hook; `perch notify test` draws a sample.
    Notify {
        /// done | needs_input | test
        kind: String,
        /// The pane whose record the card describes.
        #[arg(long)]
        pane: Option<String>,
    },
    /// The card's popup body (run by `notify` inside `display-popup`).
    #[command(hide = true)]
    NotifyBody {
        kind: String,
        duration_ms: u64,
        /// The client this popup is drawn on: where a key is forwarded.
        #[arg(long)]
        client: Option<String>,
        /// The card's lines, in order. Passed three times.
        #[arg(long = "line")]
        lines: Vec<String>,
    },
    /// Open the answer form for a pane's question as a popup on every client.
    ///
    /// Spawned detached by the `--ask` hook.
    #[command(hide = true)]
    AskPopup {
        #[arg(long)]
        pane: String,
    },
    /// The form popup's body (run by `ask-popup` inside `display-popup`).
    #[command(hide = true)]
    AskForm {
        #[arg(long)]
        pane: String,
        /// The client this popup is drawn on: where `p` jumps.
        #[arg(long)]
        client: Option<String>,
    },
    /// Serve the `ask_user` tool over MCP on stdio (registered in Codex by
    /// `perch install codex`).
    Mcp,
    /// Sound helpers.
    #[command(subcommand)]
    Sound(SoundCmd),
    /// Merge perch into a harness or tmux config.
    Install {
        target: InstallTarget,
        /// Report what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Print the merged file / snippet to stdout instead of installing.
        #[arg(long)]
        print: bool,
        /// tmux only: append the `source-file` line to ~/.tmux.conf.
        #[arg(long)]
        apply: bool,
    },
    /// Detect every installed harness and wire perch into all of them.
    Setup {
        /// Report what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Accepted for scripts; setup never prompts.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Leave tmux alone.
        #[arg(long)]
        no_tmux: bool,
        /// Restrict to these components (claude,codex,pi,tmux).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
    },
    /// Report what is installed, wired and reachable.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Remove everything perch installed.
    Uninstall {
        #[arg(long)]
        dry_run: bool,
        /// Keep the state directory.
        #[arg(long)]
        keep_state: bool,
    },
}

#[derive(Subcommand)]
enum SoundCmd {
    /// Play the sound configured for an event (done | needs_input | error).
    Test { event: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // A hook must never fail its harness: report and exit 0 regardless.
    if let Cmd::Hook { harness, ask } = cli.cmd {
        if let Err(e) = hook::run(harness.into(), ask) {
            eprintln!("perch: hook error: {e:#}");
        }
        return ExitCode::SUCCESS;
    }
    match dispatch(cli.cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("perch: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Hook { .. } => unreachable!("handled in main"),
        Cmd::List { json } => {
            nudge_stderr();
            cmd_list(json)
        }
        Cmd::Status { format } => {
            nudge_stderr();
            cmd_status(format)
        }
        Cmd::Next { client } => cmd_next(client.as_deref()),
        Cmd::Seen { pane } => {
            match pane {
                Some(p) => {
                    hook::seen(&p);
                }
                None => {
                    hook::seen_all();
                }
            }
            Ok(())
        }
        Cmd::Tui { client } => {
            let client = require_client(client.as_deref())?;
            tui::run(tmux::current().as_ref(), &client)
        }
        Cmd::Open { client } => cmd_open(client.as_deref()),
        Cmd::Notify { kind, pane } => cmd_notify(&kind, pane.as_deref()),
        Cmd::NotifyBody {
            kind,
            duration_ms,
            client,
            lines,
        } => {
            let kind = notify::Kind::parse(&kind)
                .ok_or_else(|| anyhow::anyhow!("unknown kind: {kind}"))?;
            notify::body(kind, duration_ms, &lines, client.as_deref());
            Ok(())
        }
        Cmd::Mcp => {
            perch::mcp::serve();
            Ok(())
        }
        Cmd::AskPopup { pane } => {
            perch::ask::show(tmux::current().as_ref(), &pane);
            Ok(())
        }
        Cmd::AskForm { pane, client } => {
            let client = require_client(client.as_deref())?;
            perch::ask::form_body(tmux::current().as_ref(), &pane, &client)
        }
        Cmd::Sound(SoundCmd::Test { event }) => {
            let cfg = config::load();
            match cfg.sound_for(&event) {
                Some(name) => {
                    println!("playing {name} for {event}");
                    sound::play(&cfg, &event);
                    Ok(())
                }
                None => anyhow::bail!("unknown event: {event} (done | needs_input | error)"),
            }
        }
        Cmd::Setup {
            dry_run,
            yes: _,
            no_tmux,
            only,
        } => {
            let paths = Paths::from_env();
            let report = setup::run(
                &paths,
                &SetupOpts {
                    dry_run,
                    no_tmux,
                    only,
                },
            )?;
            print!("{report}");
            Ok(())
        }
        Cmd::Doctor { json } => cmd_doctor(json),
        Cmd::Uninstall {
            dry_run,
            keep_state,
        } => {
            let report = setup::uninstall(
                &Paths::from_env(),
                &UninstallOpts {
                    dry_run,
                    keep_state,
                },
            )?;
            print!("{report}");
            Ok(())
        }
        Cmd::Install {
            target,
            dry_run,
            print,
            apply,
        } => {
            let report = match target {
                InstallTarget::Claude => install::install_claude(dry_run, print)?,
                InstallTarget::Codex => install::install_codex(dry_run, print)?,
                InstallTarget::Pi => install::install_pi(dry_run, print)?,
                InstallTarget::Tmux => install::install_tmux(dry_run, apply, print)?,
            };
            print!("{report}");
            Ok(())
        }
    }
}

/// One-line hint on stderr; stdout belongs to the tmux status line.
fn nudge_stderr() {
    let paths = Paths::from_env();
    if setup::needs_nudge(&paths) {
        eprintln!(
            "perch: not wired into {}: run `perch setup`",
            setup::unwired_harnesses(&paths).join(", ")
        );
    }
}

fn cmd_doctor(json: bool) -> anyhow::Result<()> {
    let d = setup::doctor(&Paths::from_env());
    if json {
        println!("{}", serde_json::to_string_pretty(&d)?);
    } else {
        print!("{}", setup::doctor_text(&d));
    }
    if d.ok() {
        Ok(())
    } else {
        anyhow::bail!("unwired: {}", d.unwired.join(", "))
    }
}

fn cmd_list(json: bool) -> anyhow::Result<()> {
    let recs = store::snapshot(tmux::current().as_ref());
    if json {
        // `state` is the pane's own; `effective_state` is what the user is
        // shown, and the two differ exactly while the pane is delegating.
        let mut out = Vec::new();
        for r in &recs {
            let mut v = serde_json::to_value(r)?;
            if let Some(o) = v.as_object_mut() {
                o.insert(
                    "effective_state".into(),
                    serde_json::json!(r.effective_state().as_str()),
                );
            }
            out.push(v);
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    let now = Utc::now();
    for r in &recs {
        let cells = tui::row_cells(r, now);
        println!(
            "{:<6} {:<20} {:<7} {:<14} {:<11} {:>5}  {}",
            cells[0], cells[1], cells[2], cells[3], cells[4], cells[5], cells[6]
        );
    }
    Ok(())
}

fn cmd_status(format: StatusFormat) -> anyhow::Result<()> {
    let recs = store::snapshot(tmux::current().as_ref());
    let count = |s: State| recs.iter().filter(|r| r.effective_state() == s).count();
    let (waiting, done, working) = (
        count(State::NeedsInput),
        count(State::Done),
        count(State::Working),
    );
    match format {
        StatusFormat::Tmux => {
            let mut parts = Vec::new();
            if waiting > 0 {
                parts.push(format!("#[fg=yellow]⚑{waiting}#[default]"));
            }
            if working > 0 {
                parts.push(format!("#[fg=cyan]▶{working}#[default]"));
            }
            if done > 0 {
                parts.push(format!("#[fg=green]✓{done}#[default]"));
            }
            println!("{}", parts.join(" "));
        }
        StatusFormat::Plain => println!("⚑{waiting} ▶{working} ✓{done}"),
    }
    Ok(())
}

/// The client for a client-moving command, or a hard error.
fn require_client(explicit: Option<&str>) -> anyhow::Result<String> {
    tmux::resolve_client(explicit)
        .ok_or_else(|| anyhow::anyhow!("{}", r##"no client: pass --client "#{client_name}""##))
}

/// Draw the dashboard on one client's screen.
///
/// The popup is what makes the client knowable: `display-popup` does not expand
/// `#{…}` in its shell-command, so the launcher resolves the client and passes
/// it to the TUI as an argument.
fn cmd_open(client: Option<&str>) -> anyhow::Result<()> {
    let client = require_client(client)?;
    let exe = std::env::current_exe()?;
    let cmd: Vec<String> = vec![
        "display-popup".into(),
        "-c".into(),
        client.clone(),
        "-E".into(),
        "-w".into(),
        "85%".into(),
        "-h".into(),
        "75%".into(),
        "--".into(),
        exe.to_string_lossy().into_owned(),
        "tui".into(),
        "--client".into(),
        client,
    ];
    if tmux::debug() {
        eprintln!("perch: tmux {}", cmd.join(" "));
    }
    if !tmux::current().run_checked(&[cmd]) {
        anyhow::bail!("tmux refused to open the popup");
    }
    Ok(())
}

/// Draw the card for one pane on every attached client.
///
/// `test` needs no record: it is how a user checks placement and colours.
fn cmd_notify(kind: &str, pane: Option<&str>) -> anyhow::Result<()> {
    let kind = notify::Kind::parse(kind)
        .ok_or_else(|| anyhow::anyhow!("unknown kind: {kind} (done | needs_input | test)"))?;
    let rec = match (kind, pane) {
        (notify::Kind::Test, None) => notify::sample_record(),
        (_, Some(p)) => store::load(p).ok_or_else(|| anyhow::anyhow!("no record for pane {p}"))?,
        (_, None) => anyhow::bail!("notify {}: pass --pane <pane>", kind.as_str()),
    };
    notify::show(tmux::current().as_ref(), &config::load(), kind, &rec);
    Ok(())
}

/// Jump a client to the oldest thing waiting on the human.
///
/// `needs_input` before `done`, oldest `since` first — the same order the
/// dashboard's `n` uses.
fn cmd_next(client: Option<&str>) -> anyhow::Result<()> {
    let t = tmux::current();
    let recs = store::snapshot(t.as_ref());
    let oldest = |want: State| {
        recs.iter()
            .filter(|r| r.effective_state() == want)
            .min_by(|a, b| a.since.cmp(&b.since))
    };
    let Some(target) = oldest(State::NeedsInput).or_else(|| oldest(State::Done)) else {
        anyhow::bail!("nothing waiting");
    };
    println!("{}", target.pane);
    let client = require_client(client)?;
    tui::jump_to(t.as_ref(), &client, &target.pane).map_err(|e| anyhow::anyhow!(e))
}
