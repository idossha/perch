# perch — plan (2026-09-05)

A thin, daemonless, tmux-native home base for coding agents. Rust. Claude Code first;
Codex and pi through the same event schema. It never owns sessions, panes or worktrees:
it observes what already runs in the user's tmux and gives it a visual and auditory surface.

## Prior art surveyed

| tool | model | detection | why not as-is |
|---|---|---|---|
| herdr (installed) | own server + socket API, owns terminals | harness hooks -> socket, per-agent integrations | requires migrating into herdr |
| recon (Rust) | tmux popup dashboard | `~/.claude/sessions/{pid}.json` + scraping status-bar text every 2s | Claude-only, no sound, breaks on `/clear`, scraping is fragile |
| claude-squad / agent-deck / dmux / agent-manager | create + own sessions/worktrees | tmux + worktrees | migration, worktrees denied in this setup |

Reused ideas: hook-driven state keyed by tmux pane id (herdr); popup + status-line surface (recon).

## Identity and state

- Key = tmux pane id (`%NN`) taken from `$TMUX_PANE`, inherited by hook processes.
- Store = `~/.local/state/perch/panes/<pane>.json` written atomically (tmp + rename); `events.jsonl` append-only log (rotated at 5 MB).
- Record: `{pane, harness, session_id, cwd, project, branch, state, since, last_message, title, pid}`.
- States: `starting | working | done | needs_input | idle | ended`. Reducer:
  - SessionStart -> idle; UserPromptSubmit -> working; Stop -> done (+ last_assistant_message);
    Notification{permission_prompt|idle_prompt|agent_needs_input|elicitation_*} -> needs_input;
    Notification{agent_completed} -> done; SessionEnd -> ended.
  - Subagent events (`agent_id` present) are ignored.
- Liveness: on every read, `tmux list-panes -a -F '#{pane_id} #{session_name} #{window_index} #{pane_current_command} #{pane_pid}'`; a record whose pane is gone is `ended` and pruned after 1 h.

## Binary surface

```
perch hook <claude|codex|pi>      # stdin JSON -> reducer -> file + sound + tmux pane option @perch_state
perch tui                          # ratatui dashboard (run inside display-popup)
perch status [--format tmux]       # one-line summary for status-right, e.g. "⚑2 ▶1 ✓1"
perch list [--json]                # snapshot for scripting
perch next                         # jump the current client to the oldest needs_input/done pane
perch install <claude|codex|pi|tmux>  # merge hooks/extension/keybinding; idempotent; backs up
perch sound test <event>           # play the configured sound
```

- Sounds: `afplay` (macOS) in a detached child, per-event sound from config, per-pane cooldown 3 s,
  global mute via `~/.local/state/perch/mute` toggled from the TUI with `m`. Optional desktop
  notification via `osascript` when `notify = true`.
- Config: `~/.config/perch/config.toml` with defaults `done = "Glass"`, `needs_input = "Ping"`, `error = "Basso"`.
- tmux: hook sets `@perch_state` on the pane; `perch install tmux` appends
  `bind g display-popup -E -w 85% -h 75% 'perch tui'` and a `#(perch status --format tmux)` hint to
  a `~/.tmux.conf.d/perch.conf` style include (never edits `.tmux.conf` in place beyond a `source-file` line).

## TUI

Table: pane, project (branch), harness, state (colour), age, last message (truncated). Sorted:
needs_input, done, working, idle. Keys: `j/k` move, `Enter` jump (select-window + select-pane via
`tmux` then exit), `n` next waiting, `m` mute, `x` dismiss done -> idle, `r` refresh, `q` quit.
Refresh 1 s from files (cheap). Rendering tested offscreen with ratatui `TestBackend`.

## Harness adapters

- **claude**: `perch install claude` merges hook entries for SessionStart, UserPromptSubmit, Stop,
  Notification, SessionEnd into `~/.claude/settings.json` beside the existing idosleep/herdr hooks.
- **codex**: same events into `~/.codex/hooks.json` (`perch hook codex`); field names mapped in the adapter.
- **pi**: writes `~/.pi/agent/extensions/perch.ts` that spawns `perch hook pi` on `agent_start`,
  `agent_end`, `session_start`, `session_shutdown` with a JSON payload on stdin.
- **unknown**: no scraping in v1; a pane running a process not reporting is shown as `unknown` only
  if listed in config `watch_commands`.

## Phases

1. Crate skeleton, state model + reducer + store, `hook claude`, sound, `list`, `status` — unit tests on reducer and adapters with fixture JSON.
2. `tui`, `next`, `install tmux`, `install claude`.
3. `install codex`, `install pi` + adapters.
4. README, `docs/ARCHITECTURE.md`, decision log, CI (cargo test + clippy).

## Non-goals

No daemon, no session creation, no worktrees, no scraping of pane text, no replacement of herdr's API.
