#!/usr/bin/env sh
# Run perch's real-tmux end-to-end tests.
#
# Installs nothing. Needs `tmux` on PATH; without it every e2e test skips with
# a printed reason instead of failing.
#
#   scripts/e2e.sh                 # the hermetic suite
#   PERCH_E2E_REAL=1 scripts/e2e.sh  # also the real-harness test (tokens, network)
#
# Every test brings up its own private tmux server on a unique socket and kills
# it on the way out, so this never touches the tmux server you are sitting in.
set -eu

cd "$(dirname "$0")/.."

if ! command -v tmux >/dev/null 2>&1; then
  echo "scripts/e2e.sh: tmux is not on PATH — the tests will skip, not fail" >&2
fi

exec cargo test --test 'e2e_*' -- --nocapture "$@"
