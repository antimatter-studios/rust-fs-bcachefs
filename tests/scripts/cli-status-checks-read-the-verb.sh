#!/usr/bin/env bash
# A status check in tests/cli reads the verb's status (#445). In
#
#   check "put ($(cat "$SANDBOX/err"))" test $? -eq 0
#
# bash expands the words left to right, so $? is cat's status and the check
# passes whatever the verb returned. Two things hold that: no check in
# tests/cli is written that way, and the real tests/cli/test-write.sh, run
# against a stand-in fs.bcachefs whose put fails, reports its put checks failed
# in every mode. The stand-in also passes once, so a check that always fails
# does not satisfy this.
#
#   bash tests/scripts/cli-status-checks-read-the-verb.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d)"
trap 'rm -rf -- "$sandbox"' EXIT

scan() {
    python3 - "$@" <<'PY'
import re
import sys

# A logical line: continuation lines are one command.
found = 0
for path in sys.argv[1:]:
    number, logical, first = 0, '', 0
    for line in open(path, encoding='utf-8'):
        number += 1
        first = first or number
        logical += line.rstrip('\n')
        if logical.endswith('\\'):
            logical = logical[:-1]
            continue
        if re.match(r'\s*check\b.*\$\(.*\$\?', logical):
            print(f'{path}:{first}: {logical.strip()}')
            found += 1
        logical, first = '', 0
sys.exit(1 if found else 0)
PY
}

# The scan can find what it looks for, on one line and across a continuation.
# shellcheck disable=SC2016 # the planted lines are data for the scan.
printf '%s\n' 'check "a ($(cat err))" test $? -eq 0' >"$sandbox/one.sh"
# shellcheck disable=SC2016 # the planted lines are data for the scan.
printf '%s\n' 'check "a ($(cat err))" \' '    test $? -eq 0' >"$sandbox/two.sh"
# shellcheck disable=SC2016 # the planted lines are data for the scan.
printf '%s\n' 'status=$?' 'check "a ($(cat err))" test "$status" -eq 0' 'check "b" test $? -eq 0' >"$sandbox/clean.sh"
scan "$sandbox/one.sh" >/dev/null && fail "the scan missed a check on one line"
scan "$sandbox/two.sh" >/dev/null && fail "the scan missed a check across a continuation"
scan "$sandbox/clean.sh" >/dev/null || fail "the scan refused a status captured first"

if ! found="$(scan "$REPO"/tests/cli/*.sh)"; then
    fail "these checks read a command substitution's status, not the verb's: $found"
fi

# The real test-write.sh, in a tree of its own, with empty fixtures.
tree="$sandbox/tree"
mkdir -p "$tree/tests/cli" "$tree/.vm-share/fixtures/write-study" "$sandbox/bin"
cp "$REPO/tests/cli/lib.sh" "$REPO/tests/cli/offset-lib.sh" "$REPO/tests/cli/test-write.sh" "$tree/tests/cli/"
printf 'name = "stand-in"\n' >"$tree/Cargo.toml"
: >"$tree/.vm-share/fixtures/write-study/base.img"
: >"$tree/.vm-share/fixtures/casefold.img"
: >"$tree/.vm-share/fixtures/kernel-rootsnap.img"
cat >"$sandbox/bin/fs.bcachefs" <<'STUB'
#!/usr/bin/env bash
# --offset N IMAGE VERB ...: the put verb answers as STUB_PUT says.
[ "${4:-}" != put ] && exit 0
cat >/dev/null
[ "$STUB_PUT" = fails ] || exit 0
echo '{"error": "the stand-in refused", "code": 1}' >&2
exit 1
STUB
printf '#!/usr/bin/env bash\nexit 0\n' >"$sandbox/bin/fsck.bcachefs"
chmod +x "$sandbox/bin/fs.bcachefs" "$sandbox/bin/fsck.bcachefs"

run_write() {
    # shellcheck disable=SC2030 # scoped to this one run on purpose.
    PATH="$sandbox/bin:$PATH" STUB_PUT="$1" bash "$tree/tests/cli/test-write.sh" </dev/null 2>&1
}

failing="$(run_write fails)"
passing="$(run_write passes)"
for mode in in-place journal in-place-offset journal-offset; do
    for what in 'put from stdin' 'put --from a 200000-byte file'; do
        printf '%s\n' "$failing" | grep -qF "FAIL  test-write: $mode: $what (" ||
            fail "$mode: '$what' passed with a put that failed: the check does not read the verb's status"
        printf '%s\n' "$passing" | grep -qF "FAIL  test-write: $mode: $what (" &&
            fail "$mode: '$what' failed with a put that succeeded"
    done
done

[ "$fails" -eq 0 ] || exit 1
echo "PASS  the status checks in tests/cli read the verb's status, and a failing put fails them"
