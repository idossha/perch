# Notifications

The sound tells you *something* happened. The card tells you *what*, without
you having to look anywhere.

## What you see

On a transition into `done` or `needs_input`, perch draws a borderless
three-line card in the **center of every attached client**:

```
        ✓ done  perch (main)
          editor.1  claude
   Added the reducer and its tests.
```

| Line | Content |
| --- | --- |
| 1 | `✓ done` or `⚑ needs input`, then `<project> (<branch>)` |
| 2 | the pane's tmux location, then the harness (`claude`, `codex`, `pi`) |
| 3 | the agent's last message, cut with an `…` when it does not fit |

The card is as wide as its widest line plus four columns, clamped to 30–70
columns, and always three rows tall. Missing fields degrade: no project shows
`—`, no location shows the pane id, no last message shows an empty line.

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
| Fade in | 300 ms | 3 restyles, dim → mid → full |
| Hold | the remainder | — |
| Fade out | 500 ms | the same 3 in reverse, full → mid → dim |

The fade is real, not simulated: the popup's body runs `tmux display-popup -s
<style>` **from inside the popup**, which restyles it in place. `done` fades
through green (`colour235` → `colour22` → `colour28`), `needs_input` through
red (`colour235` → `colour88` → `colour160`).

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

# Optional. Three tmux styles each, dim → mid → full. Fewer than three is
# treated as a typo and the built-in fade is kept.
done_style = [
  "bg=colour235,fg=colour240",
  "bg=colour22,fg=colour250",
  "bg=colour28,fg=colour255,bold",
]
needs_input_style = [
  "bg=colour235,fg=colour240",
  "bg=colour88,fg=colour250",
  "bg=colour160,fg=colour255,bold",
]
```

Retired keys — the whole `[toast]` table, and `[notify] desktop` /
`tmux_message` — still parse and are ignored, so an old config file keeps
working.

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
