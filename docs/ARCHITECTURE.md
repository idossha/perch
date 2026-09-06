# perch — architecture

Why any of this is shaped the way it is: [PHILOSOPHY.md](PHILOSOPHY.md). This
document is the mechanism; that one is the reason.

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
| `Stop` | `hook_event_name: Stop` | `idle` if the pane is focused, else `done` | `last_message` = `last_assistant_message` |
| `NeedsInput` | `Notification` with `notification_type` in `permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input`; codex `PermissionRequest` | `needs_input` | `last_message` = the notification type |
| `Completed` | `Notification` with `agent_completed` | `done` | — |
| `ToolUse` | `PreToolUse`, or `Notification` `elicitation_complete` / `elicitation_response` | `working`, and only from `needs_input` | — |
| `Observed` | any other `Notification` — `idle_prompt`, `auth_success`, `quota_*` | unchanged | logged only |
| `SessionEnd` | `hook_event_name: SessionEnd` | `ended` | — |

The states mean what they mean in herdr, which is where the vocabulary comes
from:

- **`working`** — a turn is in progress.
- **`needs_input`** — a real approval or question is on screen and the agent is
  blocked on you. Nothing else sets it; in particular `idle_prompt`, Claude's
  "you have been idle" nudge, is not a request and never does.
- **`done`** — the turn finished and the pane has not been seen since.
- **`idle`** — ready for input, and seen.

**`done` means finished while you were elsewhere; `idle` means you have seen
it.** A `Stop` on a seen pane goes to `idle` and stays silent; every other
`Stop` is `done` and chimes. `needs_input` always sounds.

## Seen

Seen is a **property of the live client list**, evaluated on every read — not a
side effect of a hook firing. One tmux call is the whole input:

```
tmux list-clients -F '#{pane_id}\t#{client_name}\t#{client_flags}'
```

`#{pane_id}` on a client is the pane that client's screen is showing. A client
whose `#{client_flags}` contains `focused` has keyboard focus on the terminal
window itself, which is why `set -g focus-events on` is part of the snippet.
The rule (`tmux::pane_is_seen`) is:

> A pane is **seen** when a focused client is showing it. If **no client on the
> server carries the focused flag at all**, the flag carries no information —
> the terminal never reports focus — and any client showing the pane counts.

The fallback is what keeps the rule honest on terminals without focus
reporting; without it every pane would read unseen forever there. With focus
information present, a pane on an unfocused client's screen is *not* seen: it
is on screen while the user is in their browser.

Two places apply it:

- **`hook::run`, on a `Stop` only.** One `list-clients`; no other event pays
  for it. This is what decides `idle` vs `done` in the first place.
- **`store::mark_seen`, from `store::snapshot`** — so every read (the TUI
  refresh, `perch list`, `perch status`, `perch next`) flips any `Done` record
  whose pane is now seen to `Idle`, writes it through, and sets `@perch_state`
  for all of them in one `tmux` invocation. Nothing is asked of tmux unless at
  least one record is `done`.

That second one is the safety net that makes the tmux hooks **optional**. They
only make the transition immediate:

```
set-hook -g 'after-select-window[42]'    "run-shell -b 'perch seen'"
set-hook -g 'after-select-pane[42]'      "run-shell -b 'perch seen'"
set-hook -g 'client-session-changed[42]' "run-shell -b 'perch seen'"
set-hook -g 'window-pane-changed[42]'    "run-shell -b 'perch seen'"
set-hook -g 'session-window-changed[42]' "run-shell -b 'perch seen'"
set-hook -g 'client-focus-in[42]'        "run-shell -b 'perch seen'"
```

`perch seen` with no argument runs the reconciliation over every `done` record,
so it does not matter which hook fired or what pane it names.
`window-pane-changed` and `session-window-changed` are state-change hooks, so
unlike the `after-<command>` hooks they fire for `next-window`,
`previous-window`, `last-window`, `last-pane` and `switch-client` too — which
is the whole reason a pane you were looking at used to keep saying `done`.
tmux 3.6a has no `after-next-window`, `after-previous-window`,
`after-last-window`, `after-last-pane` or `after-switch-client` option; a wrong
name errors at source time and takes the rest of the snippet with it.

`perch seen <pane>` still marks one named pane seen unconditionally — `perch
next` and the TUI's jump use it for the pane they just moved you to, where the
move itself is the evidence.

### Compared with herdr

herdr derives Claude's state by scraping the bottom of the pane buffer (a
screen manifest). perch does not: hooks are the authority for `working`,
`done` and `needs_input`, because a harness event is deterministic and pane
text is not (Invariant 3, no scraping). What perch takes from herdr is the
*seen/unseen* rule, which is not something a harness can report at all —
expressed here through tmux client focus rather than through a scraper.

A `needs_input` that you answered where perch could not see it — in the pane
itself — clears on the next proof that the agent is running: a `PreToolUse`
hook, an `elicitation_complete` / `elicitation_response` notification, or your
next prompt. `PreToolUse` fires often, so the reducer returns immediately
unless the pane is actually `needs_input`; the cost is one short-lived process
per tool call.

Anything else — an unrecognised notification, `PreCompact` and friends — parses
to `None` and is a no-op. `session_id` and `cwd` are recorded whenever present;
`project` is the last path component of `cwd`. `since` moves only on an actual
state change, so age means "time in this state". `last_message` is
whitespace-collapsed and capped at 400 characters. An event that changes
nothing on a pane perch has never seen creates no record.

States rank `needs_input < done < working < starting < idle < ended`; that rank
then `since` ascending is the sort used by `list`, `next` and the TUI, so "the
first row" is always "the oldest thing waiting on you".

## Subagents

A harness that attributes an event to a subagent (Claude's `agent_id`, codex's
`SubagentStart`) has it folded into the parent record's `children`, never into
a record of its own: a subagent runs in the parent's process, so it has no pane
to jump to and no state of its own to sort by.

Four rules keep the list an attention list rather than a transcript:

1. **A parent `Stop` retires every child still `working`.** The parent's turn
   ending is proof its subagents ended, whatever `SubagentStop` the harness
   lost. The child is marked `done` and its message is left alone. A child in
   `needs_input` is not retired — it is genuinely blocked on you.
2. **A pane reaching `idle` clears its finished children.** All three paths
   there clear: a `Stop` on a pane you were watching (`reducer::apply_with`),
   the seen reconciliation (`store::mark_seen`), and `perch seen`, which writes
   the state directly — so `store::reconcile` re-applies the rule on every read
   and persists it when the child list changed. A `UserPromptSubmit` clears
   them too: a new turn owes nothing to the last one.
3. **At most twenty children per pane** (`reducer::MAX_CHILDREN`). Pushing past
   it drops the oldest finished child, and only when none is left, the oldest
   running one.
4. **The ten-minute TTL stays** as a backstop, applied on every write, for a
   pane that goes quiet without ever reaching `idle`.

Subagent events are never parent transitions: they do not move the pane's
state, write `@perch_state` or log a state change. The one sound a child can
ask for is `needs_input`, and only for Claude's `agent_needs_input`.

On the board, a pane's harness cell carries the badge — `claude ⚑1 ▶4 ✓48`,
zero parts omitted, blocked children in the `needs_input` colour, running ones
in the `working` colour, finished ones dim. Child rows are drawn only for
children that are `working` or `needs_input`; `Space` (or `Tab`) on a pane
unfolds its finished children as dim rows, most recent first. The expansion is
session state in `App::expanded`, keyed by pane id, and is not persisted.
`next_waiting` and `perch next` ignore children entirely — there is nowhere
separate to send you.

## State directory

`~/.local/state/perch/`, overridable with `PERCH_STATE_DIR`:

```
panes/%12.json        one record per pane, written tmp-then-rename
panes/.%12.<pid>.tmp  transient; the write in progress
events.jsonl          append-only log: ts, pane, harness, event, state, session_id
events.jsonl.1        the previous log, rotated at 5 MB
mute                  presence = globally muted
```

A record is `{pane, harness, session_id, cwd, project, branch, location, state,
since, last_message, title, pid, last_sound_ms, children}`. Unknown or missing fields
default, so an old record still loads; an unparseable one is skipped rather
than fatal.

Config lives separately in `~/.config/perch/config.toml`
(`PERCH_CONFIG_DIR`), next to the generated `perch.tmux.conf`.

## The cue: sound, and nothing else

**A sound is the only thing perch does to get your attention.** It costs no
screen, cannot eat a keystroke, and needs no cleanup. The toast popup, the
window-status flag, the status-line flash and the macOS `osascript` banner are
all gone, along with `[toast]` and `[notify]`.

On a transition into `done` or `needs_input` the hook plays the configured
sound through `afplay`, subject to a per-pane `cooldown_secs` gap and the
global mute file. It also writes `set-option -p @perch_state <state>` on the
pane — one spawned `tmux`, never waited on — which costs nothing and lets a
user put perch's state in a status line they wrote themselves. Subagent events
are not parent transitions and never cue.

Under `PERCH_NO_TMUX=1`, `PERCH_TMUX_LOG=<file>` records each invocation as one
line, which is how the cue is tested.

## Navigation contract

Navigation is the product. These rules are not defaults; they are the contract.

**One primitive.** Every client-moving command is exactly

```
tmux switch-client -c <client_name> -t <pane_id>
```

which moves that client's session, window and pane in one atomic call. There is
no `select-window`, no `select-pane`, no session name and no bare command
anywhere in perch. `Tmux::jump(client, pane)` is the only API that moves a
client.

**The client is always explicit.** A tmux command without `-c` picks a "current
client" by heuristic — tty match, else most recent activity — which is exactly
how a jump issued from a popup's pty, or with two clients attached, sends the
user to the wrong screen. So every command that can move a client takes
`--client`: `perch tui --client <name>`, `perch next --client <name>`, and
`perch open --client <name>`.

`display-popup` does **not** expand `#{…}` formats in its shell-command
(verified on tmux 3.6a); `run-shell` and key bindings **do**. A popup can
therefore only know its client if a launcher passes it in, which is what
`perch open` is for:

```
tmux display-popup -c <client> -E -w 85% -h 75% -- <perch> tui --client <client>
```

and the bindings are

```
bind g run-shell 'perch open --client "#{client_name}"'
bind N run-shell 'perch next --client "#{client_name}"'
```

If `--client` is missing, perch falls back to `tmux display -p
'#{client_name}'` and prints a one-line warning to stderr, so the guess is
visible in logs rather than silent.

**Jumps are synchronous and checked.** `Tmux::run_checked` waits for tmux and
reports its exit status; only the hook's cue uses the fire-and-forget
`Tmux::batch`. A client move must never be abandoned: the popup closes the
moment the TUI returns, and its pty takes any un-waited child with it. Before
jumping, the pane is confirmed present in `list-panes`. If it is gone, or tmux
returns non-zero, the dashboard shows an inline error line — `pane %N is gone` —
and refreshes **instead of exiting**. Only a successful jump closes the popup,
and then `perch seen <pane>` runs on the destination pane.

**Selection is keyed by identity, never by row index.** `App::selected` is an
`Option<Selection { pane, child }>`; the row index is derived at render time.
The board reorders constantly — a group jumps to the top the instant one of its
panes needs input — so a stored index would quietly come to mean a different
agent, which is the other half of "Enter sent me to the wrong pane". A refresh
never moves the selection to another pane; if the selected pane disappears the
cursor falls to the nearest row. `App::jump_target()` returns the pane id for
the selection, resolving a subagent row to its parent pane.

**Rows say where the pane is, not which pane it is.** A `%444` is perch's
internal key — for the selection, for the jump, for the file name — and a user
has never navigated by it; they navigate by the window names on the tmux top
rail. So the first column is a location: `<window_name>`, prefixed
`<session>/` only when the live panes span more than one session and suffixed
`.<pane_index>` only when that window is split. A pane tmux no longer knows
about falls back to `PaneRecord::location`, which the hook stores from the same
`display -p` round trip it already pays for on a `Stop` (and once, on its own,
for a pane it has not located yet); failing that, `—`. Grouped rows drop the
project column entirely — the `▸ <project> (<branch>)` header above them
already says it. The pane id is in `perch list --json` and, under
`PERCH_DEBUG=1`, dim beside the location.

**Keys never overload each other.** `tui::Nav` owns the cursor and view keys
and the single piece of state they need — a pending `g`. `gg` (a second `g`
within `GG_WINDOW`, 500 ms) goes to the first row, `G` to the last, `v` toggles
grouped ⇄ flat; any other key cancels a pending `g`, and a lone `g` does
nothing. It is split out of the event loop so the whole contract is testable
without a terminal, and it never writes prefs itself — it raises
`prefs_dirty` and the loop persists.

`perch next` picks the oldest `needs_input`, else the oldest `done`, by `since`;
it prints the pane id, jumps, and exits 1 with `nothing waiting` when there is
nothing to go to.

Under `PERCH_DEBUG=1` every jump logs its exact tmux argv to stderr.

## Codex hook trust

Codex runs a hook only if `~/.codex/config.toml` records it as trusted; without
that record the user gets an accept prompt in the TUI and perch stays silent
until they answer it. `perch install codex` therefore writes the same record
the prompt would have written, so setup remains one step.

The key is `[hooks.state."<abs path of hooks.json>:<event_label>:<group index>:<handler index>"]`
with `trusted_hash = "sha256:<hex>"`. The hash (`src/trust.rs`, mirroring
codex's `hook_hash` and `version_for_toml`) is sha256 over the compact,
recursively key-sorted JSON of one identity object per handler:

```json
{"event_name":"stop","hooks":[{"async":false,"command":"perch hook codex","timeout":10,"type":"command"}]}
```

`matcher` appears in the identity only when the group actually has the key — an
empty matcher is a real matcher and hashes differently from none. Event labels
are codex's snake_case names: `session_start`, `user_prompt_submit`, `stop`,
`permission_request`, `session_end`, `subagent_start`, `subagent_stop`,
`pre_tool_use`, `post_tool_use`, `pre_compact`, `post_compact`, `interrupt`.

The indices are read from `hooks.json` as it stands after the merge, so they
are only valid while the file keeps that order. **Re-run `perch setup` after
reordering or editing `~/.codex/hooks.json`**; it recomputes every key and
hash. `perch doctor` shows `trust: yes/no` for codex by recomputing them, and
`perch uninstall` removes exactly those keys. `config.toml` is edited with
`toml_edit`, so comments, ordering and formatting elsewhere in the file
survive; it is backed up first and replaced atomically, and created when
missing.

## Liveness

There is no process watching for exits, so liveness is computed on every read.
`store::snapshot` runs

```
tmux list-panes -a -F '#{pane_id} #{session_name} #{window_index} #{pane_index} #{window_panes} #{session_attached} #{pane_current_command} #{pane_pid} #{window_name}'
```

`#{window_name}` trails because it is the one field a user can put spaces in,
so `parse_pane_line` takes it as the rest of the line. `snapshot_with_live`
returns that pane list alongside the records; the TUI keeps it, because a row's
location is a fact about tmux now, not about the record.

It reconciles: a record whose pane id is not in that list becomes `ended` with
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
   @perch_state` on a pane and `switch-client -c <client> -t <pane>` when the
   user asks to jump.
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
| `PERCH_TMUX_LOG` | with `PERCH_NO_TMUX`, record each tmux invocation to a file |
| `PERCH_FAKE_VIEWERS` | with `PERCH_NO_TMUX`, the client list the seen rule reads: `%1:focused,%2` |
| `PERCH_FAKE_JUMP_FAIL=1` | with `PERCH_NO_TMUX`, every checked tmux call reports failure |
| `PERCH_DEBUG=1` | log each jump's exact tmux argv to stderr |
| `PERCH_HOME` | home directory detection and every default path resolve against |
| `PERCH_NO_PATH_PROBE=1` | never probe `PATH` when detecting a harness |
| `PERCH_CLAUDE_SETTINGS` | target file for `perch install claude` |
| `PERCH_CODEX_HOOKS` | target file for `perch install codex` |
| `PERCH_CODEX_CONFIG` | codex config holding the hook trust records |
| `PERCH_DUMP_HOOK_INPUT` | directory to copy every raw hook payload into |
| `PERCH_PI_EXT_DIR` | directory for `perch install pi` |
| `PERCH_TMUX_CONF` | target file for the `source-file` line |

CI runs the suite with `PERCH_NO_SOUND=1 PERCH_NO_TMUX=1`; tests that touch the
store point `PERCH_STATE_DIR` at a `tempfile` directory. TUI rendering is
asserted offscreen with ratatui's `TestBackend`.
