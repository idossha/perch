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
`~/.config/perch/perch.tmux.conf` with the `prefix + g` popup binding and adds
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
perch next                            focus the oldest waiting pane
perch tui                             the dashboard
perch sound test <event>              play the sound for done | needs_input | error
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
| `S` | run `perch setup` (shown as a banner until perch is wired) |
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
