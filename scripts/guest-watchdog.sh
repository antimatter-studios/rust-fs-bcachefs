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

WATCHDOG_STEP=/run/fixture-step

step() { # NAME [SECONDS]
    echo "== $1"
    echo "$(($(date +%s) + ${2:-300})) $1" >"$WATCHDOG_STEP"
}

# Started once, by the first script that sources this; a script it runs
# inherits WATCHDOG_PID and shares the same watchdog.
if [ -z "${WATCHDOG_PID:-}" ]; then
    rm -f "$WATCHDOG_STEP"
    (
        while sleep 5; do
            read -r due name <"$WATCHDOG_STEP" 2>/dev/null || continue
            [ "$(date +%s)" -ge "$due" ] || continue
            {
                echo "fixtures: step '$name' outlived its time, so the guest is powered off"
                echo "-- processes in uninterruptible sleep:"
                ps -eo pid,stat,etimes,wchan:32,args | awk 'NR == 1 || $2 ~ /^D/'
                echo "-- the kernel log, last lines:"
                dmesg | tail -n 30
            } >&2
            echo 1 >/proc/sys/kernel/sysrq
            echo o >/proc/sysrq-trigger
            exit 1
        done
    ) &
    WATCHDOG_PID=$!
    export WATCHDOG_PID
    # It holds the session's output, so it must go when the build does.
    trap 'kill "$WATCHDOG_PID" 2>/dev/null || true' EXIT
fi
