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

### Known limitations

- `perch install codex` and `perch install pi` are recognised but not yet
  implemented; they exit with a message.
- Sound and desktop notifications are macOS-only.
- Nothing is tracked outside tmux: with no `$TMUX_PANE`, the hook is a no-op.

[Unreleased]: https://github.com/idohaber/perch/compare/main...HEAD
