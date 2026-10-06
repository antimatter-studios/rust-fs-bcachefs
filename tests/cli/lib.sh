# shellcheck shell=bash
# tests/cli/lib.sh -- what every tests/cli/test-*.sh sources.
#
# The house style: `ok`/`fail`, a `fails` counter, `set -uo pipefail` (not
# -e, so later checks still run after one fails), a sandbox under the
# repository's tmp/, and `finish` last, which prints the count
# test-floor.sh reads and the trailing `<name>: all checks passed` line
# scripts/test-cli.sh requires.
#
# The tools are whatever PATH finds: scripts/test-cli.sh has already made
# sure they are ours before any file runs.
set -uo pipefail

NAME="$(basename "$0" .sh)"
REPO="$(cd "$(dirname "$0")/../.." && pwd -P)"
CRATE="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -n 1)"
FIXTURES="$REPO/.vm-share/fixtures"

passed=0
fails=0
ok() { passed=$((passed + 1)); }
fail() {
    echo "FAIL  $NAME: $*" >&2
    fails=$((fails + 1))
}

# check DESCRIPTION COMMAND...: ok if COMMAND succeeds, else fail naming it.
check() {
    local what="$1"
    shift
    if "$@"; then ok; else fail "$what"; fi
}

# need_fixture NAME: the image NAME.img and its manifest exist, or the whole
# file fails naming the task that builds them. Nothing skips.
need_fixture() {
    if [ ! -f "$FIXTURES/$1.img" ] || [ ! -f "$FIXTURES/$1.json" ]; then
        echo "FAIL  $NAME: $FIXTURES/$1.img is missing -- build the fixtures with \`chore fixtures\`" >&2
        echo "test result: FAILED. $passed passed; 1 failed"
        exit 1
    fi
}

mkdir -p "$REPO/tmp"
SANDBOX="$(mktemp -d "$REPO/tmp/cli-$NAME.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT HUP INT TERM

finish() {
    if [ "$fails" -gt 0 ]; then
        echo "test result: FAILED. $passed passed; $fails failed"
        exit 1
    fi
    echo "test result: ok. $passed passed; 0 failed"
    echo "$NAME: all checks passed"
    exit 0
}
