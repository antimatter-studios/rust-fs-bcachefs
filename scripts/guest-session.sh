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
# THE HARNESS REPORTS THE SAME LOSS, since v0.5.6, on the stderr of `vm.sh exec`.
# Under a tier that goes to the tier's quiet log and nowhere else, so the two never
# meet: this one is what the job's log shows, and the harness's is one artifact
# away. This one stays for what only it has: the whole console, the guest's own
# answer, the host's memory and log, and the run's use of the disk. The harness's
# has the overlay's size and looks at the guest several times, and this one says
# where to find it (#530).
#
# WHAT STOPPED THOSE RUNS. A boot writes to a disposable overlay on the host's
# disk, which grows with what the guest writes and, before harness v0.5.6, did not
# shrink when the guest deleted it. About one job in fifty lands on a runner with
# 14 GB free instead of 86 GB (run 41620234394: m8id.xlarge, AWS). There the overlay
# filled the disk, QEMU paused the guest with `VM status: paused (io-error)`, and
# ssh gave up two minutes later. A run that would not fit passes on every other runner, so in CI
# (CI=true) a run that passes and used more of the disk than DISK_BUDGET_MB fails
# with status 66, as a tier over its output budget does. A developer's machine is
# not judged, because other work moves its free space.
#
# WHAT A RUN USED is what stops a guest: the most it held at once. The free space at
# the end cannot say that, because the overlay gives back what the guest deletes,
# so a sampler looks at the disk for as long as COMMAND runs and the budget is
# judged against the lowest free space seen (#530). A run whose low point is not
# known cannot be called under its budget, so in CI it fails the same way.
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
# variation between runner images. Run 41638391605 used 9462 MB, when the end of a
# run was its peak.
DISK_BUDGET_MB=10240
# Between looks at the disk. A peak that lasts less than this can be missed, by what
# a job writes in that time. A look is one df and one awk, every this many seconds.
SAMPLE_SECONDS=1

say() { echo "guest-session: $*" >&2; }

free_kb() { df -Pk "$PWD" 2>/dev/null | awk 'NR == 2 { print $4 }'; }

# In the background for as long as COMMAND runs: keeps the lowest free space seen in
# a file, whole, because the sampler can be stopped at any moment. It stops by itself
# when the session has gone without stopping it, which a SIGKILL does, within one
# wait, and takes its file with it because nobody is left to read it.
sample_free_space() {
    local now low="$free_before"
    while kill -0 "$$" 2>/dev/null; do
        now="$(free_kb)"
        if [ -n "$now" ] && [ "$now" -lt "$low" ]; then
            low="$now"
            printf '%s\n' "$low" >"$samples/low.new" && mv "$samples/low.new" "$samples/low"
        fi
        sleep "$SAMPLE_SECONDS"
    done
    forget_samples
}

forget_samples() {
    rm -f -- "$samples/low" "$samples/low.new"
    rmdir -- "$samples"
}

# Job control gives the sampler a process group of its own, as bounded() does for
# its command, so that stopping it stops the wait it is in. It holds none of the
# session's descriptors: a tier reads its command's output until the last holder
# of that pipe has gone.
start_sampling() {
    if [ -z "$free_before" ]; then
        not_judged="the free space could not be read"
        return 0
    fi
    samples="$(mktemp -d "${TMPDIR:-/tmp}/guest-session.XXXXXX")" || {
        not_judged="the sampler had nowhere to keep what it saw"
        return 0
    }
    set -m
    sample_free_space </dev/null >/dev/null 2>&1 &
    sampler=$!
    set +m
}

# Stops the sampler, once, and leaves what it saw in low_seen. A sampler already gone
# was not stopped by this, and has not seen the rest of the run.
stop_sampling() {
    [ -n "$sampler" ] || return 0
    {
        if kill -0 "$sampler"; then
            kill -s TERM -- "-$sampler" || kill -s TERM "$sampler"
        else
            not_judged="the sampler stopped before COMMAND did"
        fi
        wait "$sampler"
    } 2>/dev/null
    sampler=
    low_seen="$(cat "$samples/low" 2>/dev/null)"
    forget_samples
}

# Sets used_mb, the most this run held of the host's disk at once, and held_mb, what
# it still held when it ended. Neither is set for a run that cannot be judged.
judge_disk_use() {
    local now low
    [ -z "$not_judged" ] || return 0
    now="$(free_kb)"
    if [ -z "$now" ]; then
        not_judged="the free space could not be read"
        return 0
    fi
    low="${low_seen:-$free_before}"
    [ "$now" -ge "$low" ] || low="$now"
    used_mb=$(((free_before - low) / 1024))
    held_mb=$(((free_before - now) / 1024))
    [ "$held_mb" -ge 0 ] || held_mb=0
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
    if [ -n "$used_mb" ]; then
        say "this run used $used_mb MB of the host's disk; its budget is $DISK_BUDGET_MB MB. It held $held_mb MB when it ended."
    else
        say "this run's use of the host's disk could not be judged: $not_judged."
    fi
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
    say "the harness's own report of this loss, with the overlay's size, is in the tier's log."
    rm -rf -- "$scratch"
}

sampler=
samples=
low_seen=
not_judged=
used_mb=
held_mb=

free_before="$(free_kb)"
host_line

# Sourced, this script is the session: an EXIT trap brings the guest down,
# whatever happens below, and keeps the status it exits with.
# shellcheck source=/dev/null
. "$HARNESS_SCRIPTS/vm-session.sh" || exit 1

# The harness's own traps for these, with the sampler stopped first: an exit from
# here runs the harness's EXIT trap, which takes the guest down.
trap 'stop_sampling; exit 129' HUP
trap 'stop_sampling; exit 130' INT
trap 'stop_sampling; exit 143' TERM

start_sampling

status=0
"$@" || status=$?

stop_sampling
judge_disk_use

if [ "$status" -eq 255 ]; then
    report_lost_guest
elif [ "$status" -eq 0 ] && [ -z "$used_mb" ]; then
    if [ "${CI:-}" = true ]; then
        say "FAILED: this run's use of the host's disk could not be judged: $not_judged."
        status=66
    else
        say "this run's use of the host's disk could not be judged: $not_judged."
    fi
elif [ "$status" -eq 0 ]; then
    if [ "${CI:-}" = true ] && [ "$used_mb" -gt "$DISK_BUDGET_MB" ]; then
        say "FAILED: this run used $used_mb MB of the host's disk, over its budget of $DISK_BUDGET_MB MB. It held $held_mb MB when it ended."
        say "It passed here, but a runner with 14 GB free would have stopped the guest: find what the guest writes."
        status=66
    else
        say "this run used $used_mb MB of the host's disk; its budget is $DISK_BUDGET_MB MB. It held $held_mb MB when it ended."
    fi
fi
exit "$status"
