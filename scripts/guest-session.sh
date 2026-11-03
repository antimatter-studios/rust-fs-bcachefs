#!/usr/bin/env bash
#
# guest-session.sh COMMAND [ARGS...] -- run a command that uses the harness
# guest, and when the connection to the guest is lost, say what stopped the
# guest and keep its console.
#
# `vm.sh guest-test` runs the suite over ssh, whose own failures are status 255.
# Vagrant's ssh settings carry `LogLevel FATAL`, so ssh says nothing about them,
# and the teardown that follows removes what could have told why. Runs
# 41598439077 and 41608993199 ended that way: a tier log stopping in the middle
# of a test binary, then `==> default: Stopping the instance...`, which is that
# teardown and not the cause (#504).
#
# This script is a session of the harness (vm-session.sh, sourced), so the guest
# outlives COMMAND until the report is made: the harness brings a guest down
# only when the last session using it ends. The report is on stderr, outside
# the tier's quiet log, and it looks at, in order:
#
#   QEMU's process     gone: the guest powered itself off if its console says so
#   QEMU's monitor     paused: a write failed, which a full host disk causes
#   the guest          no answer within the limit: hung or crashed
#
# A guest that does answer leaves the connection itself, or a suite that exited
# 255 by itself: ssh cannot tell those apart, so the report says both.
#
# The console goes to tmp/logs/vm-console.log, which CI uploads with the tier
# logs. It is kept on this failure only. COMMAND's status is this script's.
# FS_BCACHEFS_GUEST_ANSWER_LIMIT is how many seconds the guest gets to answer.
#
# WHAT STOPPED THOSE RUNS. A boot writes to a disposable overlay on the host's
# disk, which grows with what the guest writes and does not shrink when the guest
# deletes it. About one job in fifty lands on a runner with 14 GB free instead of
# 86 GB (run 41620234394: m8id.xlarge, AWS). There the overlay filled the disk,
# QEMU paused the guest with `VM status: paused (io-error)`, and ssh gave up two
# minutes later. A run that would not fit passes on every other runner, so in CI
# (CI=true) a run that passes and used more of the disk than DISK_BUDGET_MB fails
# with status 66, as a tier over its output budget does. A developer's machine is
# not judged, because other work moves its free space.
set -uo pipefail

[ "$#" -gt 0 ] || { echo "usage: guest-session.sh COMMAND [ARGS...]" >&2; exit 64; }

HARNESS_SCRIPTS=../fs-linux-test-harness/scripts
VM="$HARNESS_SCRIPTS/vm.sh"
CONSOLE_KEPT=tmp/logs/vm-console.log
ANSWER_LIMIT="${FS_BCACHEFS_GUEST_ANSWER_LIMIT:-30}"
MONITOR_LIMIT=5
CONSOLE_TAIL_LINES=20
GUEST_LOG_LINES=20
HOST_LOG_LINES=15
# MEASURED on the 14 GB runner of run 41620234394: 11899 MB free when the session
# began, which leaves 1659 MB for what the job writes outside the session and for
# variation between runner images. Run 41638391605 used 9462 MB.
DISK_BUDGET_MB=10240

say() { echo "guest-session: $*" >&2; }

free_kb() { df -Pk "$PWD" 2>/dev/null | awk 'NR == 2 { print $4 }'; }

# What this run has used of the host's disk: the overlay only grows until the
# session ends, so the free space now is the least it has been.
disk_used_mb() {
    local now
    now="$(free_kb)"
    [ -n "$free_before" ] && [ -n "$now" ] || return 1
    echo $(((free_before - now) / 1024))
}

# bounded SECONDS OUTPUT COMMAND...: run a command with its output in a file, and
# stop it and everything it started at the limit. macOS has no timeout(1). A
# stopped command's status is 124, as timeout(1) gives. Job control gives the
# command a process group of its own: ssh, started by vm.sh, is in it too.
bounded() {
    local limit="$1" output="$2" pid watcher status
    shift 2
    set -m
    "$@" >"$output" 2>&1 &
    pid=$!
    set +m
    (
        n=0
        while [ "$n" -lt $((limit * 10)) ]; do
            sleep 0.1
            n=$((n + 1))
        done
        kill -s KILL -- "-$pid" 2>/dev/null || kill -s KILL "$pid" 2>/dev/null
    ) >/dev/null 2>&1 &
    watcher=$!
    wait "$pid" 2>/dev/null
    status=$?
    kill "$watcher" 2>/dev/null
    wait "$watcher" 2>/dev/null
    [ "$status" -ne 137 ] || status=124
    return "$status"
}

# What the host has, in one line: the pool a runner came from is the first thing
# a reader of a failure wants, and the job log of a pass is its comparison.
host_line() {
    local vendor="" free_disk memory="" kilobytes
    vendor="$(cat /sys/class/dmi/id/sys_vendor 2>/dev/null || true)"
    free_disk="$(df -h "$PWD" 2>/dev/null | awk 'NR == 2 { print $4 }')"
    if [ -r /proc/meminfo ]; then
        kilobytes="$(awk '/^MemAvailable:/ { print $2 }' /proc/meminfo)"
        memory=", $((kilobytes / 1024)) MB memory available"
    fi
    say "host: ${vendor:-$(uname -s)}, ${free_disk:-unknown} free on the working disk$memory"
}

# The pid and command line of this machine's QEMU, as the harness's engine_alive
# finds it: by process name, and by the machine's directory on its command line.
# Status 1 when there is none, 2 when the process table cannot be read.
qemu_process() {
    local identity="$1" procs
    procs="$(ps -eo pid=,args= 2>/dev/null)" || return 2
    [ -n "$procs" ] || return 2
    # awk reads to the end: stopping at the match would fail printf under pipefail.
    printf '%s\n' "$procs" | awk -v want="$identity/" '
        { n = split($2, parts, "/") }
        !found && parts[n] ~ /^qemu-system-/ && index($0, want) > 0 { print; found = 1 }
        END { exit !found }'
}

# `VM status: ...` and `I/O status: ...` from QEMU's monitor, a readline monitor
# on a socket that the provider names on QEMU's command line.
monitor_answer() {
    local process="$1" answer="$2" socket
    socket="$(printf '%s\n' "$process" | sed -n 's/.*id=mon0,path=\([^, ]*\).*/\1/p')"
    [ -n "$socket" ] || return 1
    bounded "$MONITOR_LIMIT" "$answer" bash -c \
        'printf "info status\ninfo block\n" | nc -w 1 -U "$1"' monitor "$socket" || return 1
    grep -aqo 'VM status: [a-z]' "$answer"
}

keep_console() {
    local machine="$1"
    if [ -s "$machine/console.log" ]; then
        mkdir -p "$(dirname "$CONSOLE_KEPT")"
        cp "$machine/console.log" "$CONSOLE_KEPT" && return 0
    fi
    return 1
}

report_lost_guest() {
    local config machine identity scratch process="" alive=0 console=0
    local vm_status="" io_status="" monitor=0

    say "FAILED: the connection to the guest was lost (exit 255); no test failed by this."

    config="$("$VM" config 2>/dev/null)"
    machine="$(printf '%s\n' "$config" | sed -n 's/^machine=//p')"
    identity="$(printf '%s\n' "$config" | sed -n 's/^identity=//p')"
    scratch="$(mktemp -d "${TMPDIR:-/tmp}/guest-session.XXXXXX")"

    process="$(qemu_process "$identity")" || alive=$?
    keep_console "$machine" && console=1

    if [ "$alive" -eq 0 ] && monitor_answer "$process" "$scratch/monitor"; then
        monitor=1
        vm_status="$(tr -d '\r' <"$scratch/monitor" | grep -ao 'VM status: .*' | head -n 1)"
        vm_status="${vm_status#VM status: }"
        io_status="$(tr -d '\r' <"$scratch/monitor" | grep -aoE 'I/O status: [a-z]+' | grep -v ': ok$' | head -n 1)"
    fi

    if [ "$alive" -eq 2 ]; then
        say "what stopped it: unknown; the process table could not be read."
    elif [ "$alive" -ne 0 ]; then
        if [ "$console" = 1 ] && grep -aqE 'Power down|System halted' "$CONSOLE_KEPT"; then
            say "what stopped it: the guest powered itself off; QEMU then ended."
        else
            say "what stopped it: QEMU ended and the guest's console records no power-off: it was killed or it crashed."
        fi
    elif [ "$monitor" = 1 ] && [ "${vm_status#paused}" != "$vm_status" ]; then
        say "what stopped it: QEMU paused the guest: $vm_status${io_status:+, $io_status}."
    else
        # Asked in every case but those: a paused guest cannot answer and a guest
        # with no QEMU has nothing to answer with.
        if bounded "$ANSWER_LIMIT" "$scratch/guest" "$VM" exec \
            "echo \"up \$(cut -d. -f1 /proc/uptime) s\"; free -m; df -h /; dmesg | tail -n $GUEST_LOG_LINES"; then
            say "what stopped it: not the guest: the guest is up and answers, so ssh lost its connection, or the suite itself exited 255."
            sed 's/^/guest-session: guest: /' "$scratch/guest" >&2
        else
            say "what stopped it: QEMU is running the guest and the guest does not answer, within ${ANSWER_LIMIT}s: it hung or it crashed."
        fi
        [ "$monitor" = 1 ] || say "QEMU's monitor did not answer."
        [ -z "$vm_status" ] || say "QEMU's monitor says: $vm_status${io_status:+, $io_status}."
    fi

    host_line
    [ -z "$used_mb" ] || say "this run used $used_mb MB of the host's disk; its budget is $DISK_BUDGET_MB MB."
    df -h "$PWD" 2>/dev/null | sed 's/^/guest-session: host: /' >&2
    if command -v free >/dev/null 2>&1; then
        free -m | sed 's/^/guest-session: host: /' >&2
    fi
    if sudo -n dmesg >"$scratch/host-log" 2>/dev/null; then
        tail -n "$HOST_LOG_LINES" "$scratch/host-log" | sed 's/^/guest-session: host log: /' >&2
    fi

    if [ "$console" = 1 ]; then
        say "the guest's console is kept in $CONSOLE_KEPT; its last lines:"
        tail -n "$CONSOLE_TAIL_LINES" "$CONSOLE_KEPT" | sed 's/^/guest-session: console: /' >&2
    else
        say "the guest wrote no console to keep (looked in ${machine:-an unnamed machine directory})."
    fi
    rm -rf -- "$scratch"
}

free_before="$(free_kb)"
host_line

# Sourced, this script is the session: an EXIT trap brings the guest down,
# whatever happens below, and keeps the status it exits with.
# shellcheck source=/dev/null
. "$HARNESS_SCRIPTS/vm-session.sh" || exit 1

status=0
"$@" || status=$?

used_mb="$(disk_used_mb)" || used_mb=

if [ "$status" -eq 255 ]; then
    report_lost_guest
elif [ -n "$used_mb" ] && [ "$status" -eq 0 ]; then
    if [ "${CI:-}" = true ] && [ "$used_mb" -gt "$DISK_BUDGET_MB" ]; then
        say "FAILED: this run used $used_mb MB of the host's disk, over its budget of $DISK_BUDGET_MB MB."
        say "It passed here, but a runner with 14 GB free would have stopped the guest: find what the guest writes."
        status=66
    else
        say "this run used $used_mb MB of the host's disk; its budget is $DISK_BUDGET_MB MB."
    fi
fi
exit "$status"
