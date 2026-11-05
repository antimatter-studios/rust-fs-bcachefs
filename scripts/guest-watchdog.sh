# guest-watchdog.sh -- sourced by the guest fixture scripts, which run as
# root INSIDE the harness VM.
#
# Every step of the fixture build announces itself with `step NAME
# [SECONDS]` and must reach the next step within SECONDS, five minutes
# unless it says otherwise. A step that writes through the reference mount
# takes under a minute (aging the `aged` image, the longest, took 50 seconds
# in a green build), so five minutes is ample. A reference mount that hangs
# can leave a process the kernel will not let die, and the build once held
# its job for the full 90 minutes six times over (#94). Instead, the
# watchdog prints the stuck step, every process in uninterruptible sleep and
# the kernel log's last lines, then powers the guest off at once: the
# session ends, the host sees the failure within seconds, and the guest's
# disk is never written by a run, so nothing is lost.
#
# ON A SHARED GUEST it stops only this run (fs-linux-test-harness#157). Other
# suites are using that guest, and powering it off would end them all. The harness
# sets FLTH_RUN_ROOT in every run on one. There the watchdog ends what the build
# started, and the build with it, and the build fails naming the step. A process
# the kernel will not let die stays; ending the guest for that is the harness's own
# watchdog's to decide, for every suite at once.

WATCHDOG_STEP="${GUEST_WATCHDOG_STEP:-/run/fixture-step}"

step() { # NAME [SECONDS]
    echo "== $1"
    echo "$(($(date +%s) + ${2:-300})) $1" >"$WATCHDOG_STEP"
}

# Started once, by the first script that sources this; a script it runs
# inherits WATCHDOG_PID and shares the same watchdog.
# The build and everything it started, the watchdog left out, ended. One look at the
# process table, followed down from the build, so the look's own processes and the
# watchdog's are not chased.
watchdog_end_build() { # OWNER SELF
    local pids
    pids="$(ps -A -o pid=,ppid= | awk -v owner="$1" -v self="$2" '
        { parent[$1] = $2 }
        END {
            keep[owner] = 1
            do {
                grew = 0
                for (pid in parent) if (!(pid in keep) && (parent[pid] in keep) && pid != self) { keep[pid] = 1; grew = 1 }
            } while (grew)
            for (pid in keep) printf "%s ", pid
        }')"
    # shellcheck disable=SC2086  # a list of pids
    kill -TERM $pids 2>/dev/null || true
    sleep 2
    # shellcheck disable=SC2086
    kill -KILL $pids 2>/dev/null || true
}

if [ -z "${WATCHDOG_PID:-}" ]; then
    rm -f "$WATCHDOG_STEP"
    WATCHDOG_OWNER=$$
    (
        while sleep "${GUEST_WATCHDOG_POLL:-5}"; do
            read -r due name <"$WATCHDOG_STEP" 2>/dev/null || continue
            [ "$(date +%s)" -ge "$due" ] || continue
            if [ -n "${FLTH_RUN_ROOT:-}" ]; then
                what="so only this run is stopped: other suites share this guest"
            else
                what="so the guest is powered off"
            fi
            {
                echo "fixtures: step '$name' outlived its time, $what"
                echo "-- processes in uninterruptible sleep:"
                ps -eo pid,stat,etimes,wchan:32,args | awk 'NR == 1 || $2 ~ /^D/'
                echo "-- the kernel log, last lines:"
                dmesg | tail -n 30
            } >&2
            if [ -n "${FLTH_RUN_ROOT:-}" ]; then
                # The exec makes this subshell the parent, on a Bash with no BASHPID.
                watchdog_end_build "$WATCHDOG_OWNER" "$(exec sh -c 'echo "$PPID"')"
                exit 1
            fi
            echo 1 >"${GUEST_WATCHDOG_SYSRQ_ENABLE:-/proc/sys/kernel/sysrq}"
            echo o >"${GUEST_WATCHDOG_SYSRQ:-/proc/sysrq-trigger}"
            exit 1
        done
    ) &
    WATCHDOG_PID=$!
    export WATCHDOG_PID
    # It holds the session's output, so it must go when the build does.
    trap 'kill "$WATCHDOG_PID" 2>/dev/null || true' EXIT
fi
