use std::process::ExitCode;

use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};

use perch::model::{Harness, State};
use perch::{config, hook, install, sound, store, tmux, tui};

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
    Hook { harness: HarnessArg },
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
    /// Jump the current client to the oldest waiting pane.
    Next,
    /// The dashboard (run inside `tmux display-popup`).
    Tui,
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
}

#[derive(Subcommand)]
enum SoundCmd {
    /// Play the sound configured for an event (done | needs_input | error).
    Test { event: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // A hook must never fail its harness: report and exit 0 regardless.
    if let Cmd::Hook { harness } = cli.cmd {
        if let Err(e) = hook::run(harness.into()) {
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
        Cmd::List { json } => cmd_list(json),
        Cmd::Status { format } => cmd_status(format),
        Cmd::Next => cmd_next(),
        Cmd::Tui => tui::run(tmux::current().as_ref()),
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
        Cmd::Install {
            target,
            dry_run,
            print,
            apply,
        } => {
            let report = match target {
                InstallTarget::Claude => install::install_claude(dry_run, print)?,
                InstallTarget::Tmux => install::install_tmux(dry_run, apply, print)?,
                InstallTarget::Codex | InstallTarget::Pi => {
                    anyhow::bail!("codex and pi installers land in phase 3")
                }
            };
            print!("{report}");
            Ok(())
        }
    }
}

fn cmd_list(json: bool) -> anyhow::Result<()> {
    let recs = store::snapshot(tmux::current().as_ref());
    if json {
        println!("{}", serde_json::to_string_pretty(&recs)?);
        return Ok(());
    }
    let now = Utc::now();
    for r in &recs {
        let cells = tui::row_cells(r, now);
        println!(
            "{:<6} {:<20} {:<7} {:<11} {:>5}  {}",
            cells[0], cells[1], cells[2], cells[3], cells[4], cells[5]
        );
    }
    Ok(())
}

fn cmd_status(format: StatusFormat) -> anyhow::Result<()> {
    let recs = store::snapshot(tmux::current().as_ref());
    let count = |s: State| recs.iter().filter(|r| r.state == s).count();
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

fn cmd_next() -> anyhow::Result<()> {
    let t = tmux::current();
    let recs = store::snapshot(t.as_ref());
    let Some(target) = recs
        .iter()
        .find(|r| matches!(r.state, State::NeedsInput | State::Done))
    else {
        println!("nothing waiting");
        return Ok(());
    };
    println!("{}", target.pane);
    t.focus(&target.pane);
    Ok(())
}
