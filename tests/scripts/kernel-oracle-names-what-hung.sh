#!/usr/bin/env bash
# A hung kernel-oracle first boot says whose hang it was, read from the guest's
# console (#514). Merge queue run 41640522774 failed with only "the first boot
# was stopped at its 300 s limit": the console showed the reference kernel
# module's journal full on a disk no code of this crate had touched, with its
# writes all on disk and three tasks blocked in the module.
#
# The consoles here are made up, and hold only the lines the report reads. The
# whole job, a hang still failing it, is in
# tests/scripts/kernel-oracle-reference-tools-are-bounded.sh.
#
#   bash tests/scripts/kernel-oracle-names-what-hung.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REPORT="$REPO/scripts/kernel-oracle-hang.py"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/ko-hang.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

stuck() { # stuck DISK SEQ SEQ_ONDISK
    printf '[   15.353711] bcachefs (%s): Journal stuck? Waited for 10s, err journal_full\n' "$1"
    printf '[   15.353711] seq:                       %s\n' "$2"
    printf '[   15.353711] seq_ondisk:                %s\n' "$3"
}
blocked() { # blocked TASK:PID FRAME
    printf '[  242.936362] INFO: task %s blocked for more than 120 seconds.\n' "$1"
    printf '[  242.944416] Call Trace:\n'
    printf '[  242.944920]  <TASK>\n'
    printf '[  242.945352]  schedule+0x27/0xa0\n'
    printf '[  242.946000]  %s\n' "$2"
    printf '[  242.963965]  </TASK>\n'
}
IN_MODULE='some_module_function+0x45/0x1b0 [bcachefs]'
ELSEWHERE='some_kernel_function+0x45/0x1b0'

# case NAME WANT... reads the console in $tmp/console and the guest's record in
# $tmp/oracle. A WANT starting ! must not appear in the report.
case_() {
    name="$1"
    shift
    report="$(python3 "$REPORT" "$tmp/console" "$tmp/oracle" 2>&1)"
    status=$?
    [ "$status" -eq 0 ] || fail "$name: the report exited $status: $report"
    [ "$(printf '%s\n' "$report" | wc -l)" -eq 1 ] || fail "$name: the report is not one line: $report"
    for want; do
        case "$want" in
            !*) case "$report" in *"${want#!}"*) fail "$name: the report says '${want#!}': $report" ;; esac ;;
            *) case "$report" in *"$want"*) ;; *) fail "$name: the report does not say '$want': $report" ;; esac ;;
        esac
    done
}

: >"$tmp/oracle"

# The journal is full with every write on disk, so nothing the disk or the
# runner could still deliver would free it: the module's own hang. Only tasks
# blocked in the module are named. Serial consoles end lines in CRLF.
{
    echo '[    4.948756] bcachefs (vdb): Using encoding defined by superblock'
    stuck vdb 66 66
    blocked kworker/u8:0:12 "$IN_MODULE"
    blocked rm:1697 "$IN_MODULE"
    blocked udevd:99 "$ELSEWHERE"
} | sed 's/$/\r/' >"$tmp/console"
case_ module-deadlock "reference kernel module hung" journal_full vdb \
    "kworker/u8:0" "rm" "!udevd" "!runner" "!1697"

# Two reports: the later one is the state the guest ended in.
{
    stuck vda 40 39
    stuck vdb 66 66
    blocked rm:1697 "$IN_MODULE"
} >"$tmp/console"
case_ last-report-wins "reference kernel module hung" vdb "!vda"

# The same message with journal writes still outstanding is what a disk that
# stopped answering would show, so it is not called the module's.
{
    stuck vdb 66 60
    blocked rm:1697 "$IN_MODULE"
} >"$tmp/console"
case_ writes-outstanding vdb journal_full "6 journal writes" "runner" "!reference kernel module hung"

# Blocked in the module with no journal report: a disk that stopped answering
# looks the same.
blocked rm:1697 "$IN_MODULE" >"$tmp/console"
case_ blocked-only "rm" "blocked in the reference kernel module" "runner" "!reference kernel module hung"

# Blocked elsewhere, and silent: nothing in the console accuses the module.
printf '[   12.500000] something ordinary\n' >"$tmp/console"
blocked udevd:99 "$ELSEWHERE" >>"$tmp/console"
case_ blocked-elsewhere "no report from the reference kernel module" "runner" "!reference kernel module hung"

# A console that stopped early and one still printing at the limit read
# differently: the second is a slow guest, not a hung one. Each names how far
# the guest's record got.
printf 'ORACLE kernel: 7\r\nORACLE M\t/a\tdir\r\nORACLE unmount: ok\r\n' >"$tmp/oracle"
printf '[    1.000000] one\n[   12.500000] two\n' >"$tmp/console"
case_ silent "no report from the reference kernel module" "12.5 s" "unmount: ok" "!reference kernel module hung"
printf '[    1.000000] one\n[  298.000000] two\n' >"$tmp/console"
case_ still-printing "298.0 s" "unmount: ok"
: >"$tmp/oracle"

# Nothing at all, empty or never written, and the record the same.
: >"$tmp/console"
case_ empty-console "printed nothing" "runner"
report_missing="$(python3 "$REPORT" "$tmp/no-such-console" "$tmp/no-such-oracle" 2>&1)" ||
    fail "a missing console: the report exited non-zero: $report_missing"
case "$report_missing" in
    *"printed nothing"*) ;;
    *) fail "a missing console: the report says: $report_missing" ;;
esac

# A console is whatever the guest wrote: bytes that are not text still report.
{
    printf '[    1.000000] \377\376 not text\n'
    stuck vdb 66 66
} >"$tmp/console"
case_ not-text "reference kernel module hung" vdb

if [ "$fails" -gt 0 ]; then
    echo "kernel-oracle-names-what-hung: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  a hung kernel-oracle first boot names the module, the disk under it, or nobody, from its console"
