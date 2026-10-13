# shellcheck shell=bash
# kernel-oracle-boot.sh -- sourced by kernel-oracle.sh. `boot SECONDS
# ARGS...` runs QEMU (KERNEL_ORACLE_QEMU, else qemu-system-x86_64) with
# ARGS for at most SECONDS, and returns its status: 124 for a guest stopped
# at the limit.
boot() {
    local seconds="$1"
    shift
    timeout "$seconds" "${KERNEL_ORACLE_QEMU:-qemu-system-x86_64}" "$@"
}
