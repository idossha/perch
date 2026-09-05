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

```sh
cargo install --path .
```

macOS is the primary target: sounds use `afplay` and `/System/Library/Sounds`,
and optional desktop notifications use `osascript`. Everything else works
anywhere tmux does.

## Setup

**Claude Code** — merges a `perch hook claude` entry into the `SessionStart`,
`UserPromptSubmit`, `Stop`, `Notification` and `SessionEnd` arrays of
`~/.claude/settings.json`, appending beside whatever hooks you already have and
backing the file up first:

```sh
perch install claude --dry-run   # show what would change
perch install claude
```

**tmux** — writes `~/.config/perch/perch.tmux.conf` (a `prefix + g` popup
binding and a `status-right` counter). It never edits `~/.tmux.conf` in place
beyond one line; by default it prints the line for you to add yourself:

```sh
perch install tmux
# add this line to ~/.tmux.conf:
source-file /Users/you/.config/perch/perch.tmux.conf
```

Pass `--apply` to have perch append that `source-file` line for you (backing up
`~/.tmux.conf` first). Then `tmux source-file ~/.tmux.conf`.

**Codex and pi** — `perch install codex` and `perch install pi` are recognised
but currently exit with "codex and pi installers land in phase 3".

## Commands

```
perch hook <claude|codex|pi>          read a hook payload on stdin, update this pane
perch list [--json]                   snapshot of every tracked pane
perch status [--format plain|tmux]    one-line summary for the status bar
perch next                            focus the oldest waiting pane
perch tui                             the dashboard
perch sound test <event>              play the sound for done | needs_input | error
perch install <claude|codex|pi|tmux> [--dry-run] [--print] [--apply]
```

- `hook` reads one JSON object on stdin and always exits 0, so a broken perch
  can never fail your agent. Outside tmux it does nothing.
- `list` prints pane, project (branch), harness, state, age and last message;
  `--json` prints the full records.
- `status --format plain` prints e.g. `⚑2 ▶1 ✓1` (waiting, working, done);
  `--format tmux` adds tmux colour escapes for `status-right`.
- `next` prints the pane id of the oldest `needs_input`/`done` pane and moves
  the current client to it, or prints `nothing waiting`.
- `install --dry-run` reports without writing; `--print` writes nothing and
  dumps the merged settings JSON (claude) or the config snippet (tmux) to
  stdout; `--apply` is tmux-only and appends the `source-file` line.

## TUI keys

Run it from the popup binding (`prefix + g`) or directly with `perch tui`.
The table shows pane, project (branch), harness, state, age and last message,
sorted needs_input, done, working, starting, idle, ended — oldest first.

| key | action |
|---|---|
| `j` / `k` (or ↓ / ↑) | move the cursor |
| `Enter` | jump to the selected pane and exit |
| `n` | select the oldest waiting pane |
| `m` | toggle global mute |
| `x` | dismiss a `done` pane back to `idle` |
| `r` | refresh now |
| `q` / `Esc` | quit |

The view refreshes from disk every second.

## Config

`~/.config/perch/config.toml`. Everything is optional; missing or unparseable
files fall back to these defaults.

```toml
notify = false        # also post an osascript desktop notification
cooldown_secs = 3     # per-pane minimum gap between sounds
watch_commands = []   # reserved; perch does not scrape pane text

[sounds]
done = "Glass"
needs_input = "Ping"
error = "Basso"
```

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

**Nothing shows up at all.** Confirm the hooks merged
(`perch install claude --dry-run` should say "already installed for every
event") and that you are inside tmux (`echo $TMUX_PANE`).

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — model, layout, invariants
- [docs/DECISIONS.md](docs/DECISIONS.md) — why it is shaped this way
- [docs/PLAN.md](docs/PLAN.md) — the original plan
- [CHANGELOG.md](CHANGELOG.md)
