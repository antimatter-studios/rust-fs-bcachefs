#!/usr/bin/env bash
# The differential run's reference install (#717),
# scripts/differential-reference-install.sh, against a stand-in for docker.
#
# It must: succeed on a good first attempt without trying again; try again
# after a failed one and succeed; and, when every attempt fails, fail and
# print the end of the log.
#
#   bash tests/scripts/differential-reference-install-retries.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/differential-reference-install.sh"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/reference-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"

# The stand-in fails its first $FAILS calls, counting them in $CALLS.
cat >"$tmp/bin/docker" <<'STAND_IN'
#!/usr/bin/env bash
echo call >>"$CALLS"
n="$(wc -l <"$CALLS" | tr -d ' ')"
if [ "$n" -le "$FAILS" ]; then
    echo "mirror unreachable on call $n"
    exit 100
fi
echo "installed on call $n"
STAND_IN
chmod +x "$tmp/bin/docker"

# $1 failures before success; leaves status, calls, the log and stderr.
install_with() {
    : >"$tmp/calls"
    PATH="$tmp/bin:$PATH" CALLS="$tmp/calls" FAILS="$1" DIFFERENTIAL_INSTALL_PAUSE=0 \
        bash "$SCRIPT" container 1.39.7 "$tmp/install.log" 2>"$tmp/err"
    status=$?
    calls="$(wc -l <"$tmp/calls" | tr -d ' ')"
}

install_with 0
[ "$status" -eq 0 ] && [ "$calls" -eq 1 ] || fail "a good install: status $status after $calls call(s)"

install_with 2
[ "$status" -eq 0 ] && [ "$calls" -eq 3 ] || fail "two failures, then success: status $status after $calls call(s)"
grep -q "installed on call 3" "$tmp/install.log" || fail "the log lacks the successful attempt"

install_with 9
[ "$status" -eq 1 ] && [ "$calls" -eq 3 ] || fail "a persistent failure: status $status after $calls call(s)"
grep -q "mirror unreachable on call 3" "$tmp/err" || fail "the failure does not print the end of the log"

[ "$fails" -eq 0 ] || exit 1
echo "PASS  the reference install is tried three times and prints why it failed"
