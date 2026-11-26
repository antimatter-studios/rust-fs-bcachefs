#!/usr/bin/env bash
# Three guards give their verdict on a slow host as on an idle one (#707,
# CI-04; #595, #605). Load made them fail by chance, so each condition is made
# here every time by a stand-in on PATH, and the unchanged guard is run:
#   - a QEMU stand-in that starts later than the boot's 1-second limit
#   - a process the watchdog stops that does not end on SIGTERM
#   - an s1-overlap report far longer than a pipe holds
# A guest that never starts in any attempt must still fail, by name.
#
#   bash tests/scripts/load-sensitive-guards-hold.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

work="$(mktemp -d "${TMPDIR:-/tmp}/load-guards.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir "$work/slow-once" "$work/slow-always" "$work/deaf" "$work/long"
real_bash="$(command -v bash)"
real_python="$(command -v python3)"
real_sleep="$(command -v sleep)"

# bash, which starts the QEMU stand-in late: the first time, or every time.
for kind in once always; do
    cat >"$work/slow-$kind/bash" <<STUB
#!$real_bash
case "\${1:-}" in
    */qemu)
        if [ $kind = always ] || [ ! -e "$work/delayed" ]; then
            : >"$work/delayed"
            "$real_sleep" 3
        fi
        ;;
esac
exec "$real_bash" "\$@"
STUB
done

# The build's long sleep ignores SIGTERM, as a process slow to die does.
cat >"$work/deaf/sleep" <<STUB
#!$real_bash
[ "\${1:-}" != 120 ] || trap '' TERM
exec "$real_sleep" "\$@"
STUB

# s1-overlap, reporting the copy first and then far more than a pipe holds.
cat >"$work/long/python3" <<STUB
#!$real_bash
case "\${2:-}" in
    check)
        [ -e "\$4/copied.rs" ] || exit 0
        echo "s1-overlap: copied.rs:1-2: 8 words S1 has verbatim: stand-in"
        awk 'BEGIN { for (i = 0; i < 20000; i++) print "stand-in report line ........................................" }'
        exit 1
        ;;
    build-text) exit 0 ;;
esac
exec "$real_python" "\$@"
STUB
chmod +x "$work"/*/*

# guard STUBS NAME: leaves status and out.
guard() {
    out="$(PATH="$work/$1:$PATH" "$real_bash" "$REPO/tests/scripts/$2" 2>&1)"
    status=$?
}
report() { fail "$1 (status $status): $(printf '%s\n' "$out" | cut -c1-200 | tail -n 8)"; }

guard slow-once kernel-oracle-boot-is-bounded.sh
[ "$status" -eq 0 ] || report "a late QEMU stand-in fails the boot-bound guard"
[ -e "$work/delayed" ] || fail "the late-start stand-in was never used"
case "$out" in
    *"stopped before it ignored SIGTERM"*) ;;
    *) report "the boot-bound guard does not name the attempt that tested nothing" ;;
esac
guard slow-always kernel-oracle-boot-is-bounded.sh
[ "$status" -ne 0 ] || report "a QEMU stand-in that never starts passes the boot-bound guard"
case "$out" in
    *"FAIL  the stand-in QEMU never ran"*) ;;
    *) report "the boot-bound guard does not say the stand-in never ran" ;;
esac
guard deaf guest-watchdog-stops-only-its-run.sh
[ "$status" -eq 0 ] || report "a process slow to end fails the watchdog guard, or outlives the watchdog"
guard long s1-overlap-detects-copies.sh
[ "$status" -eq 0 ] || report "a long report fails the s1-overlap guard"

if [ "$fails" -gt 0 ]; then
    echo "load-sensitive-guards-hold: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  a late start, a slow end and a long report change no guard's verdict"
