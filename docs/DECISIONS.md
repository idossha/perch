# perch — decision log

Newest last. Each entry records what was decided, what it rules out, and what
would justify revisiting it.

## 1. Daemonless file store instead of a socket server — 2026-09-05

**Decision.** State is one JSON file per pane under
`~/.local/state/perch/panes/`, written atomically by the hook process. No
resident process, no socket, no IPC.

**Why.** The write rate is a handful of events per pane per minute and the read
rate is one dashboard refresh per second — far below anything needing a server.
A daemon adds a lifecycle to manage (start, crash, restart, stale socket,
version skew between client and server) for no capability we need. Files also
mean the state is inspectable with `cat` and survives perch being upgraded or
uninstalled mid-session. herdr already occupies the "server with an API" niche;
duplicating it would have meant migrating into it.

**Cost.** No push updates: readers poll (1 s in the TUI) and liveness is
recomputed per read. Cross-machine aggregation is out of scope.

**Revisit if** we need remote panes, sub-second fan-out to many watchers, or
per-event history queries that a `jsonl` scan cannot serve.

## 2. Pane id as identity instead of PID mapping — 2026-09-05

**Decision.** A tracked agent is identified by its tmux pane id (`%NN`) taken
from `$TMUX_PANE`. No PID tables, no session-file lookups, no process-tree
walking.

**Why.** `$TMUX_PANE` is exported by tmux and inherited by every child, so a
hook process gets it for free and correctly, with no ambiguity about which of
several agents fired. It is also exactly the handle needed to act — jumping is
`select-pane -t %NN`. PID-based identity (recon's `sessions/{pid}.json`) breaks
whenever the harness re-execs or the session is cleared, and needs a liveness
probe of its own.

**Cost.** Nothing is tracked outside tmux: no `$TMUX_PANE` means the hook exits
without writing. Two agents sharing one pane are one record.

**Revisit if** we need to support agents outside tmux, at which point identity
becomes "pane id, else session id".

## 3. Hooks instead of status-bar scraping — 2026-09-05

**Decision.** All state comes from harness lifecycle events delivered to
`perch hook`. Pane content and status-bar text are never read.

**Why.** Scraping is a guess about someone else's rendering: it breaks on a
theme change, a `/clear`, a resize, a spinner frame, or a wording change in the
next release, and it costs a `capture-pane` per pane per tick. Hooks give the
harness's own account of what happened, including the distinction between "done"
and "waiting for permission" that no rendering exposes reliably. The same
neutral event schema then serves Codex and pi through their own adapters.

**Cost.** An agent whose harness has no hooks, or whose hooks are not
installed, is invisible. That is accepted for v1 — `watch_commands` is reserved
for showing such panes as `unknown` later, still without reading their text.

**Revisit if** a harness we must support ships no usable hook surface.

## 4. Sound played in the hook process, not the TUI — 2026-09-05

**Decision.** `perch hook` spawns `afplay` (and the optional `osascript`
notification) at the moment the state changes. The TUI plays nothing; it only
toggles the mute file.

**Why.** The alert is the product: you should hear that an agent needs you
whether or not a dashboard is open. Sounding from the TUI would make alerts
conditional on having a popup up, and would require the TUI to diff state to
detect transitions — inventing edge detection over polled files when the hook
already knows precisely that a transition occurred. The cooldown stamp
(`last_sound_ms`) lives in the same record as the transition, so it is written
in the same atomic save.

**Cost.** The sound decision runs in a latency-sensitive path; it is bounded by
a detached spawn whose failures are swallowed, and by a 3 s per-pane cooldown.
Mute is a file so that a TUI toggle is visible to every future hook process.

**Revisit if** we want per-window or focus-aware muting, which needs the
sounding process to know what the user is looking at.

## 5. Merge-not-replace installers, with backups — 2026-09-05

**Decision.** `perch install claude` appends its hook group to each event array
in `~/.claude/settings.json`, preserving everything already there, skipping
events where a `perch hook` command is already present, and copying the file to
`<name>.bak-<timestamp>` before writing. `perch install tmux` writes its own
`~/.config/perch/perch.tmux.conf` and, at most, appends one `source-file` line
to `~/.tmux.conf` — and only with `--apply`.

**Why.** These files are the user's, and they already contain other integrations
(idosleep, herdr). An installer that rewrites them is an installer nobody runs
twice. Idempotence by marker makes re-running safe after an upgrade; the backup
makes a bad merge recoverable without version control; `--dry-run` and `--print`
let the merge be inspected before it lands.

**Cost.** The merge is append-only, so it cannot clean up a stale perch entry
whose command string changed — that needs a manual edit or a future
`perch uninstall`. Ordering within an event array is not controlled.

**Revisit if** the hook command string changes, which would need a migration
step that rewrites rather than appends.
