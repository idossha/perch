# perch — philosophy

perch is an attention list, not a log. It answers one question — *what needs me
now?* — and everything below exists to keep that answer true. Each principle is
followed by the failure it prevents.

## Observe, never own

perch creates no sessions, no windows, no worktrees and no daemon. It reads
hook events, writes one file per pane, and moves your client when you ask.
*Prevents:* the tools that own your terminals make you migrate into them, and
leave orphaned sessions behind when they crash. perch can be deleted mid-turn
and nothing of yours is lost.

## Hooks are the authority; scraping is not

State comes only from harness lifecycle events. Pane text and status-bar output
are never parsed.
*Prevents:* a board that lies after `/clear`, after a colour-scheme change, or
whenever a harness redraws its footer. Events are deterministic; pixels are
not. The one exception proves it: *seen* is derived from tmux's live client
list, because no harness can report where you are looking.

## The dashboard shows what needs you now

Rows are ordered by urgency — `needs_input`, then `done`, then `working` — and
what is finished is folded into a count. History is one keystroke away, never
in the way.
*Prevents:* the screenshot that prompted this rule — forty-eight finished
subagents filling a pane's rows while the four that were still running, and the
one blocked on a question, scrolled off the bottom. A list you have to read is
not an attention list.

## Finished means finished

A parent turn ending retires the subagents it spawned; a pane reaching `idle`
drops everything under it that has finished; a pane keeps at most twenty
children.
*Prevents:* children stuck at `working` for twenty-five minutes because a
`SubagentStop` never arrived, and records that grow without bound. A missing
event must never be able to make the board wrong forever.

## States mean what herdr says they mean

`working` — a turn is in progress. `needs_input` — a real approval or question
dialog is blocking the agent, and nothing else. `done` — finished, and you have
not seen it. `idle` — finished, and seen. `ended` — the pane or session is gone.
*Prevents:* `needs_input` becoming a synonym for "something happened". Claude's
`idle_prompt` nudge is not a request, so it is logged and ignored; a stale
`needs_input` clears on the next tool call.

## Navigation is a contract

One primitive — `tmux switch-client -c <client> -t <pane_id>` — with the client
always named and the pane always addressed by id. The jump is waited on and
checked.
*Prevents:* a jump issued from a popup's own pty, or with two clients attached,
moving the wrong screen; and a duplicate window name sending you to the wrong
project.

## A sound is the whole cue

The only thing perch does to get your attention is play a sound. It costs no
screen and cannot eat a keystroke. Subagents never chime except when one is
genuinely blocked on you.
*Prevents:* a notification system you turn off, which is a notification system
that does not work.

## Every claim is proven against a real tmux server

The e2e suite starts a private tmux server on its own socket, attaches a real
client in a pty, and drives the real binary with committed harness payloads.
*Prevents:* a mock that agrees with the code and disagrees with tmux. If a rule
here is not tested that way, it is a wish, not a rule.
