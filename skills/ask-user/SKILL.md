---
name: ask-user
description: How to ask the user anything — a decision, a clarification, a preference, an approval of a plan, or a full plan stress-test ("grill me") — through the harness's structured question tool so the question reaches the user wherever they are (perch pops it up and returns the answer). Use whenever you would otherwise type a question into the chat and wait. Not for questions the codebase or the conversation already answers, and not for permission prompts, which the harness raises itself.
---

# Ask User

Ask through the harness's question tool, never through plain chat text. A
question typed into the transcript is invisible to everything watching the
session; a question raised through the tool is a structured event that perch
picks up, pops up in front of the user on whatever tmux window they are in,
and answers back into this session.

## Before asking

1. Try to answer it yourself: read the code, the docs, the conversation, the
   user's earlier decisions. Ask only what you genuinely cannot resolve.
2. Never ask for authority you already have, and never turn a mandatory safety,
   destructive-action or secret boundary into a multiple-choice question: state
   the boundary and stop.
3. Batch what belongs together. One call may carry one to four questions; ask
   them together when they are independent, in sequence when one depends on
   another's answer.

## The tool, per harness

| Harness | Tool | Notes |
|---|---|---|
| Claude Code | `AskUserQuestion` | perch answers it in place: the popup's answer is returned to you as if the user had clicked it. |
| Codex | `ask_user` (perch's MCP tool) | Registered by `perch setup`; available in every collaboration mode. perch pops the form up and returns the answers. Prefer it over Codex's own `request_user_input`, which exists only in Plan mode and which perch can only flag, not answer. If `ask_user` is missing, perch is not wired for Codex: say so once, then use `request_user_input` in Plan mode or ask in chat (below). |
| pi | `ask_user` | Provided by perch's pi extension (`perch setup`). perch pops the form up and returns the answers; if perch hands the question back, the tool falls back to pi's own select dialog. If the tool is missing, perch is not wired for pi: say so once, and ask in chat (below). |
| others | the harness's question or elicitation tool, if it has one | Otherwise ask in chat (below). |

Never fall back to chat text while a question tool is available. When it is
not — a non-interactive run, a harness perch is not wired into — ask in chat in the same
shape, so the user still gets a form to answer: one message, the questions
numbered, each with its lettered options and your recommendation marked, and
end by saying you are waiting for their picks. If even that is impossible (no
one can answer), make the decision from established defaults, say which
default you took, and continue. Never tell the user the popup failed: the
popup is perch's, and it appears only for questions raised through a tool.

## Shape each question so it survives the popup

The user sees each call as a small form: one tab per question, its options as
a list, an `Other…` line, and a title naming your project and pane. Write for
that form:

- **`header`**: two or three words, at most twelve characters. It is the tab
  label.
- **`question`**: one sentence, self-contained, unique within the call. It is
  also the key the answer comes back under, so it must not repeat another
  question's text.
- **`options`**: two to four, mutually exclusive unless `multiSelect` is on.
  Put your recommendation first and mark it `(Recommended)`. Each `label` is
  short; each `description` is one line on what choosing it leads to. Do not
  add an "Other" or "Skip" option; the form provides free text and lets a
  question go unanswered.
- **Multi-select** only when several answers can truly hold at once.
- No number sliders: perch hands those back to the harness's own dialog.

## After the answer

- Treat the returned answer as the user's decision. A free-text answer is an
  instruction, not a label; read it as such.
- An unanswered question means "your call": take your recommended option and
  say so.
- Record decisions where they belong: a design decision in the project's
  `docs/DECISIONS.md`, a working preference through the memory system.

## Grill mode

When the user asks to be grilled, to stress-test a plan, or to reach shared
understanding before you build:

1. If they named the topic, start there; otherwise ask for it (one question).
2. Walk the decision tree one branch at a time, resolving dependencies in
   order. Each call carries the questions that are ready to be asked now, each
   with your recommendation first.
3. Only ask, recommend, read context and record answers. No edits, builds,
   commits, pushes or other side effects while questions are open.
4. When every branch is resolved, summarize the decisions as a short list and
   ask, through the tool, whether to apply them. Implement only on that yes.

## When perch does not pick it up

perch needs its hook on this harness. Once per machine, not per question:

```sh
perch doctor || perch setup
```

A session started before `perch setup` ran keeps its old hook set until it is
restarted. If the user is looking at your pane when you ask, perch leaves the
question to the harness's own dialog on purpose.
