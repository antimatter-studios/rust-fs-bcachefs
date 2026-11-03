#!/usr/bin/env bash
# The lifecycle guards run on a host with no setsid(1) and no timeout(1), as
# macOS is (#411). PATH here is a directory of links to every tool this host
# has except those two, so Linux CI shows what a Mac would: a guard then has
# only tests/scripts/support to find them in. Here the support setsid must
# start a real session and the kernel-oracle guard must pass on the support
# timeout. The two guards that use setsid take minutes on a Mac, so they are
# not run a second time here: they run on macOS itself, in the test-darwin job,
# and each asserts that its tools lead their own sessions.
#
#   bash tests/scripts/host-tools-are-not-assumed.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d)"
trap 'rm -rf -- "$sandbox"' EXIT
farm="$sandbox/bin"
bash_path="$(command -v bash)"

python3 - "$farm" <<'PY'
import os
import sys

farm = sys.argv[1]
os.mkdir(farm)
for directory in os.environ['PATH'].split(os.pathsep):
    try:
        names = os.listdir(directory)
    except OSError:
        continue
    for name in names:
        link = os.path.join(farm, name)
        path = os.path.join(directory, name)
        if name in ('setsid', 'timeout') or os.path.lexists(link):
            continue
        if os.path.isfile(path) and os.access(path, os.X_OK):
            os.symlink(path, link)
PY

for tool in setsid timeout; do
    [ -z "$(PATH="$farm" command -v "$tool")" ] || fail "the sandboxed PATH still finds $tool"
done

# The support setsid: its command leads a new process group, and a descendant
# that ignores TERM goes when that group is killed. No pass-through does this.
(
    PATH="$farm:$REPO/tests/scripts/support"
    # shellcheck disable=SC2016 # expanded by the inner shell, from its arguments
    exec setsid -- sh -c 'trap "" TERM; sleep 300 & echo "$!" >"$2"; echo "$$" >"$1"; wait' \
        sh "$sandbox/leader" "$sandbox/descendant"
) >/dev/null 2>&1 &
started=$!
for _ in $(seq 1 100); do [ -s "$sandbox/leader" ] && break; sleep 0.1; done
if [ -s "$sandbox/leader" ]; then
    leader="$(cat "$sandbox/leader")"
    descendant="$(cat "$sandbox/descendant")"
    [ "$(ps -o pgid= -p "$leader" | tr -d ' ')" = "$leader" ] || fail "setsid: its command does not lead a process group"
    [ "$(ps -o pgid= -p "$descendant" | tr -d ' ')" = "$leader" ] || fail "setsid: a descendant is outside the command's process group"
    kill -KILL -- "-$leader" 2>/dev/null
    wait "$started" 2>/dev/null
    for _ in $(seq 1 50); do kill -0 "$descendant" 2>/dev/null || break; sleep 0.1; done
    kill -0 "$descendant" 2>/dev/null && { fail "setsid: a descendant survived its process group"; kill -KILL "$descendant"; }
else
    fail "setsid: tests/scripts/support/setsid did not start its command"
    kill -KILL "$started" 2>/dev/null
fi

for guard in kernel-oracle-boot-is-bounded; do
    out="$(PATH="$farm" "$bash_path" "$REPO/tests/scripts/$guard.sh" 2>&1)"
    status=$?
    last="$(printf '%s\n' "$out" | tail -n 1)"
    if [ "$status" -ne 0 ]; then
        fail "$guard exited $status with no setsid or timeout on PATH: $(printf '%s\n' "$out" | tail -n 3 | cut -c1-300)"
    elif [ "${last#PASS}" = "$last" ]; then
        fail "$guard exited 0 without its PASS line: it stopped early"
    fi
done

[ "$fails" -eq 0 ] || exit 1
echo "PASS  the lifecycle guards need no setsid or timeout from the host, and the stand-in for setsid starts a real session"
