# Testing perch

Two suites, with different ideas of what "real" means.

| Suite | Files | tmux | What it proves |
| --- | --- | --- | --- |
| unit / integration | `tests/adapter_*.rs`, `tests/hook_end_to_end.rs`, `tests/tui_render.rs`, … | none (`PERCH_NO_TMUX=1`, `NullTmux`) | payload → event → state, and the exact tmux commands perch *would* run |
| end-to-end | `tests/e2e_*.rs` + `tests/e2e/mod.rs` | a **real** private server with a **real** attached client | that those commands actually do what they claim, in tmux |

Run everything with `cargo test`. Run only the end-to-end suite with:

```sh
scripts/e2e.sh
```

It installs nothing; it is `cargo test --test 'e2e_*'` with a warning when tmux
is missing.

## The end-to-end harness

`tests/e2e/mod.rs` is shared by every `tests/e2e_*.rs` via
`#[path = "e2e/mod.rs"] mod e2e;`. Cargo compiles only top-level files in
`tests/`, so the directory is a module, not a test target of its own, and each
test file is its own binary — they run in parallel, each on its own server.

`Server::start()` gives you:

- a private tmux server on `-L perch-e2e-<pid>-<n>` with `-f /dev/null`, under a
  temporary `TMUX_TMPDIR`, holding one session `one` with a `sleep` pane;
- temporary `PERCH_STATE_DIR`, `PERCH_CONFIG_DIR` and `PERCH_HOME`, plus
  `PERCH_NO_SOUND=1` and an empty `config.toml`, so a stray `~/.config/perch`
  cannot reach a test;
- a `Drop` that runs `kill-server`, so no server outlives its test even when the
  test panics.

`Server::attach("one")` spawns `tmux -L … attach` inside a 120x40 pty
(`portable-pty`), drains the pty so tmux never blocks on a full buffer, and
discovers the client's name by polling `list-clients -F '#{client_name}'` for up
to three seconds. `Client`'s `Drop` kills it.

Helpers: `capture(pane)`, `client_pane(client)`, `pane_option(pane, name)`,
`send_keys(pane, keys)`, `wait_for(cond, timeout)` (50 ms polling), `hook`,
`records()`, `state_of(pane)`, `seed_record(...)`.

### How perch is told which server to talk to

This is the one thing worth knowing before writing another e2e test.
`src/tmux.rs` shells out to a bare `tmux` with **no `-S` and no `-L`**, so the
socket cannot be handed over as a flag. tmux itself falls back to the `TMUX`
environment variable — `<socket path>,<server pid>,<session id>` — which is
exactly what a real pane already carries. So the harness gives every perch child

```
TMUX=<socket_path>,<server pid>,0
TMUX_PANE=<%id>
```

and the real `RealTmux` code path runs against the private server. No
`PERCH_TMUX_SOCKET` knob was needed, and `PERCH_NO_TMUX` is never set here — that
would defeat the whole point. Two details that fall out of it:

- `#{socket_path}` must be read from the server, not guessed: with a custom
  `TMUX_TMPDIR` the socket lands at `$TMUX_TMPDIR/tmux-<uid>/<label>`, not at
  `$TMUX_TMPDIR/<label>`.
- Every tmux call in the harness passes `-L <label>` **and** `env_remove("TMUX")`,
  so a developer running the suite from inside their own tmux cannot have a
  command resolved against their own server.

`PerchRun::run` also does `env_remove("PERCH_NO_TMUX")`, because CI sets that
variable workflow-wide for the unit suite.

### What the tests cover

- `e2e_hooks_claude.rs`, `e2e_hooks_codex.rs`, `e2e_hooks_pi.rs` — every fixture
  piped through `perch hook <harness>` with a real `TMUX_PANE`; the resulting
  `perch list --json` state and the `@perch_state` pane option are both checked
  after each step. In particular, a `Stop` while the attached client is looking
  at the pane yields `idle`, and the same `Stop` while the client is on another
  window yields `done` — the one assertion that cannot be made without a real
  attached client.
- `e2e_tui_render.rs` — `perch tui --client <name>` run inside a pane; the
  header, the group headers, the state glyphs, the single-line footer, `?` for
  help and `q` to quit are all read back with `capture-pane`. Never a screenshot.
- `e2e_navigation.rs` — Enter moves the real client to the selected pane and
  marks it seen; `perch next --client` lands on the oldest wait across two
  sessions; a pane killed between render and Enter is reported `pane %N is gone`
  and the client does not move.
- `e2e_setup.rs` — `perch setup` against a fake home seeded from
  `tests/fixtures/codex_hooks_existing.json`; the generated
  `perch.tmux.conf` is sourced into the private server and `list-keys` must show
  `g` and `N` bound to perch; sourcing twice and running setup twice are both
  no-ops; `perch doctor --json` reports everything wired; `perch uninstall`
  restores the home.
- `e2e_ask.rs` — the real `perch hook claude --ask` waiting in a pane's
  environment while its popup is answered by typing **into the client's pty**
  (`Client::type_bytes`; `send-keys` cannot address a popup), asserting on the
  hook's stdout — the decision Claude would receive. Esc hands the question to
  Claude and the board then shows `⚑ question` with Enter to jump. The popup
  body `perch ask-form` run in a pane is read back and exits when the question
  is answered elsewhere. Two agents asking at once are answered in order in
  the one popup.
- `e2e_ask_flow.rs` — the popup's whole contract on a real server: `Esc`
  hands a set to Claude at once and a real window switch (the installed tmux
  hooks, sourced) reopens nothing; `p` hands the question back, lands the
  client on the pane, ends the popup, and the next waiting question pops up
  only once the client looks away again; two attached clients both get the
  popup and one answer closes both; the body read back from a pane names
  project · harness · window, counts `+1 waiting`, and moves on to the next
  set in place; a question you are looking at pops up over the pane and is
  answered there. Popups are counted through their per-client claim files.
- `e2e_ask_real.rs` — env-gated like the real-harness test: a real interactive
  `claude` in a pane, asked to use its question tool, answered through the
  real popup on the test client, and the answer Claude prints back checked;
  then two real sessions asking at once, answered in turn in one popup.
- `e2e_real_harness.rs` — env-gated, see below.

### Rules these tests keep

- Never the user's default server: a unique socket per test, `-L` on every call,
  `TMUX` scrubbed from inherited env.
- Never the screen: the client lives in a pty, and every assertion is on text
  from `capture-pane`, `display -p` or `show-options`.
- Never sound: `PERCH_NO_SOUND=1` everywhere.
- Skip, don't fail: with no `tmux` on `PATH`, `e2e::no_tmux()` prints
  `skipping: tmux is not on PATH` and the test returns green.

The dashboard is driven inside a normal pane rather than a `display-popup`,
because a popup is not a pane and `send-keys` cannot address it. The command run
is byte-for-byte the one the popup runs; `perch open --client <name>` is only the
wrapper that opens the popup.

## The real-harness test

`tests/e2e_real_harness.rs` runs `codex exec …` / `claude -p …` for real, and
`tests/e2e_ask_real.rs` drives a real interactive `claude` through its question
tool and perch's popup. Both are behind the same variable.

**It uses the user's own harness installation.** It installs nothing and edits
nothing: the hooks that fire are whatever is already in `~/.codex/hooks.json` and
`~/.claude/settings.json`. What keeps it from polluting anything is that the pane
it runs in carries this test's `PERCH_STATE_DIR`, so the records those real hooks
write land in a temp directory. It costs tokens and needs network.

It is off unless `PERCH_E2E_REAL=1`, and prints why it skipped otherwise:

```sh
PERCH_E2E_REAL=1 scripts/e2e.sh
```

It also skips, with a reason, when the harness binary is not on `PATH`.

## CI

`.github/workflows/ci.yml` does not install tmux, so on the runners the e2e
suite currently skips itself. To actually run it, add one step to the `test` job,
before `cargo test`:

```yaml
      - name: install tmux
        run: ${{ runner.os == 'macOS' && 'brew install tmux' || 'sudo apt-get update && sudo apt-get install -y tmux' }}
```

(macOS runners already ship tmux, so `brew install tmux` is usually a no-op; the
Ubuntu half is the one that matters.) Note that the workflow sets
`PERCH_NO_TMUX: "1"` at the top level for the unit suite — the harness strips it
from every perch child, so no change is needed there.

## Golden snapshots

`tests/golden.rs` renders one fixture board through ratatui's `TestBackend` at
120x24 and 80x24 with a fixed clock (`2026-01-01T12:00:00Z`, every `since` an
exact offset from it) and compares the text **verbatim** with
`tests/golden/*.txt`. `perch::tui::render_to_string(app, w, h, now)` is the only
production helper this needs; it is the same `render` the dashboard runs, drawn
into a buffer instead of a terminal.

What is pinned, and why a `contains` assertion would not do: column widths and
their adaptivity, row order, the group headers and their counts, the subagent
badge, the child rows, the footer, the help overlay's frame, and the fact that
none of it moves when only a colour changes.

| golden | what it holds |
| --- | --- |
| `grouped_120x24.txt` | the default board: needs_input (with a blocked child), done, working, idle, delegating, one ended hidden behind the note |
| `grouped_80x24.txt` | the same board squeezed to 80 columns |
| `flat_120x24.txt` | `v`: flat, newest first, with the project column back |
| `expanded_120x24.txt` | `Space`: the delegating pane's finished children unfolded, newest first |
| `ended_120x24.txt` | `e`: the ended row shown |
| `help_120x24.txt` | the help overlay over the board |
| `empty_80x24.txt` | the empty state |
| `light_120x24.txt` | the light theme (same layout, different colours — the text must not move) |
| `gone_pane_120x24.txt` | the inline `pane %9 is gone` error line |
| `debug_ids_120x24.txt` | `PERCH_DEBUG=1`: pane ids beside the location |
| `e2e_dashboard_120x40.txt` | the real binary, in a real 120x40 tmux pane, read back with `capture-pane` (`tests/e2e_tui_render.rs`) |

**Policy.** Comparison is verbatim, and re-blessing is never a passing run:

```sh
UPDATE_GOLDENS=1 cargo test --test golden      # rewrites the files, then FAILS
git diff tests/golden                          # read the diff — this is the review
cargo test --test golden                       # green only once the new text is right
```

Every golden test writes its file and then panics with "goldens were rewritten",
so a CI job that somehow ran with `UPDATE_GOLDENS=1` still goes red. A changed
golden is a diff to read, not a file to regenerate on the way past.

The real-terminal golden is normalised in exactly one way: the digits of the age
column are masked (`3h` → `Nh`), keeping the token's width so the columns must
still line up. Its `since` values are three and four hours in the past, so the
mask is a belt, not the mechanism. `PERCH_NO_PATH_PROBE=1` keeps the setup
banner out of it, since a harness on the developer's `PATH` is not a fact about
perch.

## Coverage: every documented rule and the test that would fail without it

The rules are the ones stated in `docs/ARCHITECTURE.md` and `docs/DECISIONS.md`
(entries 8–17). Each row names tests that fail if the rule is removed — not
tests that merely execute the code.

### The reducer's event → state table

| rule | test(s) |
| --- | --- |
| `SessionStart` → `idle` | `reducer::session_start_goes_idle` |
| `UserPromptSubmit` → `working` | `reducer::prompt_then_stop` |
| `Stop` → `idle` when the pane is seen, `done` when it is not | `reducer::stop_is_done_when_you_are_elsewhere_and_idle_when_you_are_looking`, `hook_end_to_end::done_means_finished_while_you_were_elsewhere`, `e2e_seen::seen_is_evaluated_from_the_live_client_list` |
| `Stop` records `last_assistant_message`, whitespace-collapsed | `reducer::prompt_then_stop`, `hook_end_to_end::a_prompt_then_stop_leaves_a_done_record` |
| `NeedsInput` only for real dialogs (`permission_prompt`, elicitations, `agent_needs_input`, codex `PermissionRequest`, codex `request_user_input`) | `adapter_claude::needs_input_notifications`, `adapter_codex::permission_request_is_needs_input`, `adapter_codex::a_request_user_input_pre_tool_use_is_needs_input` |
| `idle_prompt` / `auth_success` / `quota_*` are observed only | `adapter_claude::idle_and_informational_notifications_are_only_observed`, `reducer::an_idle_prompt_or_quota_notice_changes_nothing`, `reducer_rules::no_other_transition_makes_a_sound` |
| `agent_completed` → `done` | `adapter_claude::agent_completed_is_done`, `reducer::needs_input_and_completed` |
| `ToolUse` clears `needs_input` and nothing else | `reducer::a_tool_call_clears_a_stale_needs_input_and_nothing_else`, `hook_end_to_end::a_tool_call_unblocks_a_stale_needs_input` |
| A new prompt also clears `needs_input` | `reducer::a_new_prompt_also_clears_needs_input` |
| `SessionEnd` → `ended` | `reducer::session_end_ends`, `adapter_claude::session_end` |
| `since` moves only on a real change | `reducer::unchanged_state_keeps_since`, `reducer_rules::an_unchanged_state_chimes_nothing` |
| `project` is the last component of `cwd` | `reducer::cwd_sets_project`, `tui_render::project_falls_back_to_the_cwd_basename_then_a_stub` |
| `last_message` capped on a char boundary | `reducer::truncate_is_char_safe` |
| State rank orders `needs_input < done < working < starting < idle < ended` | `reducer::ranks_order_needs_input_first`, `store_roundtrip::reconcile_uses_the_injected_pane_list` |
| An unparseable or untracked payload is a no-op, exit 0 | `adapter_*::untracked_events_are_dropped`, `hook_end_to_end::malformed_and_untracked_payloads_still_exit_zero` |

### Questions (decision 18)

| rule | test(s) |
| --- | --- |
| `PreToolUse` for `AskUserQuestion` parses to `Question` with every option and the tool-use id | `adapter_claude::an_ask_user_question_pre_tool_use_is_a_question` |
| `PermissionRequest` for `AskUserQuestion` (auto mode) is the same question with a derived, stable id; the answer comes back in that event's shape | `adapter_claude::a_permission_request_for_ask_user_question_is_the_same_question`, `ask_hook::the_ask_hook_answers_a_permission_request_in_its_own_shape` |
| Codex asks through perch's `ask_user` MCP tool: the JSON-RPC handshake, `tools/list`, and a `tools/call` answered through the popup's file (and told to ask in chat when handed back); the adapter parses the payload; setup registers the server next to the user's own, doctor reports it, uninstall removes it | `mcp::initialize_lists_the_tool_and_ignores_notifications`, `mcp_server::codex_asks_through_the_mcp_tool_and_gets_the_popups_answer`, `adapter_codex::an_ask_user_payload_is_a_question`, `codex_mcp::upsert_keeps_other_servers_and_is_idempotent`, `codex_mcp::a_stale_entry_is_refreshed`, `setup_lifecycle::setup_registers_the_codex_mcp_server_and_uninstall_removes_it` |
| Codex `request_user_input` is `needs_input`, detected only | `adapter_codex::a_request_user_input_pre_tool_use_is_needs_input`, `e2e_ask_flow::pi_questions_pop_up_and_codex_questions_are_flagged` |
| pi's `ask_user` payload is a `Question`; the hook answers in pi's bare shape; the plain pi hook ignores it; the extension registers the tool with a perch-first, pi-dialog-fallback body | `adapter_pi::an_ask_user_payload_is_a_question`, `adapter_pi::the_embedded_extension_registers_an_ask_user_tool_that_asks_through_perch`, `ask_hook::the_ask_hook_answers_a_pi_ask_user_call_in_its_own_shape`, `ask_hook::the_plain_pi_hook_ignores_an_ask_user_payload`, `e2e_ask_flow::pi_questions_pop_up_and_codex_questions_are_flagged` |
| A question is `needs_input`, always chimes, and stays on the record with a deadline | `reducer_rules::a_question_is_needs_input_that_remembers_what_was_asked` |
| Leaving `needs_input` by any path drops the question | `reducer_rules::leaving_needs_input_by_any_path_drops_the_question` |
| An answer puts the pane to `working` silently | `reducer_rules::an_answer_puts_the_pane_back_to_work_and_is_silent`, `ask_hook::the_ask_hook_blocks_until_the_answer_file_and_prints_the_decision` |
| The plain hook ignores the payload | `ask_hook::the_plain_hook_ignores_an_ask_user_question_pre_tool_use` |
| The `--ask` hook blocks, then prints `allow` + `updatedInput.answers` (question text → label, multi joined by `, `) | `ask_hook::the_ask_hook_blocks_until_the_answer_file_and_prints_the_decision`, `e2e_ask::the_question_pops_up_on_the_client_and_is_answered_there` |
| A defer, a `number` question, a timeout and `[ask] enabled = false` all print nothing | `ask_hook::a_defer_releases_the_hook_with_no_decision`, `ask_hook::a_number_question_is_left_to_the_native_dialog`, `ask_hook::the_ask_hook_times_out_to_the_native_dialog`, `ask_hook::a_disabled_ask_is_a_no_op` |
| Where you look plays no part: the hook holds the question and pops up over a seen pane; `perch seen` leaves questions alone | `ask_hook::the_ask_hook_holds_the_question_even_when_the_pane_is_seen`, `ask_hook::argument_less_seen_leaves_a_live_question_alone`, `e2e_ask_flow::a_question_you_are_looking_at_pops_up_over_the_pane` |
| An answer for another tool use is dropped | `ask_hook::an_answer_for_another_tool_use_is_ignored_and_the_hook_keeps_waiting` |
| A set handed back once is not taken again by the following event for the same questions; a different set is, and it pops up even on a pane already `needs_input` | `ask_hook::a_deferred_question_is_not_taken_again_by_the_following_permission_request` |
| Every `--ask` invocation is traced to `ask.log` | `ask_hook::every_ask_invocation_is_traced` |
| The installer adds both matched `--ask` groups, longer than the hook's wait, to old installs too | `ask_hook::install_claude_adds_the_ask_group_with_a_long_timeout`, `ask_hook::install_claude_adds_the_permission_request_ask_group_too`, `install::merge_appends_and_preserves_existing` |
| A question pops up as the form on every client, never as the card; an unanswerable one gets the card | `ask_hook::a_question_pops_up_as_a_form_not_a_card`, `ask_hook::a_deferred_question_gets_the_card_not_the_form`, `e2e_ask_flow::two_clients_each_get_the_popup_and_answering_on_one_closes_the_other` |
| The popup is one centred `display-popup` per client, sized to its content and capped by the client; long questions and descriptions wrap, nothing is cut | `ask_hook::the_ask_popup_is_a_centered_form_per_client_sized_to_the_question`, `ask_form::the_popup_is_sized_to_its_content_and_capped_by_the_client` |
| Single pick, multi tick, `Other…` text, unanswered omitted | `ask_form::the_form_collects_a_single_and_a_multi_answer`, `ask_form::other_takes_free_text` |
| One tab per question, answered ones ticked; `←`/`→`, `Tab`, `h`/`l` move between them keeping answers, with edges | `ask_form::tabs_show_every_question_and_move_back_and_forth_keeping_answers`, `ask_form::tab_keys_have_edges_and_do_not_leak_into_the_text_line` |
| Form keys never reach the board | `ask_form::form_keys_never_reach_the_board` |
| `Esc` hands back in place, `p` hands back and jumps; `Esc` in the text line goes back to the options | `ask_form::esc_hands_back_in_place_and_p_hands_back_and_jumps`, `ask_form::esc_in_the_text_line_returns_to_the_options`, `e2e_ask::esc_hands_the_question_to_claude_and_enter_just_jumps`, `e2e_ask_flow::esc_hands_the_set_to_claude_and_nothing_brings_it_back`, `e2e_ask_flow::p_goes_to_the_pane_and_the_next_question_returns_when_you_look_away` |
| The form survives a refresh and closes when the question is gone | `ask_form::the_form_survives_a_refresh_and_closes_when_the_question_is_gone`, `e2e_ask::the_popup_body_draws_the_form_and_closes_when_the_question_is_answered_elsewhere` |
| The title names project (branch), harness and window, and the queue length | `ask_form::the_form_title_names_the_project_agent_and_pane_and_the_queue_behind_it`, `e2e_ask_flow::the_popup_names_its_owner_and_counts_the_queue_then_moves_on` |
| The board shows a question as `⚑ question` with its text, offers no answer key, and `Enter` jumps | `ask_form::the_board_marks_a_question_and_offers_no_answer_key`, `e2e_ask::esc_hands_the_question_to_claude_and_enter_just_jumps` |
| The queue: oldest live, unhandled set next, in the same popup | `ask_hook::the_next_pending_question_is_the_oldest_unhandled_one`, `e2e_ask::simultaneous_questions_queue_into_one_popup`, `e2e_ask_flow::the_popup_names_its_owner_and_counts_the_queue_then_moves_on` |
| One popup per client: exclusive claim while its pid lives; `ask-popup` skips a claimed client | `ask_hook::a_client_popup_claim_is_exclusive_while_its_owner_lives`, `ask_hook::ask_popup_skips_a_client_that_already_has_a_popup`, `e2e_ask::simultaneous_questions_queue_into_one_popup` |
| `perch seen` reopens a live question that has no popup; never on a client dealing with another dialog | `ask_hook::argument_less_seen_reopens_the_popup_for_a_waiting_question`, `ask_hook::ask_popup_never_covers_a_client_dealing_with_another_dialog`, `e2e_ask_flow::p_goes_to_the_pane_and_the_next_question_returns_when_you_look_away` |
| Regressions from the first live cuts: the Codex tool is read-only and registered `approve`, an `auto` entry is refreshed; the pi tool asks for one call per set; a 0.3 install is brought to 0.4 by one setup run | `mcp_server::the_tool_is_declared_read_only_and_registered_as_never_gated`, `adapter_pi::the_pi_tool_asks_for_one_call_per_set_and_numbers_the_fallback`, `install_codex_pi::a_zero_three_install_is_brought_to_zero_four_by_one_setup_run` |
| A real Claude question is answered through the popup, alone and queued | `e2e_ask_real::a_real_claude_question_is_answered_through_the_popup`, `e2e_ask_real::two_real_claude_questions_queue_into_one_popup` |

### Seen

| rule | test(s) |
| --- | --- |
| A pane is seen when a *focused* client is showing it | `tmux::a_pane_is_seen_when_a_focused_client_is_showing_it`, `store_roundtrip::snapshot_marks_done_panes_a_focused_client_is_showing_as_idle` |
| With no focus information anywhere, any viewer counts | `tmux::a_pane_is_seen_when_a_focused_client_is_showing_it`, `hook_end_to_end::with_no_focus_information_any_viewer_counts_as_seen` |
| The reconciliation flips `done` → `idle` only, and writes it through | `store_roundtrip::snapshot_marks_done_panes_a_focused_client_is_showing_as_idle`, `e2e_seen::seen_is_evaluated_from_the_live_client_list` |
| `since` resets on that flip | `store_roundtrip::snapshot_marks_done_panes_a_focused_client_is_showing_as_idle` |
| `perch seen` with no argument reconciles every `done` record | `hook_end_to_end::argument_less_seen_reconciles_every_done_pane_a_focused_client_shows`, `e2e_seen::the_tmux_hooks_make_seen_immediate` |
| `perch seen <pane>` marks one pane unconditionally, and is a no-op on a pane that is not `done` | `hook_end_to_end::done_means_finished_while_you_were_elsewhere`, `e2e_navigation::enter_moves_the_client_to_the_selected_pane_and_marks_it_seen` |
| The installed tmux hooks make it immediate | `e2e_seen::the_tmux_hooks_make_seen_immediate` |

### Subagents

| rule | test(s) |
| --- | --- |
| A fresh `SubagentStart` upserts a working child with its `agent_type` | `reducer::start_then_stop_tracks_one_child`, `adapter_claude::subagent_events_carry_the_agent_id` |
| A `SendMessage` `PreToolUse` is a resume; the ` [ref]` suffix is dropped, a name (`main`) is kept verbatim, an empty target is not a resume | `adapter_claude::a_send_message_pre_tool_use_is_a_resume` |
| A resume restarts the clock, keeping `agent_type` and message | `reducer::a_resume_of_a_finished_child_puts_it_back_to_work`, `reducer::a_resume_of_an_unknown_id_creates_a_working_child` |
| A resumed child keeps the pane delegating until its stop | `reducer::a_resumed_child_keeps_the_pane_delegating_until_its_stop` |
| A helper stop — unknown id, empty `agent_type` — is dropped | `reducer::a_helper_stop_for_an_unknown_id_is_ignored`, `adapter_claude::a_helper_stop_has_no_agent_type` |
| A stop for a known id, or an unknown id with an `agent_type`, records a finished child | `reducer::start_then_stop_tracks_one_child` |
| `effective_state` is `working` (**delegating**) for a done/idle pane with running children | `store_roundtrip::reconcile_keeps_running_children_of_an_idle_or_done_pane`, `tui_render::a_delegating_pane_reads_and_sorts_as_working_and_is_never_next` |
| Delegating drives the state cell, the sort, `next`, and `@perch_state` | `tui_render::a_delegating_pane_reads_and_sorts_as_working_and_is_never_next`, `hook_end_to_end::a_stop_with_running_subagents_asks_for_no_card_and_reads_as_working`, `golden::grouped_board_120x24` |
| No chime and no card while delegating | `reducer::a_parent_stop_leaves_running_children_alone_and_stays_silent`, `hook_end_to_end::a_stop_with_running_subagents_asks_for_no_card_and_reads_as_working` |
| The chime comes on the parent's next `Stop`, after the last child | `hook_end_to_end::a_stop_with_running_subagents_asks_for_no_card_and_reads_as_working` |
| A `needs_input` child chimes, whatever the others are doing, and is never folded away | `reducer::a_notification_blocks_the_child_and_sounds`, `reducer::a_parent_needs_input_still_chimes_while_delegating`, `tui_render::a_needs_input_child_is_never_folded_away` |
| A pane reaching `idle` (any path) clears its finished children | `reducer::reaching_idle_by_any_path_clears_finished_children`, `reducer::a_new_turn_clears_done_children_but_keeps_running_ones`, `store_roundtrip::reconcile_keeps_running_children_of_an_idle_or_done_pane` |
| At most twenty children, oldest finished dropped first | `reducer::children_are_capped_at_twenty_oldest_finished_first` |
| Finished children expire after ten minutes | `reducer::finished_children_are_pruned_after_ten_minutes` |
| A child running for over two hours is retired | `reducer::a_child_running_for_two_hours_is_retired`, `store_roundtrip::reconcile_keeps_running_children_of_an_idle_or_done_pane` |
| `SessionEnd` retires running children | `reducer::session_end_retires_every_running_child` |
| An `ended` pane runs nothing | `store_roundtrip::reconcile_keeps_running_children_of_an_idle_or_done_pane` |
| Subagent events never move the parent | `hook_end_to_end::subagent_events_become_children_of_the_parent_pane`, `hook_end_to_end::codex_subagents_do_not_move_the_parent` |

### Sound

| rule | test(s) |
| --- | --- |
| `done` and `needs_input` chime; nothing else does | `reducer_rules::a_stop_you_did_not_watch_chimes_done_and_one_you_watched_is_silent`, `reducer_rules::a_needs_input_chimes_its_own_sound`, `reducer_rules::a_completed_notification_chimes_done`, `reducer_rules::no_other_transition_makes_a_sound` |
| Per-pane cooldown | `sound_cue::a_second_chime_inside_the_cooldown_is_suppressed` |
| The mute file silences everything | `sound_cue::the_mute_file_silences_the_hook` |
| `PERCH_NO_SOUND=1` silences everything | `sound_cue::perch_no_sound_silences_the_hook` |
| `done` / `needs_input` / `error` are the configurable keys | `sound_cue::only_done_needs_input_and_error_have_a_sound`, `sound::named_sound_resolves_under_system_sounds` |

### Store and liveness

| rule | test(s) |
| --- | --- |
| Records are written tmp-then-rename, leaving no temp file | `store_roundtrip::save_is_atomic_and_leaves_no_temp_files`, `store_roundtrip::save_load_round_trip` |
| `events.jsonl` is append-only | `store_roundtrip::events_are_appended_as_jsonl` |
| A record whose pane is gone becomes `ended`; one `ended` over an hour is deleted | `store_roundtrip::reconcile_uses_the_injected_pane_list`, `store_roundtrip::snapshot_persists_the_reconciliation` |
| The reconciliation is persisted, not merely reported | `store_roundtrip::snapshot_persists_the_reconciliation` |
| An unknown-field or missing-field record still loads | `store_roundtrip::save_load_round_trip` |
| Ages format as s/m/h | `store_roundtrip::age_formatting_and_parsing`, `store::fmt_age_units` |
| A location perch cannot resolve falls back to the stored one, then `—` | `tui_render::an_ended_pane_shows_its_last_known_location`, `tui_render::a_pane_with_no_location_at_all_renders_an_em_dash` |

### Navigation

| rule | test(s) |
| --- | --- |
| Every move is one `switch-client -c <client> -t <pane>` | `tmux::the_only_jump_primitive_is_switch_client_with_an_explicit_client`, `tui_render::enter_switches_the_named_client_to_the_pane_id`, `e2e_navigation::enter_moves_the_client_to_the_selected_pane_and_marks_it_seen` |
| The client is always explicit; a missing `--client` warns and guesses | `tmux::an_explicit_client_wins_without_asking_tmux` |
| A jump to a gone pane shows `pane %N is gone` and does not exit | `tmux::a_failed_jump_is_reported`, `tui_render::a_gone_pane_renders_an_error_line`, `e2e_navigation::a_pane_that_died_before_enter_is_reported_gone_and_moves_nobody`, `golden::a_gone_pane_error_line_120x24` |
| The pane is confirmed present before the jump | `tmux::pane_existence_comes_from_the_live_list` |
| Selection is keyed by pane, and survives a reordering refresh | `tui_render::the_selection_survives_a_reordering_refresh`, `tui_render::a_vanished_pane_moves_the_cursor_to_the_nearest_row` |
| A child row resolves to its parent pane | `tui_render::subagent_rows_render_and_enter_resolves_to_the_parent_pane` |
| `next` is the oldest `needs_input`, else the oldest `done`, by `since`; delegating panes are skipped | `tui_render::next_waiting_is_the_oldest_needs_input_then_the_oldest_done`, `tui_render::a_delegating_pane_reads_and_sorts_as_working_and_is_never_next`, `e2e_navigation::next_lands_on_the_oldest_waiting_pane_across_sessions` |
| `gg` / `G` / `v` / `e` / `Space`, and a lone `g` does nothing | `tui_render::gg_and_shift_g_go_to_the_ends`, `tui_render::gg_goes_to_the_top_and_a_lone_g_does_nothing`, `tui_render::v_toggles_grouped_and_flat`, `tui_render::ended_rows_are_hidden_until_e`, `tui_render::finished_subagents_fold_into_the_badge_and_space_unfolds_them`, `golden::flat_board_120x24`, `golden::ended_rows_appear_with_e`, `golden::finished_children_unfold_with_space` |
| The cursor skips headers and notes | `tui_render::navigation_skips_headers_and_notes` |

### Location column

| rule | test(s) |
| --- | --- |
| Rows show the window name, never a pane id | `tui_render::rows_show_the_window_name_and_never_a_pane_id`, `e2e_tui_render::the_tui_renders_the_seeded_rows_in_a_real_pane` |
| `<session>/` only when the live panes span more than one session | `tui_render::the_session_prefix_appears_only_with_more_than_one_session`, `tmux::a_location_names_the_window_and_only_disambiguates_when_it_must` |
| `.<pane_index>` only when the window is split | `tui_render::the_pane_index_appears_only_in_a_split_window` |
| A window name may contain spaces and non-ASCII | `tmux::a_window_name_may_contain_spaces`, `tui_render::a_unicode_window_name_with_spaces_renders` |
| The column is adaptive and capped, with an ellipsis | `tui_render::the_location_column_is_adaptive_and_capped` |
| Grouped rows drop the project column; the header carries project and branch | `tui_render::grouped_rows_drop_the_project_column`, `tui_render::a_group_header_carries_the_branch_when_the_group_agrees`, `golden::grouped_board_120x24`, `golden::flat_board_120x24` |
| `PERCH_DEBUG=1` shows the pane id | `tui_render::perch_debug_appends_the_pane_id`, `golden::perch_debug_shows_pane_ids_120x24` |

### Installers, trust and doctor

| rule | test(s) |
| --- | --- |
| Claude merge appends, preserves and is idempotent, with a backup | `install::merge_appends_and_preserves_existing`, `install::merge_is_idempotent`, `hook_end_to_end::install_claude_writes_a_backup_and_is_idempotent`, `hook_end_to_end::install_claude_dry_run_writes_nothing_and_merges_correctly` |
| Only `SessionStart` carries a matcher | `install::only_session_start_gets_a_matcher` |
| Codex `SessionEnd` hook installed within the 3 s clamp; a re-run corrects an older install's timeout in place | `install_codex_pi::codex_session_end_hook_is_installed_within_the_clamp_and_older_installs_are_corrected` |
| Codex merge preserves every existing entry in order, backs up once, is idempotent | `install_codex_pi::codex_merge_preserves_every_existing_entry_in_order`, `install_codex_pi::codex_install_backs_up_once_and_is_idempotent`, `install_codex_pi::codex_install_creates_the_file_when_there_is_none` |
| pi extension is written, replaced after a backup, idempotent | `install_codex_pi::pi_install_writes_the_extension_and_is_idempotent`, `install_codex_pi::pi_install_replaces_a_stale_copy_after_backing_it_up`, `adapter_pi::the_embedded_extension_spawns_the_pi_hook` |
| `--dry-run` / `--print` write nothing | `install_codex_pi::codex_dry_run_and_print_write_nothing`, `install_codex_pi::pi_dry_run_and_print_write_nothing`, `setup_lifecycle::setup_dry_run_writes_nothing` |
| Codex trust hashes match codex's own recipe, matcher-sensitive | `trust::hashes_match_codex`, `trust::a_missing_matcher_hashes_differently_from_an_empty_one`, `trust::labels_are_snake_case` |
| Trust keys carry real indices and only perch handlers, and refresh after a reorder | `trust::entries_use_real_indices_and_only_perch_handlers`, `setup_lifecycle::reordering_hooks_json_refreshes_the_indices` |
| Trust writes keep foreign keys and formatting; uninstall removes only ours | `trust::upsert_keeps_foreign_keys_and_formatting`, `trust::remove_takes_only_the_named_keys`, `setup_lifecycle::setup_writes_codex_trust_records_and_uninstall_removes_them` |
| The tmux snippet binds through `run-shell` with an explicit client, sources cleanly and twice | `install::tmux_snippet_binds_through_run_shell_with_an_explicit_client`, `hook_end_to_end::install_tmux_writes_the_popup_binding_and_prints_the_source_line`, `e2e_setup::setup_wires_a_fake_home_and_a_real_tmux_server` |
| `setup` wires what is present, skips what is absent, is idempotent | `setup_lifecycle::setup_wires_present_harnesses_skips_absent_and_is_idempotent`, `setup_lifecycle::setup_only_and_no_tmux_restrict_the_run` |
| `uninstall` restores the originals and leaves other lines alone | `setup_lifecycle::uninstall_restores_the_originals_and_leaves_other_lines`, `setup_lifecycle::uninstall_keep_state_and_dry_run`, `setup::strip_keeps_other_entries_and_user_arrays` |
| `doctor` exits non-zero while unwired and zero once wired | `setup_lifecycle::doctor_reports_unwired_then_wired`, `e2e_setup::setup_wires_a_fake_home_and_a_real_tmux_server` |
| The unwired nudge goes to stderr only | `setup_lifecycle::list_and_status_nudge_on_stderr_only`, `tui_render::banner_shows_only_when_a_harness_is_unwired` |
| A partial or legacy config still loads with defaults | `config::partial_toml_keeps_defaults`, `config::a_pre_sound_only_config_still_loads`, `config::defaults_match_the_plan` |
| `install.sh` is strict, shellcheck-clean and runs setup | `install_script::*` |

### The dashboard as a picture

| rule | test(s) |
| --- | --- |
| Every state renders its glyph and word | `tui_render::every_state_renders_its_glyph_and_name`, `golden::grouped_board_120x24`, `golden::ended_rows_appear_with_e` |
| The empty state says what to do | `tui_render::empty_store_renders_without_panicking`, `golden::empty_state_80x24` |
| The footer is one line | `tui_render::the_footer_is_one_line_of_essentials_and_a_status`, `e2e_tui_render::the_tui_renders_the_seeded_rows_in_a_real_pane` |
| The help overlay lists every key and the legend | `tui_render::the_help_overlay_shows_every_key_and_the_state_legend`, `golden::help_overlay_120x24` |
| The light theme changes colours, not layout | `tui_render::light_theme_renders_without_panicking`, `golden::light_theme_120x24` |
| 80 and 40 columns truncate without panicking or wrapping | `tui_render::narrow_terminals_truncate_instead_of_panicking`, `golden::grouped_board_80x24` |
| A message with newlines, tabs or ANSI escapes is one flat line | `tui_render::control_characters_in_a_message_are_flattened`, `tui_render::long_message_is_one_truncated_line` |
| A very long project name is truncated, not wrapped | `tui_render::a_very_long_project_name_is_truncated` |
| The real binary draws all of it in a real pane | `e2e_tui_render::the_tui_renders_the_seeded_rows_in_a_real_pane`, `e2e_tui_render::the_real_pane_render_matches_the_golden` |
