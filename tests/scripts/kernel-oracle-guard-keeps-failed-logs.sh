#!/usr/bin/env bash
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d "${TMPDIR:-/tmp}/ko-log-control.XXXXXX")"
trap 'rm -r -- "$sandbox"' EXIT
repo="$sandbox/repo"
mkdir -p "$repo/tests/scripts" "$repo/.github/workflows" "$sandbox/bin"
cp -R "$REPO/scripts" "$repo/scripts"
cp -R "$REPO/tests/scripts/support" "$repo/tests/scripts/support"
cp "$REPO/tests/scripts/kernel-oracle-reference-tools-are-bounded.sh" "$repo/tests/scripts/"
cp "$REPO/.github/workflows/ci.yml" "$repo/.github/workflows/"

real_bash="$(command -v bash)"
cat >"$sandbox/bin/bash" <<'EOF'
#!/bin/sh
case "${1:-}" in
    */clean/repo/scripts/kernel-oracle.sh)
        echo 'diagnostic control: the clean scenario deliberately exited 23' >&2
        exit 23
        ;;
    */hung-boot-hung-writer/repo/scripts/kernel-oracle.sh)
        : >"$DIAGNOSTIC_CONTROL_LAST"
        ;;
esac
exec "$DIAGNOSTIC_CONTROL_BASH" "$@"
EOF
chmod +x "$sandbox/bin/bash"

tier_log="$REPO/tmp/logs/kernel-oracle-guard-control.log"
status=0
DIAGNOSTIC_CONTROL_BASH="$real_bash" DIAGNOSTIC_CONTROL_LAST="$sandbox/last-scenario" \
    FS_CORE_CALLER="$REPO" PATH="$sandbox/bin:$PATH" \
    "$real_bash" "$REPO/../rust-fs-core/scripts/tier.sh" \
    diagnostic-control kernel-oracle-guard-control 80 4900 -- \
    "$real_bash" "$repo/tests/scripts/kernel-oracle-reference-tools-are-bounded.sh" \
    >"$sandbox/verdict" 2>&1 || status=$?

[ "$status" -eq 1 ] || fail "the failed clean scenario returned $status, not 1"
grep -q '^FAIL  clean: exited 23, not 0$' "$tier_log" ||
    fail "the clean scenario was not deliberately failed"
[ -e "$sandbox/last-scenario" ] || fail "the guard stopped before its last scenario"
grep -q 'the fixtures job uploads no\|kernel-oracle.sh writes no' "$tier_log" ||
    fail "the assertions after the last scenario did not run"
grep -Fq "$tier_log" "$sandbox/verdict" || fail "the tier verdict names no diagnostic log"
[ "$(wc -l <"$sandbox/verdict")" -eq 1 ] || fail "the failed tier printed more than its verdict"
! grep -Fq 'diagnostic control: the clean scenario' "$sandbox/verdict" ||
    fail "the tier printed the underlying diagnostic on the screen"

diagnostic="$(sed -n 's/^kernel-oracle-reference-tools-are-bounded: diagnostics: //p' "$tier_log")"
if [ -n "$diagnostic" ] && [ -f "$diagnostic" ]; then
    grep -Fq 'diagnostic control: the clean scenario deliberately exited 23' "$diagnostic" ||
        fail "the named diagnostic log lost the clean scenario's underlying error"
    grep -q '/hung-boot-hung-writer/log$' "$diagnostic" ||
        fail "the diagnostic log lost the last scenario"
else
    fail "the failed guard names no retained diagnostic log"
fi

if [ "$fails" -gt 0 ]; then
    echo "kernel-oracle-guard-keeps-failed-logs: $fails check(s) failed; see $tier_log" >&2
    exit 1
fi
echo "PASS  a failed clean scenario keeps its diagnostic log by name without stopping later assertions or printing the log"
