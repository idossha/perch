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
hooks in indexed slots.

**Superseded in part by entry 13**, which replaces the `111` probe and makes
seen a property of the live client list instead of a hook side-effect.

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

## 11. One jump primitive, with an explicit client — 2026-09-05

**Decision.** Every command that moves a client issues exactly one
`tmux switch-client -c <client_name> -t <pane_id>`, and the client is always
named. `perch tui`, `perch next` and the new `perch open` all take `--client`;
the tmux bindings pass `#{client_name}` through `run-shell`, and `perch open`
is what launches the popup, because `display-popup` does not expand `#{…}` in
its shell-command (tmux 3.6a) while bindings and `run-shell` do. `select-window`
and `select-pane` are gone, as is `Tmux::focus`. Jumps are synchronous and
checked; the pane is confirmed in `list-panes` first, and a failure shows
`pane %N is gone` in the dashboard instead of closing it. The TUI's cursor is
keyed by `Selection { pane, child }` rather than by row index.

**Why.** Navigation from the dashboard was unreliable: sometimes the wrong
pane, sometimes nothing. Two causes. A tmux command without `-c` picks a
"current client" by heuristic — tty match, else most recent activity — so a
jump issued from a popup's pty, or with a second client attached, moved
whichever client tmux felt like. And the cursor was a row index into a list
that reorders itself every second, so between drawing a row and pressing Enter
the index could come to mean a different agent. Naming the client removes the
guess; keying the selection removes the race. One atomic call also removes the
window where a three-command sequence half-applied.

**Cost.** The bindings are longer and `perch open` is an extra process between
the key and the popup. A user who runs `perch tui` by hand must pass
`--client`, or accept a warned guess. Anyone with the old snippet in their
tmux.conf needs `perch setup` again.

**Revisit if** tmux ever expands formats in `display-popup`'s shell-command,
which would let the popup name its own client.

## 12. Sound is the only cue — 2026-09-05

**Decision.** The toast popup is deleted, with `perch toast`, `perch
toast-body`, `src/toast.rs`, the `[toast]` and `[notify]` config tables and the
`unicode-width` dependency. A transition plays a sound and writes
`@perch_state` on the pane; nothing is drawn on the user's screen.

**Why.** Every visual cue perch tried borrowed something that belongs to the
user — the window list, the status line, or the keyboard. The toast was the
least invasive of them and still needed keystroke passthrough, a fade, a
per-client process and a tmux 3.2 floor to be merely tolerable. The sound
already carried the whole message, and the dashboard is one keystroke away for
the detail. Deleting the toast removes about 350 lines and a whole class of
"it ate my keypress" failure.

**Cost.** A user with sound off or muted now gets no interrupt at all; they
learn about a finished agent from the status line or the dashboard. `perch
toast test` is gone, as is the macOS notification banner.

**Revisit if** users on muted machines ask for a visual cue — which should then
be opt-in, and should not take the keyboard.

## 13. Seen is evaluated from live clients on every read; hooks only make it immediate — 2026-09-05

**Decision.** Whether a pane has been seen is a question asked of tmux, not a
flag a hook sets. One `list-clients -F '#{pane_id}\t#{client_name}\t#{client_flags}'`
is the whole input, and `tmux::pane_is_seen` is the whole rule: a pane is seen
when a *focused* client is showing it, or — when no client on the server
carries the `focused` flag at all, so the flag carries no information — when
any client is showing it. `hook::run` applies it once on a `Stop` (replacing
the `#{pane_active}#{window_active}#{session_attached}` == `111` probe) and
`store::snapshot` applies it on every read, flipping any `done` record whose
pane is now seen to `idle` and writing it through. The tmux hooks stay, now
pointed at an argument-less `perch seen`, and are demoted to an optimisation.

**Why.** The old design could only learn about a switch it was told about, and
tmux told it about three: `after-select-window`, `after-select-pane` and
`client-session-changed`. Every other way a user changes what is on screen —
`prefix n`, `prefix p`, `prefix l`, `last-pane`, a sessionx picker, clicking a
window — fires none of them, so panes the user had been staring at for minutes
kept claiming `done`. That is a direct attack on the one thing the board is
for. Making it a derived property means there is no event to miss: correctness
no longer depends on a hook installation the user may not have, on a tmux
version, or on a hook name existing. Adding focus to the rule fixes the other
half — the `111` probe called a pane seen while the user was in a browser.

The rule is herdr's, and only the rule: herdr's state authority for Claude is
screen-manifest scraping of the bottom of the pane buffer, which perch will not
do (Invariant 3). Hooks remain the authority for `working` / `done` /
`needs_input` because harness events are deterministic and pane text is not.
Seen is the one thing no harness can report, so it is the one thing worth
deriving from tmux.

**Cost.** Every read that finds at least one `done` record pays one extra
`list-clients` (a board with nothing finished pays nothing). A pane can now
change state without any event, which means `since` moves on a read — age
means "time in this state", which is still true, but the mover is a reader.
`perch seen`'s signature changed, and `PERCH_FAKE_CLIENTS` /
`PERCH_FAKE_PANE_FOCUSED` are replaced by `PERCH_FAKE_VIEWERS`.

tmux 3.6a has no `after-next-window`, `after-previous-window`,
`after-last-window`, `after-last-pane` or `after-switch-client` — all five were
tried and all five error, which would break the whole snippet at source time.
The state-change hooks `window-pane-changed` and `session-window-changed` cover
those commands instead, and `client-focus-in` covers returning to the terminal.

**Revisit if** `list-clients` shows up in a profile of the TUI refresh, at
which point the reconciliation should be rate-limited rather than removed.

## 14. Finished subagents fold into the badge; a parent stop retires its children — 2026-09-05

> **Superseded in part by decision 16.** The retire-on-`Stop` half of this entry
> was wrong: Claude runs subagents in the background, so a parent `Stop` is not
> proof its subagents ended. The folding, the `idle` clearing, the cap and the
> TTL all stand.

**Decision.** A pane's finished subagents are a count in its harness cell
(`claude ▶4 ✓48`), not rows. Only children that are `working` or `needs_input`
get a row; `Space` or `Tab` unfolds the rest, dim and newest first, as session
state keyed by pane id. Four reducer/store rules keep the list short: a parent
`Stop` marks every still-`working` child `done`, a pane reaching `idle` by any
path drops its finished children, a pane keeps at most twenty of them, and the
ten-minute TTL stays as a backstop.

**Why.** A real board had forty-eight finished subagents listed under one pane,
pushing the four that were still running — and the pane above that was waiting
on a permission prompt — off the screen. Those four had been claiming `working`
for twenty-five minutes because no `SubagentStop` ever arrived for them. Both
halves are the same defect: perch was recording what happened instead of
showing what needs you. Retiring on the parent's `Stop` makes correctness
independent of an event the harness may never send, which is the same argument
as decision 13 for `seen`. Clearing on `idle` puts the rule where the state
already means "nothing outstanding here", so `perch seen`, the reconciliation
and a watched `Stop` cannot disagree.

**Cost.** The record no longer keeps a full history of a fan-out: past twenty
children, or once the pane goes idle, earlier ones are gone from
`perch list --json` as well as from the board. `events.jsonl` still has every
`subagent_start` / `subagent_stop`, which is where a transcript belongs.
`store::snapshot_with_live` now writes a record back when only its child list
changed, so a read can touch the disk where it previously would not. The
harness column became adaptive to fit the badge, so its width now depends on
the widest fan-out on screen.

**Revisit if** users want the finished list to survive a pane going idle — in
which case it should be a query over `events.jsonl`, not a longer record.

## 15. The card takes the screen, and gives every keystroke back — 2026-09-05

**Decision.** A sound is unmissable and uninformative: it does not say which of
six agents finished. On `done` or `needs_input` perch also fades a three-line
card into the center of every attached client: project and branch, tmux
location and harness, last message. Rejected the same day: a desktop banner
(per-OS, unstyleable, outside tmux) and a one-line bottom-right toast (not
enough to identify an agent).

**Why it is allowed to take the screen.** It cannot cost the user anything. The
popup body reads the client's active pane before entering raw mode and forwards
the first keystroke verbatim with `send-keys -l --` as it closes, so a card
drawn over someone mid-sentence loses no character. It costs the hook no time:
the hook spawns a detached `perch notify` and returns.

**Cost.** The fade is the popup restyling itself from inside
(`display-popup -s` run within the popup), which needs tmux 3.2+; below that
perch degrades to sound only. Two cards in a row replace each other rather than
stack. `[notify] enabled = false` turns it off.

## 16. A pane with running subagents is delegating, not idle — 2026-09-05

**Decision.** A pane whose own state is `done` or `idle` while any subagent is
`working` or `starting` has an `effective_state` of `working`, shown as
**delegating**. Everything user-facing reads it: the TUI state cell and its
spinner, the sort rank, `perch next` (a delegating pane is never a jump
target), `perch status` counts, the `@perch_state` pane option, and
`perch list --json`, which reports `effective_state` next to `state`. A parent
`Stop` with running children asks for no chime and no card, and the last
child's `SubagentStop` is silent too. This **supersedes the retire-on-`Stop`
rule of decision 14** and the read-time invariant "a pane that is not working
has no running subagents"; running children are now retired only by the
parent's `SessionEnd`, by the pane being `ended`, and by a two-hour age cap
(`reducer::CHILD_MAX_RUNNING_SECS`).

**Why.** The assumption behind 14 — a subagent runs inside its parent's turn —
is not how Claude Code works. Subagents run in the background: the main agent's
turn ends and `Stop` fires while they keep working, and it is woken by a task
notification when each finishes. Retiring on `Stop` therefore erased live work,
and the chime it fired sent the user to a pane that was still busy — the exact
failure the states exist to prevent. Splitting the pane's own state from the
state it is shown as keeps the reducer honest about the events it saw while the
board stays honest about what needs a human.

**Cost.** A record now carries two states a reader must not confuse: `state` is
the pane's own and is what the reducer and the seen rule move; `effective_state`
is what the user is shown. A pane can sit `done`-but-delegating indefinitely if
a `SubagentStop` is lost, which is what the two-hour cap bounds — coarse on
purpose, since a legitimate subagent can run for a long time. The
delegating-to-done transition writes `@perch_state` from a subagent event,
which previously never touched tmux.

**Revisit if** a harness starts reporting subagents that genuinely cannot
outlive the parent turn, or if the two-hour cap proves either too slow to
repair a lost `SubagentStop` or too quick for a real long-running fan-out.

## 17. Resumed agents are tracked from `SendMessage`, helper stops are ignored — 2026-09-05

**Decision.** A Claude `PreToolUse` for the `SendMessage` tool with a non-empty
`tool_input.to` becomes `Event::SubagentResume { id }`: the named child is set
`working` with `since` = now, keeping its `agent_type` and last message, and is
created if perch has never seen it. It is not a parent transition and asks for
no sound; it makes the pane **delegating** after its next `Stop`, until the
matching `SubagentStop`. Conversely a `SubagentStop` for an id perch does not
know is recorded as a finished child **only if** `agent_type` is non-empty; an
unknown id with an empty `agent_type` is logged to `events.jsonl` and dropped.
Every other `PreToolUse` is still `ToolUse`, and the caps, the TTL and the
retire rules of decision 16 are unchanged.

**Why.** Captured payloads show `SubagentStart` fires only for a *fresh* spawn:
resuming a background agent through `SendMessage` fires nothing, so the pane
went `done` and stayed `done` while a real agent worked — decision 16's
delegating state, blind to half the agents it was written for. The tool's
target is the one place the id appears before the run. The mirror problem is
Claude's internal helper agents, which finish constantly with an empty
`agent_type` and no start — 310 unpaired stops in one day's capture; treating
those as subagents filled panes with rows for work the user never asked for.
`agent_type` is the only field that separates the two, so it is what the rule
turns on.

**Cost.** A `SendMessage` to a teammate or a session, whose `to` is a name
rather than an agent id, creates a child row named after them; it retires like
any other on the two-hour cap. A real spawned agent whose `SubagentStart` was
lost *and* whose stop carries an empty `agent_type` is now invisible rather
than appearing as a spurious finished child. `PreToolUse` parsing does one
extra string compare per tool call.

**Revisit if** Claude starts firing `SubagentStart` on resume, gives helper
agents a distinguishable `agent_type`, or exposes the resumed agent's id
anywhere earlier than the tool call.

## 18. Claude's questions are answered from a popup that *is* the dialog — 2026-09-06

**Decision.** Two Claude hook groups matched to `AskUserQuestion` — on
`PreToolUse` and on `PermissionRequest` — run `perch hook claude --ask`. That
hook records the question as `needs_input`, opens the answer form as a
`display-popup` on every attached client (the asking pane's own client
included), then blocks until a form writes `answers/<pane>.json`, and prints
`allow` with `updatedInput.answers` — which Claude takes as the user's answer.
**perch holds a question exactly as long as its popup is on screen.** Closing
the popup — `Esc` in place, or `p` to close and jump to the pane — hands the
question to Claude at once and its own dialog appears; so does the 59-minute
deadline, and so does a question kind the form has no control for. The board
shows such a pane as `⚑ question` and `Enter` jumps to it; it offers no
answer key of its own. The plain hook ignores the payload. Claude only;
`[ask] enabled = false` turns it off.

**Why.** The request was to answer an agent's questions from perch and pass the
answers back. For permission prompts and for cloud sessions there is no
channel: the `permission_prompt` and `elicitation_dialog` notifications carry
no text and no way to reply, and cloud sessions expose no local API. Claude's
`PreToolUse` for `AskUserQuestion` is the one place both halves exist — the
full question in `tool_input`, and a documented return path (`updatedInput`
with `answers`, "enabling headless integrations that collect answers via their
own UI"). Building on anything else would have meant scraping or a relay
server, both rejected in decisions 1 and 3.

`PermissionRequest` is not belt-and-braces: it is the only event auto mode
fires for the question tool. The first live test in an auto-mode session showed
Claude's own dialog and no hook run at all, while the same test in default mode
went through `PreToolUse`; the transcripts differ in exactly that one hook
record. Since the two events carry the same input and accept the same answer,
one hook serves both, and a defer on the first is remembered so the second does
not re-take a question the user chose to answer in the pane.

The popup pops up where the user is, rather than waiting behind a key on the
board, because the point of taking the question is to spare the walk to the
pane. It is the card's own mechanism with the form as its body, so it costs no
new tmux surface, and it can be answered on one client and vanish from the rest
because every body polls the one record.

"The popup is the dialog" replaced two earlier rules, each tried in use and
each confusing in the same way. The first handed a question back whenever a
focused client was showing the pane, so a glance at the pane took the board's
`a` away for good. The second kept perch's hold after the popup was put away
and offered `a` on the board to come back to it — and then a pane could show
Claude "thinking" for as long as the user liked, with nothing on screen asking
anything, while the answer lived behind a key on another screen. Both split
the question's ownership across two surfaces and left the user to know which
one had it. One rule ends that: whatever is on screen asking is the thing that
has it — perch's popup while it is up, Claude's dialog the moment it is not.
There is no held-but-hidden state, so there is nothing for `a` to do, and the
board's job is what it always was: say what needs you, and take you there.

Several agents asking at once are the normal case on a board, and two popups
fighting for one client (tmux replaces the first) would lose an answer in
progress. So a client holds one popup at a time, and the popup is a queue:
it shows the next waiting set when the current one is done, oldest first,
and says how many are behind it. `p` ends the popup because a form over the
pane you just asked for would defeat it, and no popup opens on a client that is
sitting on some other agent's dialog. The popup is sized from its content —
wrapped question and descriptions — and capped by the client, because a form
that cuts an option's description is a form that gets the wrong answer.

**Cost.** One hook process per question stays alive while the human decides,
which is the first time a perch process outlives its event; it is Claude's own
hook, bounded by Claude's timeout, and holds no socket. The installed settings
gain two groups with a one-hour timeout, and an install from 0.3 needs a `perch
setup` re-run to get them (`doctor` reports wired either way, since the marker
is `perch hook`). A `number` question is never taken. There is no "answer it
later from the board": once Claude's dialog owns a question, only that dialog
can take it, which is the price of having exactly one owner at a time.

Codex and pi reach the same popup by a different road, because neither lets a
hook answer its dialog: pi has no question tool at all, and Codex's
`request_user_input` takes no answer in its input and exists only in Plan
mode. Both harnesses let a model call tools perch provides — pi through its
extension API, Codex through MCP — so perch provides the tool: `ask_user`, the
same schema as Claude's, whose body is `perch hook <harness> --ask`. That
keeps one hook, one popup and one answer file for all three, and it means
Codex is never pushed into Plan mode for a question; the tool is there in
every mode. A harness's own dialog remains the fallback: pi's select dialog
inside the tool, and for Codex a tool result that tells the model to ask in
chat.

**Revisit if** Claude gives hooks a return path for permission prompts (then
approve-from-the-popup is the same shape), exposes questions from cloud
sessions locally, or codex grows an answerable question tool.

## 19. The model column is what the harness reported, never what the config says — 2026-09-07

**Decision.** A pane's `model` and `effort` come only from hook payloads —
Claude's `SessionStart.model` and `PostModelSwitch.to_model` plus `effort.level`
on turn hooks, Codex's `model` on every payload, pi's extension's `model` and
`effort` — and the board's `model` column exists only once some pane has
reported one. A harness that has said nothing shows nothing. Labels are short
(`model::model_label`): provider prefix and a leading `claude` dropped, version
numbers dotted, dates and bracketed variants such as `[1m]` dropped.

**Why.** The user wanted to see which model and effort each agent runs. Every
harness makes the live value available in the events perch already reads, so
the answer is authoritative; a configured default read from
`~/.codex/config.toml` or `settings.json` would be wrong the moment `/model` is
used and perch would have no way to know. Better an empty cell than a
plausible lie — the same rule as decision 3's "hooks, not scraping". The
context-window variant is dropped from the label because it is a size, not a
model, and the column is for telling agents apart at a glance.

**Cost.** A Claude pane shows its effort only after its first turn, and its
model only when Claude includes it on `SessionStart` (it does not after
`/clear` or a recovery) or on a switch; one more hook group
(`PostModelSwitch`) is installed. Codex exposes no effort. The board gains a
column whose width follows the longest label.

**Revisit if** a harness starts omitting the model from every payload, or the
column crowds the message on narrow terminals.
