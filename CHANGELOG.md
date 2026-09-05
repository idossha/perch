# Changelog

All notable changes to perch are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Per-pane state tracking keyed by `$TMUX_PANE`, stored as atomically written
  JSON under `~/.local/state/perch/panes/` with an append-only `events.jsonl`
  log that rotates at 5 MB.
- `perch hook claude`: Claude Code lifecycle payloads on stdin become
  `starting`/`working`/`done`/`needs_input`/`idle`/`ended`. The hook always
  exits 0, so a perch failure can never fail the agent.
- `perch tui`: ratatui dashboard of every tracked pane, sorted with whatever is
  waiting on you first — `j`/`k` move, `Enter` jumps, `n` next waiting,
  `m` mute, `x` dismiss, `r` refresh, `q` quit.
- `perch list [--json]`, `perch status [--format plain|tmux]` for the tmux
  status line, and `perch next` to jump to the oldest waiting pane.
- Sound on transitions into `done` and `needs_input` via `afplay`, with a
  per-pane cooldown and a global mute file toggled from the TUI; optional
  `osascript` desktop notification.
- `perch sound test <event>` to check the configured sound.
- `perch install claude` and `perch install tmux`: idempotent, backup-first
  merges that never rewrite what is already in your config, with `--dry-run`,
  `--print` and (tmux) `--apply`.
- Config at `~/.config/perch/config.toml` — sound names, `notify`,
  `cooldown_secs`.
- README, `docs/ARCHITECTURE.md`, `docs/DECISIONS.md`, and CI running fmt,
  clippy and the test suite on Linux and macOS.

- `perch setup`: detects every installed harness, runs its installer, wires the
  tmux popup binding and reloads tmux, prints a component table and records the
  run in `~/.config/perch/setup.json`. Idempotent; `--dry-run`, `--no-tmux` and
  `--only claude,codex,pi,tmux` narrow it.
- `perch doctor [--json]`: binary and version, each harness (found? hooked?),
  the tmux binding and `source-file` line, the sound player, the state
  directory and record count, and the mute flag. Exits 1 when a harness you
  have is not wired.
- `perch uninstall [--dry-run] [--keep-state]`: removes only the hook entries
  perch added, the pi extension, the tmux config and its `source-file` line,
  and the state directory.
- First-run nudge: a TUI banner with `S` to run setup, and a one-line hint on
  stderr from `list` and `status`, until perch is wired.
- Install paths: a Homebrew tap (`brew install idossha/perch/perch`),
  `cargo install perch` / `cargo binstall perch`, and `install.sh`, which
  downloads the latest release and runs `perch setup` for you. Release
  workflow builds four targets on a `v*` tag and updates the tap formula.
- MIT LICENSE.

### Known limitations

- Homebrew cannot write to `$HOME`, so a brew install needs one `perch setup`;
  the formula says so in its caveats.
- Sound and desktop notifications are macOS-only.
- Nothing is tracked outside tmux: with no `$TMUX_PANE`, the hook is a no-op.

[Unreleased]: https://github.com/idohaber/perch/compare/main...HEAD
