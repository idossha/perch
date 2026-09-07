# perch

A thin, daemonless, tmux-native home base for coding agents.

perch watches the agents you already run — it never owns a session, a pane or a
worktree. Each harness calls `perch hook` from its own lifecycle hooks; perch
writes a small JSON record per tmux pane, plays a sound when a pane needs you,
and shows everything in a popup dashboard. There is no server, no polling
daemon, and no scraping of pane text.

Claude Code works today. Codex and pi share the same event schema and land in
phase 3.

## Install

Three paths, each ending in one `perch setup` — the single command that
detects which harnesses you actually have and wires all of them.

**Homebrew**

```sh
brew install idossha/perch/perch
perch setup
```

**Cargo**

```sh
cargo install perch && perch setup
```

Cargo cannot run anything after an install, so setup is a separate word on the
same line. `cargo binstall perch` fetches the same release tarball.

**Script** (downloads the latest release into `~/.local/bin` and runs setup for
you)

```sh
curl -fsSL https://raw.githubusercontent.com/idossha/perch/main/install.sh | sh
```

`PERCH_VERSION=v0.2.0` pins a release, `PERCH_INSTALL_DIR` moves the target
directory, and `--no-setup` installs the binary alone.

macOS is the primary target: sounds use `afplay` and `/System/Library/Sounds`,
and optional desktop notifications use `osascript`. Everything else works
anywhere tmux does.

## Setup

```sh
perch setup [--dry-run] [--yes] [--no-tmux] [--only claude,codex,pi,tmux]
```

`setup` detects each harness (its config directory under `$HOME`, or its
binary on `PATH`), runs that harness's installer, writes
`~/.config/perch/perch.tmux.conf` with the `prefix + g` popup binding, the
`prefix + N` binding for `perch next` and the seen-tracking hooks, and adds
one `source-file` line to `~/.tmux.conf`, and reloads tmux when you are inside
it. Every merge is backup-first and append-only; a harness that is not
installed is skipped with a note, and a second run reports `already`:

```
component  status              file                          backup
claude     installed           ~/.claude/settings.json        ~/.claude/settings.json.bak-20260905...
codex      installed           ~/.codex/hooks.json            ~/.codex/hooks.json.bak-20260905...
pi         skipped: not found  -                              -
tmux       installed           ~/.tmux.conf                   ~/.tmux.conf.bak-20260905...
```

For codex, setup also records the hooks as trusted in `~/.codex/config.toml`
(`[hooks.state."<hooks.json>:<event>:<group>:<handler>"] trusted_hash`) — the
same thing you would get by accepting codex's prompt, without which codex
silently skips the hook. The keys carry the handler's position in
`hooks.json`, so **re-run `perch setup` after editing or reordering that
file**; `perch doctor` shows `trust: yes/no`, and `perch uninstall` removes
only perch's own keys. The rest of `config.toml` keeps its formatting.

A successful run leaves `~/.config/perch/setup.json` recording the version, the
timestamp and what it touched — which is also what tells `uninstall` which hook
arrays were perch's to remove. Until that file exists, the TUI shows a banner
(`S` runs setup) and `list`/`status` print a one-line hint on stderr.

**Check it.** `perch doctor` reports the binary, each harness (found? hooked?),
the tmux binding and `source-file` line, the sound player, the state directory
and the mute flag. It exits 1 when a harness you have is not wired, and
`--json` prints the same as one object.

**Remove it.** `perch uninstall [--dry-run] [--keep-state]` reverses every
installer: it drops only the hook entries whose command contains `perch hook`
(backing the file up first, leaving every other entry untouched), deletes the
pi extension, removes the `source-file` line and `perch.tmux.conf`, and deletes
the state directory unless you keep it.

Individual installers remain available for one harness at a time:

```sh
perch install claude --dry-run   # show what would change
perch install claude
perch install tmux --apply
```

## Commands

```
perch hook <claude|codex|pi>          read a hook payload on stdin, update this pane
perch hook claude --ask               the AskUserQuestion variant: pop up the form, wait for its answer
perch mcp                             the ask_user tool for Codex, over MCP on stdio (registered by setup)
perch list [--json]                   snapshot of every tracked pane
perch status [--format plain|tmux]    one-line summary for the status bar
perch next --client <name>            jump a client to the oldest waiting pane
perch open --client <name>            open the dashboard in a popup on a client
perch tui --client <name>             the dashboard itself
perch sound test <event>              play the sound for done | needs_input | error
perch notify test                     draw a sample notification card
perch seen [<pane>]                   mark seen panes idle (all, or one named)
perch install <claude|codex|pi|tmux> [--dry-run] [--print] [--apply]
perch setup [--dry-run] [--yes] [--no-tmux] [--only ...]
perch doctor [--json]
perch uninstall [--dry-run] [--keep-state]
```

When an agent finishes or needs you, perch plays a sound **and** fades a small
card into the middle of every attached client: project, place, and what the
agent last said. Type anything and it vanishes; the character you typed lands
in the pane underneath. `perch notify test` shows one. See
[docs/NOTIFICATIONS.md](docs/NOTIFICATIONS.md).

- `hook` reads one JSON object on stdin and always exits 0, so a broken perch
  can never fail your agent. Outside tmux it does nothing.
- `list` prints pane, project (branch), harness, state, age and last message;
  `--json` prints the full records.
- `status --format plain` prints e.g. `⚑2 ▶1 ✓1` (waiting, working, done);
  `--format tmux` adds tmux colour escapes for `status-right`.
- `next` prints the pane id of the oldest `needs_input` pane (else the oldest
  `done`) and moves the named client to it; it exits 1 with `nothing waiting`
  when there is nothing to go to.
- `open` draws the dashboard as a popup on one client and passes that client's
  name to `perch tui`, which is the only way the popup can know which client to
  move. `prefix + g` runs it for you.
- `install --dry-run` reports without writing; `--print` writes nothing and
  dumps the merged settings JSON (claude) or the config snippet (tmux) to
  stdout; `--apply` is tmux-only and appends the `source-file` line.

## TUI keys

Run it from the popup binding (`prefix + g`) or directly with
`perch open --client "$(tmux display -p '#{client_name}')"`.
Each row's first column is where the pane is in tmux — the window name, with
`session/` in front when more than one session is on the board and `.N` after
it when the window is split — not its `%id`, which perch uses internally and
shows only in `perch list --json` and under `PERCH_DEBUG=1`.
Rows are grouped by project, the groups ordered by urgency, and each state has
a glyph as well as a colour so the board reads without colour: `⚑` needs_input,
`✓` done, `▶` working, `…` starting, `·` idle, `✕` ended. Ended panes are
collapsed behind a footer count until you press `e`.

Panes with subagents carry a badge in the harness column — `claude ⚑1 ▶4 ✓48`,
blocked, running and finished. Only subagents that are running or blocked get a
row of their own; `Space` folds the finished ones back out, newest first, and
folds them away again. A pane you have seen drops the finished ones entirely,
so the board never fills up with work that is over. Jumping to a subagent lands
on its parent pane, because that is where it lives.

Claude runs subagents in the background: the main agent's turn ends while they
keep working, and it is woken again when each one finishes. A pane in that
state reads `▶ delegating` — it sorts with the working panes, gets no chime and
no card, and `n` skips it. It goes back to saying `done` (and chimes on the
next turn's end) once its subagents are in. Sending a follow-up to a background agent
counts too, and an agent Claude runs for its own internal purposes does not.

| key | action |
|---|---|
| `j` / `k` (or ↓ / ↑) | move the cursor |
| `Enter` | jump to the selected pane and exit |
| `gg` / `G` | first / last row (a second `g` within 500 ms) |
| `n` | select the oldest waiting pane |
| `m` | toggle global mute |
| `x` | dismiss a `done` pane back to `idle` |
| `e` | show or hide `ended` panes |
| `Space` / `Tab` | expand or collapse a pane's finished subagents |
| `v` | grouped by project ⇄ flat, newest change first |
| `?` | help overlay (any key closes it) |
| `r` | refresh now |
| `S` | run `perch setup` (shown as a banner until perch is wired) |
| `q` / `Esc` | quit |

### Answering an agent's questions

When Claude asks you something with its question tool — the multiple-choice
dialogs a skill like `ask-user` drives, or any `AskUserQuestion` — the chime
plays and **the answer form pops up in the middle of whatever you are doing**,
on every attached client, the asking pane included. Each question is a tab
across the top, answered ones ticked; `j`/`k` move, `Enter` picks (or `Space`
ticks several, then `Enter` moves on), `Other…` opens a free-text line,
`←`/`→` (or `Tab`/`Shift-Tab`, `h`/`l`) go back and forth between the tabs,
and `Enter` on the last tab sends. The answers go to Claude exactly as if you
had clicked them and the pane goes back to `working`. Answer on one client and
the popup on every other client closes too. The popup is sized to its content:
long questions and descriptions wrap rather than being cut, within the client
it is drawn on.

**The popup is the dialog.** perch holds a question only while its popup is
on screen. Close it — `Esc`, or `p` to close it and jump to the pane — and the
question goes to Claude at once: its own dialog appears in the pane, and that
is where it is answered from then on. There is no in-between state where a
pane looks busy but nothing is asking. The board shows such a pane as
`⚑ question` with the question as its message; `Enter` jumps to it.

The popup's title says whose question it is — `⚑ perch (main) · claude · api`:
project and branch, harness, and the pane's tmux window — so two sessions
asking at once are told apart at a glance.

**Several agents asking at once queue up.** A client shows one popup at a
time; a question that arrives while you are answering another waits, the title
reads `+1 waiting`, and when you finish the set (send, `Esc` or `p`) the next
set appears in the same popup, oldest first. A client that is sitting on a pane
with an agent's own dialog open is left alone until it moves.

All three harnesses are answered this way, each through the channel it has.
Claude's hook API lets a hook answer its own tool (`PreToolUse`, or in auto
mode `PermissionRequest`, returning `updatedInput` with `answers`). pi has no
question tool, so perch's pi extension registers one — `ask_user` — that sends
the questions to `perch hook pi --ask`, returns the popup's answers to the
model, and falls back to pi's own select dialog if perch hands the question
back. Codex's own `request_user_input` cannot be answered by a hook and exists
only in Plan mode, so `perch setup` registers a tiny MCP server (`perch mcp`)
in Codex's config that gives its model the same `ask_user` tool, in every mode;
the skill tells Codex to prefer it. A Codex `request_user_input` that does fire
is still flagged — the chime and the `⚑ needs_input` row — and `Enter` takes
you to the pane.
Question kinds the form has no control for (a number slider) go to Claude at
once. `[ask] enabled = false` turns the whole thing off. If a question ever
reaches Claude's own dialog when you expected the popup,
`~/.local/state/perch/ask.log` says what perch did with it and why. An install
from 0.3 or earlier needs one `perch setup` to gain the two extra hook entries;
`perch doctor` still reads as wired without it.

**Teach your agents to ask this way.** None of this fires for a question typed
into the chat: it has to go through the harness's question tool. A skill that
tells every agent to do so — with headers short enough for the tabs, a
recommended option first, and a grill mode for stress-testing plans — is the
other half of the feature. It ships in this repo as
[`skills/ask-user/SKILL.md`](skills/ask-user/SKILL.md): symlink or copy the
directory into `~/.claude/skills/` (Claude) or `~/.agents/skills/` (Codex, pi).

## Config

`~/.config/perch/config.toml`. Everything is optional; missing or unparseable
files fall back to these defaults.

```toml
cooldown_secs = 3     # per-pane minimum gap between sounds
watch_commands = []   # reserved; perch does not scrape pane text

[sounds]
done = "Glass"
needs_input = "Ping"
error = "Basso"

[ask]
enabled = true        # answer Claude's questions through perch's popup (see TUI keys)
```

A sound is the only thing perch does to get your attention: it costs no screen,
cannot eat a keystroke, and needs no cleanup. The hook also sets the pane
option `@perch_state`, so you can put perch's state in your own status line.

A bare name resolves to `/System/Library/Sounds/<name>.aiff`; a value
containing `/` is used as a path. Sounds play only on transitions into `done`
and `needs_input`.

## State and identity

A pane is identified by its tmux pane id (`%NN`), read from `$TMUX_PANE`, which
hook processes inherit from the agent. Records live in
`~/.local/state/perch/panes/<pane>.json`, written atomically, with an
append-only `~/.local/state/perch/events.jsonl` log (rotated at 5 MB), a
`mute` marker file, and `answers/<pane>.json` while an answer is on its way
from the board to the hook waiting for it.

Outside tmux there is no `$TMUX_PANE`, so `perch hook` reads its payload and
exits without writing anything — the agent runs exactly as before. `list`,
`status` and `tui` still read whatever is on disk.

Liveness is checked on every read: `tmux list-panes -a` says which panes exist,
a record whose pane is gone flips to `ended`, and an `ended` record is deleted
an hour later.

## Troubleshooting

**No sound.** Check the mute file: `ls ~/.local/state/perch/mute` — if it
exists, unmute with `m` in the TUI or delete it. Check the player and the file
itself: `/usr/bin/afplay /System/Library/Sounds/Glass.aiff`. A sound name that
does not resolve to an existing file is skipped silently. `PERCH_NO_SOUND=1`
in the environment disables sound for that process. Then try
`perch sound test done`.

**A pane shows `ended`.** Its tmux pane no longer exists — the shell exited or
the pane was killed. That is the liveness rule, not a bug; the row disappears
an hour later, or immediately after `tmux kill-pane` plus a refresh.

**A pane says `needs_input` but nothing is waiting.** That should no longer
happen: `needs_input` now means only a real approval or question dialog
(Claude's `permission_prompt`, `elicitation_*`, `agent_needs_input`, or a codex
`PermissionRequest`). Claude's `idle_prompt` nudge is logged and ignored, and a
stale `needs_input` clears on the next tool call. If you are on an older
install, re-run `perch setup` — the `PreToolUse` hook is new.

**A subagent is missing.** Only running and blocked subagents get a row; the
finished ones are the `✓N` in the pane's badge, and `Space` shows them. They do
not last: seeing the pane clears them, a pane keeps at most twenty, and
anything finished is pruned after ten minutes. A subagent still claiming
`working` is only ever retired by the session ending, its pane going away, or
two hours passing.
If none ever appear at all, re-run `perch setup` — the `SubagentStart` /
`SubagentStop` hooks were added later than the rest.

**Nothing shows up for codex.** Run `perch doctor`: if it says `trust: no`,
codex is refusing to run the hook. `perch setup` writes the trust record;
after editing `~/.codex/hooks.json` by hand you need to run it again.

**Checking what a harness really sends.** Set
`PERCH_DUMP_HOOK_INPUT=~/perch-payloads` and every raw hook payload is copied
to `<dir>/<harness>-<event>-<timestamp>.json`.

**Nothing shows up at all.** Confirm the hooks merged
(`perch install claude --dry-run` should say "already installed for every
event") and that you are inside tmux (`echo $TMUX_PANE`).

## What the states mean

perch uses herdr's definitions:

| state | meaning |
|---|---|
| `working` | a turn is in progress |
| `delegating` | the pane's own turn ended, but subagents it spawned are still running |
| `needs_input` | a permission prompt is on screen; the agent is blocked on you |
| `question` | the agent asked you something: perch's popup, or its own dialog, is waiting |
| `done` | the turn finished and you have not looked at the pane since |
| `idle` | ready for input, and seen |
| `starting` / `ended` | the session has not reported yet / its pane is gone |

**`done` means finished while you were elsewhere; `idle` means you have seen
it.** A pane counts as seen when a *focused* tmux client is showing it — where
"focused" means the terminal window itself has keyboard focus, so a pane left
on screen behind your browser is not seen. (On a terminal that never reports
focus, any client showing the pane counts.)

A turn that ends in a pane you are watching goes straight to `idle` without a
chime. Afterwards, seen is re-evaluated from the live client list on **every
read** — `perch list`, `perch status`, the dashboard refresh — so a pane you
have since looked at stops saying `done` however you got there: `prefix n`,
`prefix p`, `prefix l`, a session picker, or clicking the window. The tmux
hooks the snippet installs only make it immediate; they are not what makes it
correct. `needs_input` always sounds.

## Docs

- [docs/PHILOSOPHY.md](docs/PHILOSOPHY.md) — what perch is for, and what it refuses to do
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — model, layout, invariants
- [docs/DECISIONS.md](docs/DECISIONS.md) — why it is shaped this way
- [docs/NOTIFICATIONS.md](docs/NOTIFICATIONS.md) — the sound and the card
- [docs/TESTING.md](docs/TESTING.md) — the real-tmux harness
- [docs/PLAN.md](docs/PLAN.md) — the original plan
- [CHANGELOG.md](CHANGELOG.md)
