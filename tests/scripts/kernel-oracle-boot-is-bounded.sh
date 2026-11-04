#!/usr/bin/env bash
# A kernel-oracle boot ends at its limit even when QEMU does not (#194). A
# guest hung on #190's fixtures job, and the boot after it outlived its
# `timeout 120` by ten minutes, until the job's own 35-minute limit
# cancelled it and nothing was uploaded: SIGTERM alone does not stop a
# process that ignores it. Here a stand-in for QEMU ignores SIGTERM, and
# `boot` (scripts/kernel-oracle-boot.sh) must still return within seconds.
#
#   bash tests/scripts/kernel-oracle-boot-is-bounded.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

# The host's own timeout(1) wins; tests/scripts/support is for a host with none.
export PATH="$PATH:$REPO/tests/scripts/support"
command -v timeout >/dev/null || {
    fail "timeout(1) is missing: this test and kernel-oracle.sh need GNU coreutils', or tests/scripts/support/timeout"
    exit 1
}

tmp="$(mktemp -d "${TMPDIR:-/tmp}/ko-boot.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

# A QEMU that ignores SIGTERM, as the hung guest's did. An ignored signal
# stays ignored across exec, so its sleep ignores SIGTERM too.
cat >"$tmp/qemu" <<'EOF'
#!/usr/bin/env bash
trap '' TERM
echo started >"$(dirname "$0")/started"
sleep 20
EOF
chmod +x "$tmp/qemu"

# The boot under a watchdog of its own, so a boot that does not end fails
# this test rather than hanging it.
#
# GNU timeout(1) sends its SIGKILL to its own process group, itself included.
# The shell running `boot` then reaps a job a signal killed and reports it on
# that shell's stderr, with the checkout's path in front (#559). This test
# expects that kill, so the wrapper below discards that one shell's stderr
# from the moment timeout(1) starts. It stays discarded because Bash 3.2 makes
# the report at a later command. timeout(1) and the stand-in keep the real
# stderr, and so does everything the shell does before timeout(1) starts. It
# goes through env(1) because the redirection on a builtin, `command` too,
# applies to this shell, and Bash 3.2 then made the report on the real stderr.
start=$SECONDS
# shellcheck disable=SC2016 # expanded by the inner shell, from its arguments
timeout -s KILL 10 bash -c '
    source "$1/scripts/kernel-oracle-boot.sh"
    timeout() {
        exec 3>&2 2>/dev/null
        env timeout "$@" 2>&3 3>&-
    }
    KERNEL_ORACLE_QEMU="$2/qemu" KERNEL_ORACLE_KILL_AFTER=1 boot 1 -nographic
' _ "$REPO" "$tmp"
status=$?
took=$((SECONDS - start))

if [ -f "$tmp/started" ]; then ok "the stand-in QEMU ran"; else fail "the stand-in QEMU never ran"; fi
if [ "$took" -ge 8 ]; then
    fail "boot ran ${took}s on a 1-second limit, until the watchdog killed it: a guest that ignores SIGTERM is never stopped"
else
    ok "boot returned in ${took}s"
    case "$status" in
        124 | 137) ok "boot returned $status, a guest stopped at its limit" ;;
        *) fail "boot returned $status, not the status of a guest stopped at its limit" ;;
    esac
fi

if [ "$fails" -gt 0 ]; then
    echo "kernel-oracle-boot-is-bounded: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  a kernel-oracle boot ends at its limit when QEMU ignores SIGTERM"
