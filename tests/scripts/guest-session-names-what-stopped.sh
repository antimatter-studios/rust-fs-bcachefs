#!/usr/bin/env bash
# A guest suite that loses its guest says what stopped the guest, and keeps
# the guest's console, instead of ending in a bare exit 255 (#504). Runs
# 41598439077 and 41608993199 failed `chore test:vm` with exit 255 and no failed
# test, and the console went with the runner. In CI a run that passes and used
# more of the host's disk than the smallest runner has fails too: that runner is
# about one job in fifty, so a run on any other would not show it. What it used
# is the most it held at once, not what it still held at the end: since harness
# v0.5.6 the overlay gives back what the guest deletes (#530).
#
# scripts/guest-session.sh runs in a sandbox beside a stand-in harness. `ps`,
# `nc`, `df`, `sleep` and `sudo` are stand-ins too, so each scenario decides what QEMU, its
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

# The stand-in sleep and the stand-in suite wait for real, on this one.
REAL_SLEEP="$(command -v sleep)"

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

# As the real file does when sourced: an EXIT trap that keeps the status, and a
# signal is an exit with its status. A teardown lasts as long as a scenario says,
# and records how many looks at the disk it saw begin and end around it.
cat >"$harness/vm-session.sh" <<'STUB'
echo "session began" >>"$GS_STATE/order"
teardown() {
    if [ -n "${GS_TEARDOWN_HOLD:-}" ]; then
        printf '%s ' "$(($(wc -l <"$GS_STATE/asked-df")))" >"$GS_STATE/looks-in-teardown"
        "$GS_REAL_SLEEP" "$GS_TEARDOWN_HOLD"
        echo "$(($(wc -l <"$GS_STATE/asked-df")))" >>"$GS_STATE/looks-in-teardown"
    fi
    echo "torn down" >>"$GS_STATE/order"
}
trap 'code=$?; trap - EXIT; teardown; exit "$code"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
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
# The free space before the session begins, and at every call after that, except
# while the suite's overlay is at its peak. `unreadable` is a df that fails.
cat >"$stubs/df" <<'STUB'
#!/usr/bin/env bash
free="$GS_DF_AFTER"
if [ ! -e "$GS_STATE/asked-df" ]; then
    free="$GS_DF_BEFORE"
elif [ -e "$GS_STATE/peak" ]; then
    free="$(cat "$GS_STATE/peak")"
    echo look >>"$GS_STATE/looks-at-peak"
fi
echo look >>"$GS_STATE/asked-df"
[ "$free" != unreadable ] || exit 1
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "/dev/stand-in 90000000 1000 $free 1% /"
STUB
printf '#!/usr/bin/env bash\nexit 1\n' >"$stubs/sudo"
# A wait, carrying the sandbox's path on its command line so that one left running
# is found, and writing to whatever it inherited, so that a pipe a sampler keeps
# open shows. A tick of the sampler is any wait the script's other waits are not.
# It is recorded and it is short, unless a scenario asks for one that outlasts it.
cat >"$stubs/sleep" <<'STUB'
#!/usr/bin/env bash
echo "the stand-in sleep wrote this"
echo "the stand-in sleep wrote this" >&2
case "$1" in
    0.1 | 120) "$GS_REAL_SLEEP" "$1" ;;
    *)
        echo "$1" >>"$GS_STATE/ticks"
        echo "$PPID" >"$GS_STATE/sampler"
        if [ -e "$GS_STATE/long-ticks" ]; then
            "$GS_REAL_SLEEP" 5
        else
            "$GS_REAL_SLEEP" 0.1
        fi
        ;;
esac
STUB
chmod +x "$harness/vm.sh" "$stubs/ps" "$stubs/nc" "$stubs/df" "$stubs/sudo" "$stubs/sleep"

# A command that ends with the status it is given: the suite's stand-in. With
# GS_PEAK its overlay reaches that much free space and gives it back before it
# ends. The peak lasts for two looks at the disk, because the second one proves
# the first was recorded, or until a run with nothing looking has shown that.
# GS_KILL_SAMPLER takes the sampler away, and GS_HOLD waits for a scenario's word.
cat >"$tmp/suite" <<'STUB'
#!/usr/bin/env bash
echo "suite ran" >>"$GS_STATE/order"
echo "suite said this"
if [ -n "${GS_PEAK:-}" ]; then
    : >"$GS_STATE/looks-at-peak"
    echo "$GS_PEAK" >"$GS_STATE/peak"
    n=0
    while [ "$n" -lt 100 ] && [ "$(($(wc -l <"$GS_STATE/looks-at-peak")))" -lt 2 ]; do
        "$GS_REAL_SLEEP" 0.1
        n=$((n + 1))
    done
    rm -f "$GS_STATE/peak"
fi
if [ -n "${GS_KILL_SAMPLER:-}" ] && [ -e "$GS_STATE/sampler" ]; then
    sampler="$(cat "$GS_STATE/sampler")"
    kill -s KILL -- "-$sampler" 2>/dev/null || kill -s KILL "$sampler" 2>/dev/null
    n=0
    while [ "$n" -lt 50 ] && kill -0 "$sampler" 2>/dev/null; do
        "$GS_REAL_SLEEP" 0.1
        n=$((n + 1))
    done
fi
if [ -n "${GS_HOLD:-}" ]; then
    : >"$GS_STATE/held"
    n=0
    while [ "$n" -lt 300 ] && [ ! -e "$GS_STATE/release" ]; do
        "$GS_REAL_SLEEP" 0.1
        n=$((n + 1))
    done
fi
exit "$1"
STUB
chmod +x "$tmp/suite"

# Kilobytes free before and after and at the peak, and CI's variable, for the next
# scenarios.
df_before=30000000
df_after=29000000
peak=
kill_sampler=
ci=

# scenario NAME STATUS QEMU GUEST CONSOLE STATE IO: one run; sets out, rc, state.
scenario() {
    name="$1"
    state="$tmp/state-$name"
    mkdir -p "$state/machine" "$state/tmp"
    if [ "$5" != none ]; then
        printf 'boot line\n%s\n' "$5" >"$state/machine/console.log"
    fi
    rm -rf "$repo/tmp"
    out="$(
        cd "$repo" &&
            PATH="$stubs:$PATH" GS_STATE="$state" GS_QEMU="$3" GS_GUEST="$4" \
                GS_STATUS="$6" GS_IO="$7" FS_BCACHEFS_GUEST_ANSWER_LIMIT=2 \
                GS_DF_BEFORE="$df_before" GS_DF_AFTER="$df_after" CI="$ci" \
                GS_PEAK="$peak" GS_KILL_SAMPLER="$kill_sampler" GS_REAL_SLEEP="$REAL_SLEEP" \
                TMPDIR="$state/tmp" \
                timeout "$WATCHDOG" bash scripts/guest-session.sh "$tmp/suite" "$2" 2>&1
    )"
    rc=$?
    nothing_inherited
    no_temporary_files
}

# Whatever writes to a pipe the sampler kept, and a shell's notice of a job it
# stopped, would land in the tier's output. A sampler something else killed is the
# one job a notice is true of, so only that scenario may carry one.
nothing_inherited() {
    silent "the stand-in sleep wrote this"
    silent "Terminated"
    [ -n "$kill_sampler" ] || silent "Killed"
}

no_temporary_files() {
    [ -z "$(ls -A "$state/tmp")" ] ||
        fail "$name: temporary files were left behind: $(ls -A "$state/tmp" | tr '\n' ' ')"
}

# What is still running from the sandbox, by its path on the command line: a
# subshell of the session carries the session's, and a stand-in carries its own.
from_the_sandbox() {
    local procs
    procs="$(ps -ww -eo pid=,args=)"
    printf '%s\n' "$procs" | awk -v want="$tmp/" 'index($0, want) > 0'
}

# none_left_within TENTHS: signalled processes need a moment to go, and no more.
# What is left is reported and stopped, so that it does not outlive this test.
none_left_within() {
    local n=0 left
    while :; do
        left="$(from_the_sandbox)"
        [ -n "$left" ] || return 0
        [ "$n" -lt "$1" ] || break
        "$REAL_SLEEP" 0.1
        n=$((n + 1))
    done
    fail "$name: still running after the session ended:"$'\n'"$left"
    printf '%s\n' "$left" | while read -r pid _; do
        kill -s KILL "$pid" 2>/dev/null
    done
}

# session_ends NAME STATUS HOW EXPECTED: a session in CI whose suite is held until
# the sampler has looked at the disk, then ended by HOW: `finishes` lets the suite
# end with STATUS, and a signal goes to the session itself. It starts under job
# control because a background job of a script otherwise ignores INT.
session_ends() {
    name="$1"
    local suite_status="$2" how="$3" expected="$4" pid n looks before after tick
    state="$tmp/state-$name"
    mkdir -p "$state/machine" "$state/tmp"
    set -m
    (
        cd "$repo" &&
            exec env PATH="$stubs:$PATH" GS_STATE="$state" GS_QEMU=gone GS_GUEST=refused \
                GS_STATUS=x GS_IO=x FS_BCACHEFS_GUEST_ANSWER_LIMIT=2 \
                GS_DF_BEFORE=30000000 GS_DF_AFTER=29000000 CI=true \
                GS_PEAK=25000000 GS_HOLD=1 GS_TEARDOWN_HOLD=0.5 GS_REAL_SLEEP="$REAL_SLEEP" \
                TMPDIR="$state/tmp" \
                bash scripts/guest-session.sh "$tmp/suite" "$suite_status"
    ) </dev/null >"$state/out" 2>&1 &
    pid=$!
    set +m

    n=0
    while [ "$n" -lt 150 ] && [ ! -e "$state/held" ]; do
        "$REAL_SLEEP" 0.1
        n=$((n + 1))
    done
    [ -e "$state/held" ] || fail "$name: the suite never ran"
    looks="$(($(wc -l <"$state/looks-at-peak" 2>/dev/null)))"
    [ "$looks" -ge 2 ] ||
        fail "$name: nothing looked at the disk while the suite ran ($looks looks): there is no sampler to stop"

    # A sampler stopped now is in the middle of a wait: that wait goes with it.
    if [ "$how" != KILL ]; then
        : >"$state/long-ticks"
        "$REAL_SLEEP" 0.3
    fi

    # In one group, whose stderr takes this shell's notice of a job killed.
    {
        case "$how" in
            finishes) ;;
            *) kill -s "$how" "$pid" ;;
        esac
        : >"$state/release"

        n=0
        while [ "$n" -lt "$((WATCHDOG * 10))" ] && kill -0 "$pid"; do
            "$REAL_SLEEP" 0.1
            n=$((n + 1))
        done
        [ "$n" -lt "$((WATCHDOG * 10))" ] || kill -s KILL "$pid"
        wait "$pid"
    } 2>/dev/null
    rc=$?
    [ "$n" -lt "$((WATCHDOG * 10))" ] || fail "$name: the session did not end within ${WATCHDOG}s"
    out="$(cat "$state/out")"

    status_is "$expected"
    nothing_inherited

    if [ "$how" = KILL ]; then
        # Nothing can stop the sampler for a session that is killed outright: it
        # sees that for itself, one wait later.
        none_left_within 30
    else
        tail -n 1 "$state/order" | grep -qx "torn down" ||
            fail "$name: the guest was not torn down last: $(tr '\n' ',' <"$state/order")"
        # The sampler is gone before the harness starts to take the guest down.
        read -r before after <"$state/looks-in-teardown" ||
            fail "$name: the teardown recorded nothing"
        [ "${before:-}" = "${after:-}" ] ||
            fail "$name: the sampler looked at the disk during the teardown ($before, then $after looks)"
        none_left_within 5
    fi
    no_temporary_files

    before="$(($(wc -l <"$state/asked-df")))"
    "$REAL_SLEEP" 0.3
    after="$(($(wc -l <"$state/asked-df")))"
    [ "$before" = "$after" ] ||
        fail "$name: the disk is still being looked at after the session ended ($before, then $after looks)"

    # Often enough to see a peak: a job writes at most some hundred MB a second.
    tick="$(head -n 1 "$state/ticks" 2>/dev/null)"
    awk -v t="${tick:-0}" 'BEGIN { exit !(t + 0 > 0 && t + 0 <= 5) }' ||
        fail "$name: the sampler waits '${tick:-nothing}' between looks at the disk, over 5 seconds"
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
    says "the harness's own report of this loss, with the overlay's size, is in the tier's log"
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

# The overlay gives back what the guest deletes, so the budget is judged against
# the lowest free space the run saw. Over the budget at the peak and under it at
# the end is the run that #530 is about: on a smaller runner it would have stopped.
ci=true
peak=15000000
scenario peak-over-budget 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 66
says "guest-session: FAILED: this run used 14648 MB of the host's disk, over its budget of"
says "held 976 MB"
says "14 GB"
tail -n 1 "$state/order" | grep -qx "torn down" || fail "$name: the guest was not torn down last"

# A failed test and a stopped guest stay what they were, and the guest's report
# gives the peak.
scenario peak-over-budget-and-failed 1 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 1
silent "guest-session: FAILED"

scenario peak-over-budget-and-lost 255 gone refused "an ordinary line" x x
lost
says "this run used 14648 MB of the host's disk"
says "held 976 MB"

# Under the budget at the peak, the peak is the number a pass reports.
peak=25000000
scenario peak-under-budget 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 0
says "guest-session: this run used 4882 MB of the host's disk; its budget is"
says "held 976 MB"

# A developer's machine is told the peak, and is not judged by it.
ci=
peak=15000000
scenario peak-over-budget-off-ci 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 0
silent "guest-session: FAILED"
says "guest-session: this run used 14648 MB of the host's disk; its budget is"
peak=

# A sampler that stopped early saw some of the run, so what it saw is not the low
# point. A run that cannot be judged says so, and in CI it does not pass (#530).
ci=true
peak=25000000
kill_sampler=1
scenario sampler-lost 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 66
says "guest-session: FAILED: this run's use of the host's disk could not be judged"
scenario sampler-lost-and-failed 1 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 1
silent "guest-session: FAILED"
scenario sampler-lost-and-guest-lost 255 gone refused "an ordinary line" x x
lost
says "could not be judged"
ci=
scenario sampler-lost-off-ci 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 0
says "this run's use of the host's disk could not be judged"
silent "guest-session: FAILED"
kill_sampler=
peak=

# As is a run whose free space could not be read at all.
ci=true
df_before=unreadable
df_after=unreadable
scenario df-unreadable 0 alive answers "a line" "VM status: running" "I/O status: ok"
status_is 66
says "guest-session: FAILED: this run's use of the host's disk could not be judged"
df_before=30000000
df_after=29000000
ci=

# The sampler stops with the session, whichever way the session ends, and before
# the guest is taken down. A session killed outright cannot stop it, so it stops
# itself: a sampler that outlived a run would look at the disk for ever.
session_ends ends-passing 0 finishes 0
session_ends ends-failing 1 finishes 1
session_ends ends-losing-the-guest 255 finishes 255
says "this run used 4882 MB of the host's disk"
session_ends ends-on-term 0 TERM 143
session_ends ends-on-int 0 INT 130
session_ends ends-on-hup 0 HUP 129
session_ends ends-killed 0 KILL 137

# The guest suite goes through it, and CI uploads where the console is kept.
sed -n '/^  test:vm:$/,/^  [a-z:_-]*:$/p' "$REPO/chores.yml" | grep -qF 'scripts/guest-session.sh' ||
    fail "chores.yml: test:vm does not run through scripts/guest-session.sh"
sed -n '/^  suite-in-vm:$/,/^  cli:$/p' "$REPO/.github/workflows/ci.yml" | grep -qF 'path: tmp/logs/*.log' ||
    fail "ci.yml: suite-in-vm does not upload tmp/logs/*.log, where the console is kept"

[ "$fails" -eq 0 ] || exit 1
echo "guest-session-names-what-stopped: all checks passed"
