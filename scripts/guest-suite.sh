#!/usr/bin/env bash
#
# guest-suite.sh [cargo test args...] -- THE WHOLE SUITE, INSIDE THE VM.
#
# The fs-linux-test-harness [test] guest_command: `chore test:vm` boots the
# VM and runs this from /repo, where the harness mounts this repository.
# On a Mac this is how the Linux tests run, against the same sources and
# the same pinned toolchain.
#
# rust-fs-core's guest-rust-run, run in place from the staged sibling,
# links the siblings, puts the toolchain and the build on the guest's own
# disk and installs the pinned toolchain; then the unit tier's tests run in
# debug, and every oracle test target (by glob, so a new one needs no edit
# here) and the C ABI test in release. The fixtures are the ones on the
# share (.vm-share/fixtures).
set -euo pipefail

[ "${FLTH_GUEST:-}" = 1 ] ||
    { echo "guest-suite.sh runs INSIDE the harness VM ('chore test:vm')." >&2; exit 1; }

cd /repo
# FS_CORE_CALLER names this repository: run directly rather than through
# core's core.sh, the script would otherwise take core for the caller.
FS_CORE_CALLER=/repo exec bash /share/siblings/rust-fs-core/scripts/guest-rust-run.sh \
    fs-bcachefs /share rust-fs-core -- bash -c '
        set -euo pipefail
        EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib --test fuzz_decoders "$@"
        cargo test --locked --release --features write --test "oracle_*" --test capi "$@"
        # The checker against the reference checker, which only exists in here.
        cargo test --locked --release --test check_oracle "$@"
        # The write path, judged by the reference tools that only exist in
        # here; one test at a time, since they share one mount point.
        RUST_TEST_THREADS=1 cargo test --locked --release --features write --test write_oracle "$@"
    ' guest-suite "$@"
