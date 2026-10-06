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
# disk and installs the pinned toolchain; then the unit and oracle tiers'
# tests run, exactly the targets `chore test:unit` and `chore test:oracle`
# name. The fixtures are the ones on the share (.vm-share/fixtures).
set -euo pipefail

[ "${FLTH_GUEST:-}" = 1 ] ||
    { echo "guest-suite.sh runs INSIDE the harness VM ('chore test:vm')." >&2; exit 1; }

cd /repo
FS_CORE_ROOT=/share/siblings/rust-fs-core exec bash /share/siblings/rust-fs-core/scripts/core.sh \
    guest-rust-run fs-bcachefs /share rust-fs-core -- bash -c '
        set -euo pipefail
        EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib --test fuzz_decoders "$@"
        cargo test --locked --release --test oracle_superblock --test oracle_tree \
            --test oracle_fs --test capi "$@"
    ' guest-suite "$@"
