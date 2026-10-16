#!/usr/bin/env bash
# Controls for how the pinned default-74 job reads its own test's log. They
# run scripts/pinned-default74-reference.sh --observed-tuple, the same
# function the live path uses, before any container starts: a well-formed
# single observation must yield its complete key/stored/computed tuple, and
# a missing, malformed or ambiguous observation must be refused.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
ref="$here/pinned-default74-reference.sh"
t="$(mktemp -d)"
fails=0
ok() { echo "ok: $*"; }
bad() { echo "FAIL: $*" >&2; fails=$((fails + 1)); }

accept() {
    local name="$1" want="$2" got
    got="$(bash "$ref" --observed-tuple "$t/$name.log" 2>/dev/null)" || { bad "$name refused"; return; }
    [ "$got" = "$want" ] && ok "$name yields '$got'" || bad "$name yields '$got', not '$want'"
}
refuse() {
    local name="$1" path="${2:-$t/$1.log}" got
    if got="$(bash "$ref" --observed-tuple "$path" 2>/dev/null)"; then
        bad "$name accepted as '$got'"
    else
        [ -z "$got" ] && ok "$name refused" || bad "$name refused but printed '$got'"
    fi
}

valid='default-74: extent_data key 2147483904:1:U32_MAX stored 0x11ba7 computed 0xa2f1ba7'
printf 'running 1 test\n%s\ntest default74 ... ok\n' "$valid" >"$t/recorded.log"
accept recorded "2147483904:1:U32_MAX 0x11ba7 0xa2f1ba7"

# A different tuple parses to itself, so the parse is not the expected value.
printf '%s\n' 'default-74: extent_data key 4096:2:U32_MAX stored 0x1 computed 0xdeadbeef' >"$t/other.log"
accept other "4096:2:U32_MAX 0x1 0xdeadbeef"

refuse missing "$t/absent.log"
: >"$t/empty.log"
refuse empty
printf 'running 1 test\ntest default74 ... ok\n' >"$t/no-observation.log"
refuse no-observation
printf '%s\n' 'default-74: extent_data key 2147483904:1:U32_MAX stored 0x11ba7' >"$t/no-computed.log"
refuse no-computed
printf '%s\n' 'default-74: extent_data key 2147483904:1:U32_MAX stored 0x11ba7 computed ' >"$t/empty-computed.log"
refuse empty-computed
printf '%s\n' 'default-74: extent_data key 2147483904:1:U32_MAX stored 0x11BA7 computed 0xA2F1BA7' >"$t/uppercase.log"
refuse uppercase
printf '%s extra\n' "$valid" >"$t/trailing.log"
refuse trailing
printf '%s\n%s\n' "$valid" 'default-74: extent_data key 4096:2:U32_MAX stored 0x1 computed 0x2' >"$t/ambiguous.log"
refuse ambiguous

rm -r "$t"
[ "$fails" = 0 ] || exit 1
echo "pinned-default74-observation-controls: all checks passed"
