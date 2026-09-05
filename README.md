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
perch list [--json]                   snapshot of every tracked pane
perch status [--format plain|tmux]    one-line summary for the status bar
perch next --client <name>            jump a client to the oldest waiting pane
perch open --client <name>            open the dashboard in a popup on a client
perch tui --client <name>             the dashboard itself
perch sound test <event>              play the sound for done | needs_input | error
perch seen <pane>                     mark a pane seen: done -> idle
perch install <claude|codex|pi|tmux> [--dry-run] [--print] [--apply]
perch setup [--dry-run] [--yes] [--no-tmux] [--only ...]
perch doctor [--json]
perch uninstall [--dry-run] [--keep-state]
```

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
Rows are grouped by project, the groups ordered by urgency, and each state has
a glyph as well as a colour so the board reads without colour: `⚑` needs_input,
`✓` done, `▶` working, `…` starting, `·` idle, `✕` ended. Ended panes are
collapsed behind a footer count until you press `e`. Panes with subagents show
a `+N` badge and one indented `└` row per live subagent; jumping to a subagent
lands on its parent pane.

| key | action |
|---|---|
| `j` / `k` (or ↓ / ↑) | move the cursor |
| `Enter` | jump to the selected pane and exit |
| `gg` / `G` | first / last row (a second `g` within 500 ms) |
| `n` | select the oldest waiting pane |
| `m` | toggle global mute |
| `x` | dismiss a `done` pane back to `idle` |
| `e` | show or hide `ended` panes |
| `v` | grouped by project ⇄ flat, newest change first |
| `?` | help overlay (any key closes it) |
| `r` | refresh now |
| `S` | run `perch setup` (shown as a banner until perch is wired) |
| `q` / `Esc` | quit |

### Navigation guarantees

- Every jump is one `tmux switch-client -c <client> -t <pane_id>` — the client
  is always named, so a second attached client or a popup's own pty can never
  send you to the wrong screen, and the pane is addressed by id, so a duplicate
  window or session name cannot either.
- The cursor is keyed by pane id, not by row number. The board reorders itself
  as agents change state; the selection stays on the agent you picked.
- A jump is waited on and checked. If the pane is gone, the dashboard says
  `pane %N is gone` and stays open instead of exiting or moving you somewhere
  else. Only a real move closes the popup.
- `PERCH_DEBUG=1` prints each jump's exact tmux command to stderr.

The view refreshes from disk every second. The `g` and `e` choices are
remembered in `~/.local/state/perch/tui.json`. The palette is `dark` by
default; set `PERCH_THEME=light` or `[tui] theme = "light"` in the config for
a light terminal.

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
append-only `~/.local/state/perch/events.jsonl` log (rotated at 5 MB) and a
`mute` marker file.

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

**A subagent is missing.** Subagents show as indented rows under their pane,
and only while the harness reports them: they are cleared by your next prompt
and pruned ten minutes after they finish. If none ever appear, re-run
`perch setup` — the `SubagentStart`/`SubagentStop` hooks were added later than
the rest.

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
| `needs_input` | a real approval or question is on screen; the agent is blocked on you |
| `done` | the turn finished and you have not looked at the pane since |
| `idle` | ready for input, and seen |
| `starting` / `ended` | the session has not reported yet / its pane is gone |

**`done` means finished while you were elsewhere; `idle` means you have seen
it.** A turn that ends in the pane you are watching goes straight to `idle`
without a chime, switching to its window or pane marks it seen (a tmux `after-select-window` hook runs
`perch seen`), and so do `prefix + N` and the TUI's jump. `needs_input` always
sounds.

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — model, layout, invariants
- [docs/DECISIONS.md](docs/DECISIONS.md) — why it is shaped this way
- [docs/PLAN.md](docs/PLAN.md) — the original plan
- [CHANGELOG.md](CHANGELOG.md)
