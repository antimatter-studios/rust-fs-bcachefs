#!/usr/bin/env bash
# A guest suite that loses its guest says what stopped the guest, and keeps
# the guest's console, instead of ending in a bare exit 255 (#504). Runs
# 41598439077 and 41608993199 failed `chore test:vm` with exit 255 and no failed
# test, and the console went with the runner. In CI a run that passes and used
# more of the host's disk than the smallest runner has fails too: that runner is
# about one job in fifty, so a run on any other would not show it.
#
# scripts/guest-session.sh runs in a sandbox beside a stand-in harness. `ps`,
# `nc`, `df` and `sudo` are stand-ins too, so each scenario decides what QEMU, its
# monitor and the disk answer. A real guest stopped under the real harness is the proof this
# cannot give; the pull request that added this test records that run.
#
#   bash tests/scripts/guest-session-names-what-stopped.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/guest-session.sh"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

# The host's own timeout(1) wins; tests/scripts/support is for a host with none.
export PATH="$PATH:$REPO/tests/scripts/support"
command -v timeout >/dev/null || {
    fail "timeout(1) is missing: this test needs GNU coreutils', or tests/scripts/support/timeout"
    exit 1
}

[ -f "$SCRIPT" ] || {
    fail "scripts/guest-session.sh is missing: nothing names what stopped a guest"
    exit 1
}

# Twice the answer limit below and room for the rest, so that a guest that never
# answers is seen failing this test rather than holding the tier.
WATCHDOG=30

tmp="$(mktemp -d "${TMPDIR:-/tmp}/guest-session.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

harness="$tmp/fs-linux-test-harness/scripts"
repo="$tmp/repo"
stubs="$tmp/stubs"
mkdir -p "$harness" "$repo/scripts" "$stubs"
cp "$SCRIPT" "$repo/scripts/"

# The two calls the session makes of the harness, and the one way into the guest.
cat >"$harness/vm.sh" <<'STUB'
#!/usr/bin/env bash
case "$1" in
    config)
        printf 'project.name=sandbox\nmachine=%s\nidentity=%s\n' "$GS_STATE/machine" "$GS_STATE/machine"
        ;;
    exec)
        echo "asked the guest" >>"$GS_STATE/order"
        case "$GS_GUEST" in
            answers)
                echo "up 612 s"
                echo "             total  used  free"
                echo "kernel: reference line from the guest's log"
                ;;
            hangs) exec sleep 120 ;;
            *) exit 255 ;;
        esac
        ;;
    *) echo "stand-in vm.sh: unexpected call: $*" >&2; exit 64 ;;
esac
STUB

# As the real file does when sourced: an EXIT trap that keeps the status.
cat >"$harness/vm-session.sh" <<'STUB'
echo "session began" >>"$GS_STATE/order"
trap 'code=$?; trap - EXIT; echo "torn down" >>"$GS_STATE/order"; exit "$code"' EXIT
STUB

cat >"$stubs/ps" <<'STUB'
#!/usr/bin/env bash
case "$GS_QEMU" in
    alive) echo "4242 /usr/bin/qemu-system-x86_64 -chardev socket,id=mon0,path=$GS_STATE/monitor,server=on,wait=off -drive file=$GS_STATE/machine/disk.img"
        # More than a pipe holds, after the match: a reader that stops at its
        # first match fails the writer under pipefail, and reads as no QEMU.
        for n in $(seq 1 3000); do
            echo "$n /usr/bin/a-process-with-a-long-command-line --and-its-arguments --that-go-on-for-a-while"
        done
        ;;
    gone) echo "1 /sbin/init" ;;
    unreadable) exit 1 ;;
esac
STUB
cat >"$stubs/nc" <<'STUB'
#!/usr/bin/env bash
cat >/dev/null
printf 'QEMU 8.2.2 monitor\r\n(qemu) info status\r\n%s\r\n(qemu) info block\r\ndisk: file (raw)\r\n    %s\r\n' "$GS_STATUS" "$GS_IO"
STUB
# The free space before the session begins, and at every call after that.
cat >"$stubs/df" <<'STUB'
#!/usr/bin/env bash
free="$GS_DF_AFTER"
if [ ! -e "$GS_STATE/asked-df" ]; then
    : >"$GS_STATE/asked-df"
    free="$GS_DF_BEFORE"
fi
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "/dev/stand-in 90000000 1000 $free 1% /"
STUB
printf '#!/usr/bin/env bash\nexit 1\n' >"$stubs/sudo"
chmod +x "$harness/vm.sh" "$stubs/ps" "$stubs/nc" "$stubs/df" "$stubs/sudo"

# A command that ends with the status it is given: the suite's stand-in.
cat >"$tmp/suite" <<'STUB'
#!/usr/bin/env bash
echo "suite ran" >>"$GS_STATE/order"
echo "suite said this"
exit "$1"
STUB
chmod +x "$tmp/suite"

# Kilobytes free before and after, and CI's variable, for the next scenarios.
df_before=30000000
df_after=29000000
ci=

# scenario NAME STATUS QEMU GUEST CONSOLE STATE IO: one run; sets out, rc, state.
scenario() {
    name="$1"
    state="$tmp/state-$name"
    mkdir -p "$state/machine"
    if [ "$5" != none ]; then
        printf 'boot line\n%s\n' "$5" >"$state/machine/console.log"
    fi
    rm -rf "$repo/tmp"
    out="$(
        cd "$repo" &&
            PATH="$stubs:$PATH" GS_STATE="$state" GS_QEMU="$3" GS_GUEST="$4" \
                GS_STATUS="$6" GS_IO="$7" FS_BCACHEFS_GUEST_ANSWER_LIMIT=2 \
                GS_DF_BEFORE="$df_before" GS_DF_AFTER="$df_after" CI="$ci" \
                timeout "$WATCHDOG" bash scripts/guest-session.sh "$tmp/suite" "$2" 2>&1
    )"
    rc=$?
}

says() { # WHAT: the output names it
    case "$out" in
        *"$1"*) ;;
        *) fail "$name: the output does not say '$1':"$'\n'"$out" ;;
    esac
}

silent() { # WHAT: the output does not name it
    case "$out" in
        *"$1"*) fail "$name: the output says '$1' and should not:"$'\n'"$out" ;;
    esac
}

status_is() {
    [ "$rc" = "$1" ] || fail "$name: exit status $rc, expected $1"$'\n'"$out"
}

# What every stopped guest gets: the status, the record, the order.
lost() {
    status_is 255
    says "guest-session: FAILED: the connection to the guest was lost (exit 255)"
    says "no test failed by this"
    says "suite said this"
    tail -n 1 "$state/order" | grep -qx "torn down" ||
        fail "$name: the guest was not torn down last: $(tr '\n' ',' <"$state/order")"
}

kept() {
    grep -qF "$1" "$repo/tmp/logs/vm-console.log" 2>/dev/null ||
        fail "$name: tmp/logs/vm-console.log does not hold '$1'"
}

# A pass and a failed test say nothing of a lost guest, and keep no console.
scenario pass 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 0
says "suite said this"
silent "guest-session: FAILED"
[ ! -e "$repo/tmp/logs/vm-console.log" ] || fail "$name: a console was kept on a pass"
[ "$(tr '\n' ',' <"$state/order")" = "session began,suite ran,torn down," ] ||
    fail "$name: order was $(tr '\n' ',' <"$state/order")"

scenario failed-test 1 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 1
silent "guest-session: FAILED"
[ ! -e "$repo/tmp/logs/vm-console.log" ] || fail "$name: a console was kept on a failed test"

# The guest powered itself off: its console says so, and QEMU has gone.
scenario powered-off 255 gone refused "reboot: Power down" x x
lost
says "the guest powered itself off"
says "reboot: Power down"
kept "reboot: Power down"
silent "asked the guest"

# QEMU has gone with no such line: killed, or crashed.
scenario qemu-gone 255 gone refused "an ordinary line" x x
lost
says "QEMU ended and the guest's console records no power-off"
kept "an ordinary line"

# QEMU paused the guest on a failed write: what a full host disk does.
scenario paused 255 alive hangs "an ordinary line" "VM status: paused (io-error)" "I/O status: nospace"
lost
says "QEMU paused the guest: paused (io-error), I/O status: nospace"
says "host:"
kept "an ordinary line"

# QEMU runs and the guest does not answer, within the limit: hung or crashed.
scenario hung 255 alive hangs "BUG: soft lockup" "VM status: running" "I/O status: ok"
lost
says "QEMU is running the guest and the guest does not answer"
says "BUG: soft lockup"
kept "BUG: soft lockup"

# The guest answers: what it says is in the output, and the guest is not blamed.
scenario answers 255 alive answers "an ordinary line" "VM status: running" "I/O status: ok"
lost
says "the guest is up and answers"
says "or the suite itself exited 255"
says "guest: up 612 s"
says "guest: kernel: reference line from the guest's log"

# Nothing to read must not end the report early, and must be said.
scenario no-console 255 gone refused none x x
lost
says "the guest wrote no console"
[ ! -e "$repo/tmp/logs/vm-console.log" ] || fail "$name: a console was kept from nothing"

scenario no-monitor 255 alive answers "an ordinary line" "" ""
lost
says "the guest is up and answers"
says "QEMU's monitor did not answer"

scenario no-process-table 255 unreadable refused "an ordinary line" x x
lost
says "the process table could not be read"
kept "an ordinary line"

# The disk budget. Under it, the run says what it used and passes.
ci=true
scenario under-budget 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 0
says "guest-session: this run used 976 MB of the host's disk; its budget is"

# Over it, a run that passed fails, and says what the budget is for.
df_after=15000000
scenario over-budget 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 66
says "guest-session: FAILED: this run used 14648 MB of the host's disk, over its budget of"
says "14 GB"
tail -n 1 "$state/order" | grep -qx "torn down" || fail "$name: the guest was not torn down last"

# A failed test stays what it was: its status is the one to read.
scenario over-budget-and-failed 1 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 1

# A stopped guest stays what it was too, and says how much it had used.
scenario over-budget-and-lost 255 gone refused "an ordinary line" x x
lost
says "this run used 14648 MB of the host's disk"

# On a developer's machine other work moves the free space: it is not judged.
ci=
scenario over-budget-off-ci 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 0
silent "guest-session: FAILED"
df_after=29000000

# The guest suite goes through it, and CI uploads where the console is kept.
sed -n '/^  test:vm:$/,/^  [a-z:_-]*:$/p' "$REPO/chores.yml" | grep -qF 'scripts/guest-session.sh' ||
    fail "chores.yml: test:vm does not run through scripts/guest-session.sh"
sed -n '/^  suite-in-vm:$/,/^  cli:$/p' "$REPO/.github/workflows/ci.yml" | grep -qF 'path: tmp/logs/*.log' ||
    fail "ci.yml: suite-in-vm does not upload tmp/logs/*.log, where the console is kept"

[ "$fails" -eq 0 ] || exit 1
echo "guest-session-names-what-stopped: all checks passed"
