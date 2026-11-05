#!/usr/bin/env bash
# A fixture step that outlives its time ends the build. On a guest of its own that
# is done by powering the guest off. On a guest that other suites share (the harness
# sets FLTH_RUN_ROOT in each run) it must stop only this run: powering off would
# end every suite in the guest (fs-linux-test-harness#157).
set -uo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d "${TMPDIR:-/tmp}/guest-watchdog.XXXXXX")"
trap 'rm -rf "$sandbox"' EXIT
# What the watchdog would write to the kernel and read from it, here.
export GUEST_WATCHDOG_SYSRQ="$sandbox/sysrq-trigger" GUEST_WATCHDOG_SYSRQ_ENABLE="$sandbox/sysrq"
export GUEST_WATCHDOG_STEP="$sandbox/step" GUEST_WATCHDOG_POLL=1
cat > "$sandbox/build.sh" <<'BUILD'
set -uo pipefail
source "$1"
echo $$ > "$2/build.pid"
step hangs 1
sleep 120 &
echo $! > "$2/child.pid"
wait
echo "reached the end" > "$2/end"
BUILD

# The shell's own notice of a job a signal ended is not this test's output.
run_build() { start_build "$@" 2>/dev/null; }
start_build() {
    # start_build [env...]: leaves STATUS and ERR
    ( exec env "$@" bash "$sandbox/build.sh" "$repo/scripts/guest-watchdog.sh" "$sandbox" ) \
        > /dev/null 2> "$sandbox/err" &
    local pid=$! n=0
    while kill -0 "$pid" 2>/dev/null && [ "$n" -lt 300 ]; do sleep 0.1; n=$((n + 1)); done
    if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null
        { wait "$pid"; } 2>/dev/null
        STATUS=running
    else
        { wait "$pid"; } 2>/dev/null
        STATUS=$?
    fi
    ERR="$(cat "$sandbox/err")"
}
alive() { kill -0 "$(cat "$1" 2>/dev/null)" 2>/dev/null; }

# A run on a shared guest.
rm -f "$sandbox"/sysrq* "$sandbox"/*.pid "$sandbox/end"
run_build FLTH_RUN_ROOT=/var/lib/fs-linux-test-harness/runs/abc
if [ "$STATUS" != running ] && [ "$STATUS" != 0 ]; then ok "on a shared guest a stuck step ends the build ($STATUS)"; else fail "on a shared guest a stuck step ends the build (status $STATUS)"; fi
if [ ! -e "$sandbox/sysrq-trigger" ]; then ok "and does not power the guest off"; else fail "and does not power the guest off (it wrote: $(cat "$sandbox/sysrq-trigger"))"; fi
if ! alive "$sandbox/child.pid"; then ok "and what the build started has gone"; else fail "and what the build started has gone"; fi
if [ ! -e "$sandbox/end" ]; then ok "and the build did not go on past it"; else fail "and the build did not go on past it"; fi
case "$ERR" in *"step 'hangs'"*) ok "it names the step" ;; *) fail "it names the step (got: $ERR)" ;; esac
case "$ERR" in *"only this run"*) ok "and says it stopped only this run" ;; *) fail "and says it stopped only this run (got: $ERR)" ;; esac

# A guest of its own, as before.
kill -KILL "$(cat "$sandbox/child.pid" 2>/dev/null)" 2>/dev/null
rm -f "$sandbox"/sysrq* "$sandbox"/*.pid "$sandbox/end"
run_build -u FLTH_RUN_ROOT
if [ "$(cat "$sandbox/sysrq-trigger" 2>/dev/null)" = o ]; then ok "on a guest of its own it powers the guest off"; else fail "on a guest of its own it powers the guest off (wrote '$(cat "$sandbox/sysrq-trigger" 2>/dev/null)')"; fi
kill -KILL "$(cat "$sandbox/child.pid" 2>/dev/null)" "$(cat "$sandbox/build.pid" 2>/dev/null)" 2>/dev/null

if [ "$fails" -gt 0 ]; then
    echo "guest-watchdog-stops-only-its-run.sh: $fails check(s) failed" >&2
    exit 1
fi
echo "guest-watchdog-stops-only-its-run.sh: all checks passed"
