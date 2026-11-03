#!/usr/bin/env bash
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
guard="$repo/tests/scripts/guest-write-study-cleans-up.sh"
supervisor="$repo/scripts/guest-write-study.sh"
python3 "$repo/tests/scripts/support/supervisor_guard.py" || exit 1
fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

cleanup_body="$(sed -n '/^cleanup() {/,/^}/p' "$supervisor")"
case "$cleanup_body" in
    '') fail 'the supervisor has no cleanup function to read' ;;
    *'wait "$tool_pid"'*) fail 'cleanup waits for the tool, which can block for ever: wait "$tool_pid"' ;;
esac
[ "$fails" = 0 ] || exit 1

sandbox="$(mktemp -d)"
trap 'rm -r -- "$sandbox"' EXIT

# A mutation that could not be built is one no case exercises: it fails by name.
break_supervisor() {
    python3 - "$supervisor" "$sandbox/$1.sh" "${@:2}" <<'PY' ||
import sys
source, destination, pairs = sys.argv[1], sys.argv[2], sys.argv[3:]
assert pairs and len(pairs) % 2 == 0, f'old and new do not pair up: {pairs}'
text = open(source).read()
for old, new in zip(pairs[::2], pairs[1::2]):
    assert old in text, f'the supervisor no longer holds: {old}'
    text = text.replace(old, new)
open(destination, 'w').write(text)
PY
        fail "$1: the mutation could not be built"
}

expect_failure() {
    local name="$1" case="$2" output status=0 text
    shift 2
    output="$(STUDY_GUARD_SUPERVISOR="$sandbox/$name.sh" STUDY_GUARD_CASES="$case" bash "$guard" 2>&1)" ||
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
break_supervisor stuck-before-ready \
    "trap 'exit 143' TERM" "trap 'exit 143' TERM; exec /bin/sleep 100000"
break_supervisor stuck-in-cleanup \
    'local status=$?' 'local status=$?; exec /bin/sleep 100000'
break_supervisor leaks-tool \
    'tool_pid=$!' 'forgotten=$!' \
    'wait "$tool_pid" || status=$?' 'wait "$forgotten" || status=$?'
break_supervisor leaks-descendant \
    'kill -KILL -- "-$tool_pid" "$tool_pid" 2>/dev/null || true' 'kill -KILL "$tool_pid" 2>/dev/null || true'
[ "$fails" = 0 ] || exit 1

output="$(STUDY_TOOL_DELAY=0.5 STUDY_GUARD_CASES=SIGTERM-copy-hold bash "$guard" 2>&1)" ||
    { fail 'a slow host failed the guard'; printf '%s\n' "$output" | tail -n 20 >&2; }

output="$( (trap '' HUP INT; STUDY_GUARD_CASES=SIGHUP-hold,SIGINT-hold bash "$guard") 2>&1)" ||
    { fail 'a guard started with its signals ignored failed'; printf '%s\n' "$output" | tail -n 20 >&2; }

STUDY_GUARD_IDLE_LIMIT=3 expect_failure stuck-before-ready SIGTERM-hold \
    'SIGTERM-hold' 'ready' 'no progress for'

STUDY_GUARD_IDLE_LIMIT=3 expect_failure stuck-in-cleanup SIGTERM-hold \
    'SIGTERM-hold' 'exit with status 143' 'no progress for'

expect_failure leaks-tool SIGTERM-hold 'owned child survived cleanup'

STUDY_GUARD_REAP_LIMIT=3 expect_failure leaks-descendant SIGTERM-descendant-hold \
    'descendant survived cleanup'

[ "$fails" = 0 ] || exit 1
echo 'PASS  the write-study cleanup guard passes on a slow host, and names a stuck supervisor and a leak'
