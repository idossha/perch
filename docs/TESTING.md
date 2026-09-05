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

`tests/e2e_real_harness.rs` runs `codex exec …` / `claude -p …` for real.

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
