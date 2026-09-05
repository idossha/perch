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

## 6. `perch setup` detects and wires; the formula only tells you to run it — 2026-09-05

**Decision.** One command, `perch setup`, detects which harnesses exist on the
machine (config directory under `$HOME`, or the binary on `PATH`), runs each
installer, wires tmux and reloads it, and writes
`~/.config/perch/setup.json`. Homebrew cannot write to `$HOME`, so the formula
does not attempt it: it prints a caveat asking for one `perch setup`.
`install.sh` — which the user runs as themselves — does run setup, and
`cargo install perch && perch setup` says it in the README. Detection takes a
`Paths` struct resolved from `PERCH_HOME`, so the tests run against a temp home.

**Why.** "Install the binary" and "modify the user's dotfiles" are different
privileges and different moments. A package manager that edits `$HOME` breaks
`brew uninstall`, surprises anyone installing for a shared machine, and cannot
be undone by the manager. Making setup a single explicit command keeps the
dotfile edit visible, idempotent and reversible, and gives every install path
the same second step. A first-run nudge (a TUI banner, a stderr hint from
`list`/`status`) closes the gap for the user who forgets.

**Cost.** Brew users are one manual step from a working perch. The nudge costs
a detection pass on every `list` and `status` until the marker exists.

**Revisit if** Homebrew grows a sanctioned post-install user hook, or a harness
gains a config location that neither `$HOME` nor `PATH` reveals.

## 7. `uninstall` reverses only perch-owned entries — 2026-09-05

**Decision.** `perch uninstall` removes hook groups whose command contains
`perch hook` and nothing else. An event array left empty is deleted only when
`setup.json` records that perch created it; the file is backed up before the
rewrite; the pi extension, `perch.tmux.conf` and the `source-file` line — all
entirely ours — are deleted outright.

**Why.** The same reasoning as the merge-not-replace installers (decision 5),
run backwards: these files belong to the user and hold other integrations. The
marker is what makes the difference between "this array existed before perch"
and "perch made it", which no amount of reading the current file can tell.
Without a marker, uninstall falls back to removing arrays that end up empty,
which is right for the common case and never touches a non-empty one.

**Cost.** Uninstalling after deleting `setup.json` by hand may leave an empty
array behind, or remove one the user had emptied themselves. Backups
accumulate; they are timestamped, never overwritten, and never cleaned up.

**Revisit if** the marker becomes unreliable — at which point uninstall should
refuse to remove keys rather than guess.

## 8. perch writes codex's hook trust records at install time — 2026-09-05

**Decision.** `perch install codex` (and `perch setup`) computes codex's own
hook hash for each perch handler and writes
`[hooks.state."<hooks.json>:<label>:<group>:<handler>"] trusted_hash` into
`~/.codex/config.toml`, exactly as accepting the prompt in the codex TUI would.
`uninstall` removes those keys; `doctor` recomputes them and reports
`trust: yes/no`.

**Why.** Codex silently refuses to run an untrusted hook. Without the record,
`perch setup` reports success, `doctor` reports "hooked", and nothing ever
appears in the dashboard until the user happens to open codex, notice a prompt
and accept it. Writing the record is the only way for one command to leave a
working install, and it is a record the user is being asked for anyway.

**Cost.** perch reproduces an undocumented hash recipe from codex's source; if
codex changes the canonical form, the records go stale and codex re-prompts —
loud, not silent. The keys carry file indices, so editing `hooks.json` requires
another `perch setup`. Two unit tests pin the recipe against hashes taken from
a real accepted config.

**Revisit if** codex publishes a command to trust a hook non-interactively, or
changes the fingerprint, at which point perch should call it instead.

## 9. herdr's state definitions, verbatim — 2026-09-05

**Decision.** `needs_input` means a real approval or question dialog and
nothing else; `done` means a turn finished while the pane was not being looked
at; `idle` means ready and seen. Claude's `idle_prompt` (and `auth_success`,
`quota_*`) is recorded and ignored. A `Stop` on the focused pane goes to `idle`
silently. `perch seen <pane>`, run from the tmux `after-select-window`, `after-select-pane` and `client-session-changed` hooks, from `perch next` and
from the TUI, performs the transition back to `idle`; a `PreToolUse` clears a
`needs_input` nobody told perch about.

**Why.** Mapping `idle_prompt` to `needs_input` made every pane you had not
touched for a minute claim to be waiting on you, which is exactly the signal
the dashboard exists to protect. herdr already had definitions that survive
contact with real sessions; adopting them wholesale is cheaper than inventing
a second vocabulary, and makes `done` mean something a glance can trust.

**Cost.** perch now subscribes to `PreToolUse`, so a hook process runs on every
tool call; the reducer returns immediately unless the pane is `needs_input`.
Deciding a `Stop` costs one blocking `tmux display -p`. Seen-tracking rides on
tmux's `after-select-window`, `after-select-pane` and `client-session-changed`
hooks in indexed slots; `pane-focus-in` registers but never fires on tmux 3.6,
so a pane you switch to by other means reads `done` until a reader or `perch
next` looks at it.

**Revisit if** the `PreToolUse` cost shows up in practice, or a harness starts
reporting "the dialog is gone" directly.

## 10. A toast popup instead of a window flag — 2026-09-05

**Decision.** A parent transition draws a borderless one-line
`display-popup` in the bottom-right corner of every attached client (`-B -E -x
R -y P -h 1`), styled green `✓` or red `⚑`, which fades through two dimmer
styles over its last 600 ms and vanishes. The first keystroke dismisses it and
is forwarded verbatim to the client's active pane with `send-keys -l --`. The
`@perch_flag` window-status marker, the two guarded `window-status-format`
appends and the per-client `display-message` flash are gone, along with the
`[notify] tmux_message` and `duration_ms` keys; `[toast] enabled /
duration_ms / done_style / needs_input_style` replaces them.

**Why.** The flag wrote perch's state into the user's window list and stayed
there, in a line the user had themselves designed; the flash borrowed the
status line, which is also theirs. A toast borrows a corner for three seconds
and gives it back. Keystroke passthrough is what makes it safe to draw over
someone who is typing: without it the toast is a trap, because a popup takes
the keyboard and the character is lost.

**Cost.** `display-popup` needs tmux 3.2, and the toast is one process per
attached client (plus the `perch toast` launcher) rather than one for all of
them — off the hook's critical path, but not free. A toast can be replaced by
the next one before it is read, and a user with no client attached sees
nothing, where the flag persisted.

**Revisit if** `display-popup` proves too heavy on many clients, or tmux gains
a real non-focus-stealing notification.
