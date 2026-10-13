#!/usr/bin/env bash
# A failed build of the reference implementation must not print its
# compiler output (#153). The clean-room rule is that nobody working on this
# crate reads the reference source in any form, and a compiler diagnostic
# quotes the source line it complains about. Printed by a script, it lands in
# the transcript of whoever ran `chore fixtures`, and in the CI log that
# agents read with `gh run view --log-failed`.
#
# So: no script prints a build log, and the reference module's DKMS build,
# which runs in the kernel oracle's container, sends its stderr to a file.
#
#   bash tests/scripts/reference-build-output-stays-put.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO" || exit 1

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

# 1. Nothing prints a make.log: not tail, head, cat, sed or awk of one.
printed="$(grep -nE '\b(tail|head|cat|less|more|sed|awk)\b[^#]*make\.log' scripts/*.sh || true)"
if [ -z "$printed" ]; then
    ok "no script prints a reference build log"
else
    fail "a script prints a reference build log, which can quote the reference source:
$printed"
fi

# 2. The DKMS install's stderr goes to a file. The command spans lines, so
#    continuation lines are joined first.
dkms="$(awk '{ if (sub(/\\$/, "")) { buf = buf $0; next } print buf $0; buf = "" }' scripts/kernel-oracle.sh |
    grep -E 'apt-get install[^#]*bcachefs-kernel-dkms' || true)"
if [ -z "$dkms" ]; then
    fail "scripts/kernel-oracle.sh has no apt-get install of bcachefs-kernel-dkms to check"
elif printf '%s\n' "$dkms" | grep -qE '2>[[:space:]]*[^&[:space:]]|2>&1' &&
    ! printf '%s\n' "$dkms" | grep -qE '2>&2'; then
    ok "the module's DKMS build sends its stderr to a file"
else
    fail "the module's DKMS build leaves its stderr to the CI log:
$dkms"
fi

if [ "$fails" -gt 0 ]; then
    echo "reference-build-output-stays-put: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  no reference build prints its compiler output"
