#!/usr/bin/env bash
# scripts/s1-overlap.py must catch a run of eight words copied from a
# specification and pass one of seven, or the check that keeps S1's prose out
# of this repository (#154) could pass while it is there. It runs on made-up
# text, so this test carries none of S1.
#
#   bash tests/scripts/s1-overlap-detects-copies.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/s1-overlap.py"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

work="$(mktemp -d "${TMPDIR:-/tmp}/s1-overlap-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT

printf 'alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima\n' >"$work/spec.txt"
python3 "$SCRIPT" build-text "$work/spec.txt" --hashes "$work/h.bin" >/dev/null

r="$work/r"
mkdir -p "$r" && git -C "$r" init -q
printf '// Bravo, charlie\n// delta echo -- foxtrot golf hotel india.\n' >"$r/copied.rs"
printf 'bravo charlie delta echo foxtrot golf hotel\n' >"$r/short.md"
git -C "$r" add -A

out="$(python3 "$SCRIPT" check --root "$r" --hashes "$work/h.bin" 2>&1)"
status=$?
if [ "$status" -ne 0 ] && printf '%s' "$out" | grep -q 'copied.rs:1-2: 8 words'; then
    ok "eight copied words are caught, across lines and punctuation"
else
    fail "eight copied words were not caught (status $status): $out"
fi
if printf '%s' "$out" | grep -q 'short.md'; then
    fail "seven shared words were reported: $out"
else
    ok "seven shared words are not"
fi

git -C "$r" rm -qf copied.rs
if python3 "$SCRIPT" check --root "$r" --hashes "$work/h.bin" >/dev/null 2>&1; then
    ok "with the copy gone, the check passes"
else
    fail "the check still fails with nothing copied"
fi

if [ "$fails" -gt 0 ]; then
    echo "s1-overlap-detects-copies: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  s1-overlap catches 8 copied words and passes 7"
