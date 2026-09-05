# perch — architecture

perch is a binary with no resident process. Harnesses call `perch hook` from
their own lifecycle hooks; that call reduces one event into one file. Every
other subcommand is a pure reader of those files plus one `tmux list-panes`
call.

```
harness hook --stdin JSON--> perch hook <h> --> adapter --> Event
                                                   |
                                            reducer::apply
                                                   |
                        panes/<pane>.json  <-------+-------> sound + @perch_state
                                 ^
                     list / status / next / tui (read + liveness)
```

## Prior art

| tool | model | detection | why not as-is |
|---|---|---|---|
| herdr (installed) | own server + socket API, owns terminals | harness hooks -> socket, per-agent integrations | requires migrating into herdr |
| recon (Rust) | tmux popup dashboard | `~/.claude/sessions/{pid}.json` + scraping status-bar text every 2s | Claude-only, no sound, breaks on `/clear`, scraping is fragile |
| claude-squad / agent-deck / dmux / agent-manager | create + own sessions/worktrees | tmux + worktrees | migration, worktrees denied in this setup |

Reused ideas: hook-driven state keyed by tmux pane id (herdr); popup +
status-line surface (recon).

## Event to state

An adapter (`src/adapters/`) turns raw harness JSON into a harness-neutral
`ParsedEvent { event, session_id, cwd }`. `reducer::apply` folds it into the
pane's record and returns whether the state changed.

| event | source (Claude) | next state | side effect on the record |
|---|---|---|---|
| `SessionStart` | `hook_event_name: SessionStart` | `idle` | — |
| `UserPromptSubmit` | `hook_event_name: UserPromptSubmit` | `working` | — |
| `Stop` | `hook_event_name: Stop` | `done` | `last_message` = `last_assistant_message` |
| `NeedsInput` | `Notification` with `notification_type` in `permission_prompt`, `idle_prompt`, `agent_needs_input`, `elicitation_*` | `needs_input` | `last_message` = the notification type |
| `Completed` | `Notification` with `agent_completed` | `done` | — |
| `SessionEnd` | `hook_event_name: SessionEnd` | `ended` | — |

Anything else — a subagent event (`agent_id` present), an unrecognised
notification type, `PreCompact` and friends — parses to `None` and is a no-op.
`session_id` and `cwd` are recorded whenever present; `project` is the last
path component of `cwd`. `since` moves only on an actual state change, so age
means "time in this state". `last_message` is whitespace-collapsed and capped
at 400 characters.

States rank `needs_input < done < working < starting < idle < ended`; that rank
then `since` ascending is the sort used by `list`, `next` and the TUI, so "the
first row" is always "the oldest thing waiting on you".

## State directory

`~/.local/state/perch/`, overridable with `PERCH_STATE_DIR`:

```
panes/%12.json        one record per pane, written tmp-then-rename
panes/.%12.<pid>.tmp  transient; the write in progress
events.jsonl          append-only log: ts, pane, harness, event, state, session_id
events.jsonl.1        the previous log, rotated at 5 MB
mute                  presence = globally muted
```

A record is `{pane, harness, session_id, cwd, project, branch, state, since,
last_message, title, pid, last_sound_ms}`. Unknown or missing fields default,
so an old record still loads; an unparseable one is skipped rather than fatal.

Config lives separately in `~/.config/perch/config.toml`
(`PERCH_CONFIG_DIR`), next to the generated `perch.tmux.conf`.

## Liveness

There is no process watching for exits, so liveness is computed on every read.
`store::snapshot` runs

```
tmux list-panes -a -F '#{pane_id} #{session_name} #{window_index} #{pane_current_command} #{pane_pid}'
```

and reconciles: a record whose pane id is not in that list becomes `ended` with
a fresh `since`; a record already `ended` for more than one hour is dropped and
its file deleted. The pure half, `store::reconcile(records, live_ids, now)`,
does no I/O and is unit-tested with an injected pane list. If the tmux call
fails, the live list is empty and every record reads as `ended` — degraded, but
never wrong about liveness in the direction of claiming a dead pane is alive.

## Invariants

1. **No daemon.** Nothing runs between commands. State is files; the hook is
   the only writer on the hot path.
2. **No session ownership.** perch never creates, kills, resizes or attaches a
   pane, session or worktree. The only tmux writes are `set-option -p
   @perch_state` on a pane and `select-window`/`select-pane` when the user asks
   to jump.
3. **No scraping.** Pane text and status-bar output are never parsed. State
   comes only from harness events. `watch_commands` exists in the config but
   drives no reading of pane content.
4. **Hooks never fail the harness.** `perch hook` traps every error, reports on
   stderr and returns exit code 0. Missing `afplay`, an unreadable config, a
   broken tmux — all degrade to doing less, never to a non-zero exit.
5. **Identity is `$TMUX_PANE`.** No PID tables, no session-file lookups. No
   `$TMUX_PANE` means no write.
6. **Installers merge, never replace.** Existing hook entries are preserved and
   perch appends its own; a marker (`perch hook`) makes a second run a no-op;
   the original is backed up as `<name>.bak-<timestamp>` first.

## Env overrides

Used for testing and for opting out; all read at the point of use, so a single
command can be neutered without config.

| variable | effect |
|---|---|
| `PERCH_STATE_DIR` | state directory instead of `~/.local/state/perch` |
| `PERCH_CONFIG_DIR` | config directory instead of `~/.config/perch` |
| `PERCH_NO_SOUND=1` | never spawn `afplay` or `osascript` |
| `PERCH_NO_TMUX=1` | use `NullTmux` (empty pane list, no tmux writes) |
| `PERCH_HOME` | home directory detection and every default path resolve against |
| `PERCH_NO_PATH_PROBE=1` | never probe `PATH` when detecting a harness |
| `PERCH_CLAUDE_SETTINGS` | target file for `perch install claude` |
| `PERCH_CODEX_HOOKS` | target file for `perch install codex` |
| `PERCH_PI_EXT_DIR` | directory for `perch install pi` |
| `PERCH_TMUX_CONF` | target file for the `source-file` line |

CI runs the suite with `PERCH_NO_SOUND=1 PERCH_NO_TMUX=1`; tests that touch the
store point `PERCH_STATE_DIR` at a `tempfile` directory. TUI rendering is
asserted offscreen with ratatui's `TestBackend`.
