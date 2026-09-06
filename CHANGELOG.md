# Changelog

All notable changes to perch are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A centered notification card: on `done` or `needs input`, a borderless
  three-line popup fades in at the middle of every attached client showing the
  project and branch, the tmux location and harness, and the agent's last
  message, then fades out after 3.5 s. The first key you type dismisses it and
  is forwarded to the pane underneath, so it can never eat a character.
- `perch notify test` draws a sample card, for checking placement and colours.
- `[notify]` config: `enabled`, `duration_ms`, and `done_style` /
  `needs_input_style` fade overrides. See `docs/NOTIFICATIONS.md`.

### Changed

- **Finished subagents are a count, not a wall of rows.** A pane with a big
  fan-out now shows `claude ▶4 ✓48` in its harness column and one row per
  subagent that is actually running or blocked on you. Press `Space` (or `Tab`)
  on the pane to unfold the finished ones, newest first, and again to fold them
  away. Nothing is hidden that is waiting on you: a blocked subagent always has
  a row.
- **A pane whose subagents are still working now reads `delegating`.** Claude
  runs subagents in the background: the main agent's turn ends while they keep
  going, and it is woken again when each finishes. Such a pane sorts with the
  working ones, shows `▶ delegating`, and is skipped by `perch next`. `perch
  list --json` gained an `effective_state` field next to `state`, and the
  `@perch_state` pane option now carries the effective state.

### Fixed

- **A pane with running subagents is no longer reported as finished.** Its turn
  ending used to mark every subagent done, chime, and draw a done card — so
  perch told you a job was over while three agents were still working on it,
  and threw away their rows. The turn ending now moves only the pane's own
  state; the chime and the card come when the work is actually in.
- **Subagents no longer sit at `working` forever.** A subagent whose
  `SubagentStop` the harness never sent is retired when the session ends, when
  its pane goes away, or after two hours — so a lost event can no longer leave
  a pane delegating to a child that finished long ago.
- **A pane you have seen sheds its finished subagents.** Previously only a new
  prompt or a `Stop` cleared them, so a pane reached `idle` still carrying the
  last turn's children. Every path to `idle` now clears them, and a pane keeps
  at most twenty subagents whatever happens.

- **`done` and `idle` are trustworthy again.** Whether a pane has been "seen"
  is now worked out from the live tmux client list every time anything reads
  the board, instead of only when one of three tmux hooks happened to fire.
  Panes you had looked at kept showing `done` because `prefix n`, `prefix p`,
  `prefix l`, `last-pane` and session pickers fire none of those hooks; now it
  does not matter how you got there. The tmux hooks are still installed, but
  they only make the change instant — nothing is lost if they never run.
- **A pane behind your browser no longer counts as seen.** Deciding a finished
  turn used to ask only whether the pane was the active pane of an attached
  session; it now requires a client with the terminal's own keyboard focus, so
  a turn that ends while you are in another app is `done` and chimes. On a
  terminal that does not report focus at all, any client showing the pane
  still counts, so nothing regresses there.

### Changed

- **`perch seen` takes no argument now** and re-checks every finished pane;
  `perch seen <pane>` still marks one pane seen unconditionally. The tmux
  snippet points all its hooks at the argument-less form and adds
  `window-pane-changed`, `session-window-changed` and `client-focus-in`, plus
  `set -g focus-events on`. Re-run `perch setup` to pick them up.
- `PERCH_FAKE_CLIENTS` and `PERCH_FAKE_PANE_FOCUSED` are replaced by one
  `PERCH_FAKE_VIEWERS` (`"%1:focused,%2"`).

## [0.2.0] - 2026-09-05

### Changed

- **Rows show the tmux location instead of the pane id.** The board's first
  column is now the window name a user actually navigates by — with
  `session/` in front only when more than one session is on the board, and
  `.N` after it only when the window is split — instead of a `%444` that means
  nothing outside perch. Grouped rows no longer repeat the project under their
  own `▸ project (branch)` header, and an `ended` pane still says where it
  was. The pane id remains in `perch list --json` and appears on screen only
  under `PERCH_DEBUG=1`.
- **`gg` and `G` are plain vim now, and `v` toggles the view.** `g` used to
  both toggle grouped/flat and start `gg`, so going to the top of the board
  also flipped the layout under you. `gg` (a second `g` within 500 ms) goes to
  the first row, `G` to the last, `v` switches grouped ⇄ flat, and a lone `g`
  does nothing.
- **Navigation is now a contract.** Every jump is exactly one
  `tmux switch-client -c <client_name> -t <pane_id>`, with the client always
  named: `perch tui --client`, `perch next --client` and the new `perch open
  --client`. A tmux command without `-c` picks a client by heuristic, which is
  why a jump from the dashboard sometimes landed on the wrong pane or nowhere;
  it can no longer happen. Jumps are waited on and checked, the pane is
  confirmed live first, and a failure shows `pane %N is gone` in the dashboard
  instead of closing it.
- The dashboard's cursor is keyed by pane id rather than by row number, so the
  board reordering under you can never move your selection to another agent.
- The footer is one line — `Enter jump  n next  ? help  q quit` plus a
  right-aligned `[sound on] [grouped] N panes` — and `?` now opens a centered
  help overlay with the full key list and a legend explaining each state. Any
  key closes it. `gg` / `G` jump to the first / last row, and the arrow keys
  work everywhere `j`/`k` do.
- `perch next` prefers the oldest `needs_input` over the oldest `done` and
  exits 1 with `nothing waiting`.
- `perch tui` renders "no agents yet" on an empty state directory instead of a
  bare board, and never panics without tmux.
- `PERCH_DEBUG=1` logs each jump's exact tmux argv to stderr.

### Removed

- **The toast popup, and every visual cue with it.** `perch toast`, `perch
  toast-body`, the `[toast]` and `[notify]` config tables, the macOS
  `osascript` banner and the `unicode-width` dependency are gone. Sound is the
  only thing perch does to get your attention; the `@perch_state` pane option
  stays, for status lines you write yourself. An old config with `[toast]` or
  `[notify]` still loads — the retired tables are ignored.

### Migration

- Re-run `perch setup` (or `perch install tmux`) to get the new bindings:
  `bind g run-shell 'perch open --client "#{client_name}"'` and the matching
  `bind N`. The old `bind g display-popup ... 'perch tui'` cannot tell perch
  which client to move.

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
  waiting, `m` mute, `x` dismiss, `e` show/hide ended, `v` grouped/flat,
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
- Instant cue on `done` / `needs_input`: a toast in the bottom-right corner of
  every attached client (see *Changed* below) plus a `@perch_state` pane
  option. The tmux snippet also binds `prefix + N` to `perch next`, which
  switches session before selecting the pane, so it works across sessions.
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

### Changed

- The `done` / `needs_input` cue is a toast: a borderless one-line popup in
  the bottom-right corner of every attached client, green `✓` or red `⚑`, that
  fades out over its last 600 ms. The first key you press dismisses it *and* is
  passed through to the pane you were typing at, so it can never eat a
  character. `perch toast test` draws a sample. Configured under `[toast]`:
  `enabled` (true), `duration_ms` (3000), `done_style`, `needs_input_style`.
- perch no longer writes into your window list or your status line: the
  `#{@perch_flag}` window-status marker and the per-client `display-message`
  flash are gone, with the `[notify] tmux_message` and `duration_ms` keys that
  governed them. `[notify] desktop` is unchanged. Re-run `perch setup` (or
  `perch install tmux --apply`) to drop the `window-status-format` lines from
  an earlier install.

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
