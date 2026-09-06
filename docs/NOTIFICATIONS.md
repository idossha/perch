# Notifications

The sound tells you *something* happened. The card tells you *what*, without
you having to look anywhere.

## What you see

On a transition into `done` or `needs_input`, perch draws a small card with a
**rounded grey border** in the center of every attached client. The popup keeps
the terminal's own background (`-s bg=default,fg=default`), so it reads as part
of the screen rather than as a banner:

```
╭──────────────────────────────────╮
│  ✓ done  perch (main)            │
│  editor.1  ·  claude             │
│  Added the reducer and its tests.│
╰──────────────────────────────────╯
```

| Line | Content |
| --- | --- |
| 1 | `✓ done` / `⚑ needs input` in the dashboard's state colour, bold; then `<project>` in the normal foreground and ` (<branch>)` dim |
| 2 | the pane's tmux location and the harness, both dim, joined by `  ·  ` |
| 3 | the agent's last message, normal foreground, cut with an `…` when it does not fit |

The colours are the dashboard's own: `colour114` for `done`, `colour203` for
`needs_input`, `colour240` for the border. The card's text field is as wide as
its widest line plus two columns of padding each side, clamped to **36–72**
columns; the popup is two columns wider again for the border it draws, and five
rows tall (`-h 5`: three lines between the two border rows).

Missing fields degrade: no project shows `—`, no location shows the pane id, no
last message shows an empty line.

The body prints raw text into the popup's pty, where `#[fg=…]` means nothing —
so the colours are ANSI SGR sequences (`\x1b[1;38;5;114m` and friends) built
from the configured colour names.

Every attached client gets its own popup, centered on *that* client's screen. A
second card on the same client replaces the first, so two transitions in a row
never stack.

## When it happens

Exactly when the sound happens: a **parent** state change into `done` or
`needs_input`. Subagent events are not parent transitions and draw nothing.
Working, idle and ended draw nothing.

The hook never waits on any of this. It spawns a detached `perch notify <kind>
--pane <pane>`, which spawns one `tmux display-popup` per client and exits.

## Fade timings

Default total: **3500 ms**, fades included.

| Phase | Duration | Steps |
| --- | --- | --- |
| Fade in | 300 ms | 3 steps, dim → normal → full |
| Hold | the remainder | — |
| Fade out | 500 ms | the same 3 in reverse |

Nothing flashes: no background ever changes hands. Each step restyles the
border from inside the popup (`tmux display-popup -S fg=colour240[,dim]`) and
reprints the three lines with different SGR — level 0 dims every run, level 1
drops the bold, level 2 is the card as designed.

## The passthrough guarantee

**A card can never eat a keystroke.** The popup takes the client's keyboard
while it is up, so the body:

1. reads the client's active pane *before* raw mode
   (`tmux display -p -c <client> '#{pane_id}'`),
2. waits for a key on a reader thread,
3. on the first key, forwards the exact bytes with
   `tmux send-keys -t <pane> -l -- <bytes>` and exits immediately.

`-l` keeps the bytes literal; `--` keeps a leading `-` out of tmux's flag
parsing. The popup closes and the character you typed lands where you thought
you were typing it. This is the reason the card is allowed to take the screen
at all.

## Configuration

```toml
[notify]
enabled = true      # false: sound and @perch_state only, no card
duration_ms = 3500  # total, fades included

# Colour names, tmux-style (`colour114`, `114`, `green`, `brightred`).
accent_done = "colour114"         # the dashboard's done colour
accent_needs_input = "colour203"  # …and its needs_input colour
border = "colour240"              # the rounded border
```

An unrecognised colour name falls back to the terminal's default foreground.

Retired keys — the whole `[toast]` table, `[notify] desktop` / `tmux_message`,
and the old `done_style` / `needs_input_style` fade arrays — still parse and are
ignored, so an old config file keeps working.

## Checking it

```
perch notify test
```

draws a sample card on every attached client, so you can check placement and
colours without waiting for an agent to finish. `test` ignores `enabled`.

## Troubleshooting

**No card at all.**

- `tmux -V` must report **3.2 or newer**; `display-popup` does not exist
  before that, and perch degrades to sound only.
- `enabled = false` in `[notify]`.
- A popup is already open on that client — your own dashboard (`perch open`),
  or another popup binding. tmux allows one popup per client, and yours wins.
- No client is attached: a card is drawn per attached client, and a detached
  session has none.

**A card, but no sound**: separate systems. See `perch sound test done` and the
global mute (`~/.local/state/perch/mute`).

**Cards from the wrong agent**: the card describes the pane the hook fired in,
not the pane you are looking at. Line 2 is where to go.

**Under `PERCH_NO_TMUX=1`** nothing is drawn; the request is written to
`PERCH_TMUX_LOG` as `notify <kind> <pane>`, which is how it is tested.
