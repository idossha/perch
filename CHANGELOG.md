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
- `perch tui`: ratatui dashboard of every tracked pane, grouped by project with
  the most urgent group first, a glyph plus a colour per state, `ended` panes
  collapsed behind a footer count, and indented rows for a pane's subagents —
  `j`/`k` move, `Enter` jumps (a subagent jumps to its parent pane), `n` next
  waiting, `m` mute, `x` dismiss, `e` show/hide ended, `g` grouped/flat,
  `?` key help, `r` refresh, `q` quit. `dark` and `light` palettes, chosen with
  `PERCH_THEME` or `[tui] theme`.
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
- `perch install codex` / `perch setup` now record perch's hooks as trusted in
  `~/.codex/config.toml`, so codex actually runs them without you accepting a
  prompt first. `perch doctor` reports `trust: yes/no`, `perch uninstall`
  removes only those keys, and the rest of the file keeps its formatting.
  Re-run `perch setup` after reordering `~/.codex/hooks.json`.
- Instant cue on `done` / `needs_input`: a one-line `tmux display-message` on
  every attached client, and a per-window `@perch_flag` (`⚑` / `✓`) that the
  tmux snippet appends to the window status. The snippet also binds
  `prefix + N` to `perch next`, which now switches session before selecting the
  pane, so it works across sessions. Configured under `[notify]`:
  `tmux_message` (default true), `duration_ms` (4000), `desktop` (false).
  Everything is batched into one spawned tmux call.
- `[notify]` replaces the old top-level `notify = true`, which is still read and
  understood as `[notify] desktop = true`.
- Subagents: `SubagentStart` / `SubagentStop` (and `Stop` / `Notification`
  carrying an `agent_id`) from Claude and Codex now appear as `children` of
  their parent pane in `perch list --json` and in the TUI, instead of being
  dropped. They never change the pane's own state and are silent, except a
  subagent `agent_needs_input` notification, which sounds. Finished children
  are cleared by the next turn, or after ten minutes. `perch setup` adds the
  two new hook entries to an existing install.
- `PERCH_DUMP_HOOK_INPUT=<dir>` copies every raw hook payload to
  `<dir>/<harness>-<event>-<ts>.json` for checking field mappings.
- The codex adapter accepts `hook_event_name`/`event`,
  `session_id`/`thread_id`/`turn_id` and
  `last_assistant_message`/`last_message`.
- MIT LICENSE.

### Fixed

- `Enter` in the dashboard and `prefix + N` move the client again. The tmux
  commands that move a client were being spawned and never waited on, so the
  popup closed, took its pty with it and killed the child before tmux ran it.
  Client moves are now synchronous, and `Enter` resolves the pane's session
  from tmux so a jump out of the popup crosses sessions like `perch next`.
- Panes no longer sit in `needs_input` when nothing is waiting. `needs_input`
  now means only a real approval or question dialog — `permission_prompt`,
  `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input` and codex
  `PermissionRequest`. Claude's `idle_prompt` nudge (and `auth_success`,
  `quota_*`) is recorded in `events.jsonl` and changes nothing, and a stale
  `needs_input` clears on the next `PreToolUse`, on `elicitation_complete` /
  `elicitation_response`, or on your next prompt.
- `done` now means "finished while you were elsewhere": a turn that ends in the
  pane you are watching goes to `idle` silently. New `perch seen <pane>` marks
  a pane seen (`done` → `idle`, flag cleared); the tmux snippet runs it from
  `pane-focus-in`, and `perch next` calls it for the pane it moves you to.
  Re-run `perch setup` to pick up the `PreToolUse` hook and the new snippet.

### Known limitations

- Homebrew cannot write to `$HOME`, so a brew install needs one `perch setup`;
  the formula says so in its caveats.
- Sound and desktop notifications are macOS-only.
- Nothing is tracked outside tmux: with no `$TMUX_PANE`, the hook is a no-op.

[Unreleased]: https://github.com/idohaber/perch/compare/main...HEAD
