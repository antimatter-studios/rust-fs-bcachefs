#!/usr/bin/env bash
# The fixture selector's controls, in the scripts tier the required unit job
# runs (#432). They once ran only in differential.yml, which no required check
# reads, so a broken selector failed nothing that gates a merge. Their own last
# line is required as well as their status, so controls that stop early with
# status 0 fail too.
#
#   bash tests/scripts/select-fixture-run.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONTROLS="select-fixture-run-test.sh"

fail() { echo "FAIL  $*" >&2; exit 1; }

output="$(bash "$REPO/scripts/$CONTROLS" 2>&1)"
status=$?
printf '%s\n' "$output"

[ "$status" -eq 0 ] || fail "$CONTROLS exited $status"
[ "$(printf '%s\n' "$output" | tail -n 1)" = "$CONTROLS: all checks passed" ] ||
    fail "$CONTROLS stopped before its last line"

echo "select-fixture-run.sh: all checks passed"
