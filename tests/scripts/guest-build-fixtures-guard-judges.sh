#!/usr/bin/env bash
# A guard that fails on a busy host teaches people to rerun it, and one that
# waits longer may stop failing on a real fault. This runs
# guest-build-fixtures-cleans-up.sh over a slowed host, which must pass, and
# over broken copies of the supervisor, which must fail by name.
#
#   bash tests/scripts/guest-build-fixtures-guard-judges.sh
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
guard="$repo/tests/scripts/guest-build-fixtures-cleans-up.sh"
supervisor="$repo/scripts/guest-build-fixtures.sh"
python3 "$repo/tests/scripts/support/supervisor_guard.py" || exit 1

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d)"
trap 'rm -rf -- "$sandbox"' EXIT

# break NAME OLD NEW [OLD NEW]...: a copy of the supervisor with each OLD
# replaced. An OLD that is gone fails by name: the mutation would test nothing.
break_supervisor() {
    python3 - "$supervisor" "$sandbox/$1.sh" "${@:2}" <<'PY' ||
import sys
source, dest, pairs = sys.argv[1], sys.argv[2], sys.argv[3:]
assert pairs and len(pairs) % 2 == 0, f'old and new do not pair up: {pairs}'
text = open(source).read()
for old, new in zip(pairs[::2], pairs[1::2]):
    assert old in text, f'the supervisor no longer holds: {old}'
    text = text.replace(old, new)
open(dest, 'w').write(text)
PY
        fail "$1: the mutation could not be built"
}

# expect_failure NAME CASE TEXT...: the guard fails on that broken supervisor,
# and what it says holds each TEXT.
expect_failure() {
    local name="$1" case="$2" output status=0 text
    shift 2
    output="$(FIXTURE_GUARD_SUPERVISOR="$sandbox/$name.sh" FIXTURE_GUARD_CASES="$case" bash "$guard" 2>&1)" ||
        status=$?
    if [ "$status" = 0 ]; then
        fail "$name: the guard passed"
        return
    fi
    for text in "$@"; do
        case "$output" in
            *"$text"*) ;;
            *) fail "$name: the guard's failure does not say: $text"
               printf '%s\n' "$output" | tail -n 20 >&2 ;;
        esac
    done
}

# Every mutation is built before any case runs, so a stale anchor is named at once.
# The supervisor stops before any tool runs: nothing ever becomes ready.
break_supervisor stuck-before-ready \
    "trap 'exit 143' TERM" "trap 'exit 143' TERM; exec /bin/sleep 100000"
# The supervisor takes its signal and then stops inside its cleanup.
break_supervisor stuck-in-cleanup \
    'local status=$? unmounted=yes' 'local status=$? unmounted=yes; exec /bin/sleep 100000'
# Cleanup forgets the tool it owns.
break_supervisor leaks-tool \
    'tool_pid=$!' 'forgotten=$!' \
    'wait "$tool_pid" || status=$?' 'wait "$forgotten" || status=$?'
# Cleanup stops the tool but not the tool's session.
break_supervisor leaks-descendant \
    'kill -TERM -- "-$tool_pid" "$tool_pid" 2>/dev/null || true' 'kill -TERM "$tool_pid" 2>/dev/null || true' \
    'kill -KILL -- "-$tool_pid" "$tool_pid" 2>/dev/null || true' 'kill -KILL "$tool_pid" 2>/dev/null || true'
[ "$fails" = 0 ] || exit 1

# 0.15 s per stand-in is more than 15 s across the 124 tools that run before
# the first mount is ready, as a host at a load of about 57 was measured to be.
output="$(FIXTURE_TOOL_DELAY=0.15 FIXTURE_GUARD_CASES=SIGTERM-mount-hold bash "$guard" 2>&1)" ||
    { fail "a slow host failed the guard"; printf '%s\n' "$output" | tail -n 20 >&2; }

# Started in the background, or under nohup, the guard inherits a signal ignored,
# and a shell cannot trap a signal ignored on entry. That is the host, not the
# supervisor, so the guard hands its supervisors the signals it sends them.
output="$( (trap '' HUP INT; FIXTURE_GUARD_CASES=SIGHUP-format-hold,SIGINT-format-hold bash "$guard") 2>&1)" ||
    { fail "a guard started with its signals ignored failed"; printf '%s\n' "$output" | tail -n 20 >&2; }

FIXTURE_GUARD_IDLE_LIMIT=3 expect_failure stuck-before-ready SIGTERM-format-hold \
    'SIGTERM-format-hold' 'ready' 'no progress for'

FIXTURE_GUARD_IDLE_LIMIT=3 expect_failure stuck-in-cleanup SIGTERM-format-hold \
    'SIGTERM-format-hold' 'exit with status 143' 'no progress for'

expect_failure leaks-tool SIGTERM-format-hold 'owned child survived cleanup'

FIXTURE_GUARD_REAP_LIMIT=3 expect_failure leaks-descendant SIGTERM-format-descendant-hold 'descendant survived cleanup'

# A signal can land just as a tool exits: bash collects the status and jumps
# out of its wait before recording it, so it believes the tool still runs. A
# second wait then blocks on every other child it has, the FUSE daemon among
# them, for ever. No run here can land there on demand, so this reads the text.
cleanup_body="$(sed -n '/^cleanup() {/,/^}/p' "$supervisor")"
case "$cleanup_body" in
    '') fail "the supervisor has no cleanup function to read" ;;
    *'wait "$tool_pid"'*) fail 'cleanup waits for the tool, which can block for ever: wait "$tool_pid"' ;;
esac

[ "$fails" = 0 ] || exit 1
echo "PASS  the fixture cleanup guard passes on a slow host, and names a stuck supervisor and a leak"
