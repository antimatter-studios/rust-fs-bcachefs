# shellcheck shell=bash
# kernel-oracle-boot.sh -- sourced by kernel-oracle.sh. `boot SECONDS
# ARGS...` runs QEMU (KERNEL_ORACLE_QEMU, else qemu-system-x86_64) with
# ARGS for at most SECONDS, then sends SIGTERM, and SIGKILL
# KERNEL_ORACLE_KILL_AFTER seconds (15) later if it is still running: a hung
# guest once ignored SIGTERM and held the fixtures job until its own limit
# (#194). Returns QEMU's status: 124 for a guest stopped at the limit, 137
# for one that had to be killed.
boot() {
    local seconds="$1"
    shift
    timeout -k "${KERNEL_ORACLE_KILL_AFTER:-15}" "$seconds" \
        "${KERNEL_ORACLE_QEMU:-qemu-system-x86_64}" "$@"
}
