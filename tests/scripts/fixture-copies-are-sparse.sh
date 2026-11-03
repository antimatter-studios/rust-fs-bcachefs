#!/usr/bin/env bash
# A test copies an image with common::copy_sparse, never std::fs::copy (#504).
#
# A fixture is mostly zeros: default.img is 64 MiB and holds 8 MiB. std::fs::copy
# writes the zeros too, and in the guest every block it writes reaches the
# disposable overlay on the host, which does not shrink when the copy is deleted.
# Probe run 41620234394 saw that overlay reach 9.7 GB on a runner with 86 GB free,
# and fill the disk of a runner with 14 GB free, where QEMU paused the guest and
# `chore test:vm` ended with exit 255.
#
#   bash tests/scripts/fixture-copies-are-sparse.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

full_copies="$(cd "$REPO" && grep -rnE '\bfs::copy\b' tests --include='*.rs')"
if [ -n "$full_copies" ]; then
    fail "tests copy images in full; use common::copy_sparse:"$'\n'"$full_copies"
fi

grep -qE '^pub fn copy_sparse\b' "$REPO/tests/common/mod.rs" ||
    fail "tests/common/mod.rs has no copy_sparse"

# The guard fails on what it looks for, in a sandbox of its own.
tmp="$(mktemp -d "${TMPDIR:-/tmp}/fixture-copies.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/tests"
echo 'std::fs::copy(fixture("default.img"), &img).unwrap();' >"$tmp/tests/a.rs"
(cd "$tmp" && grep -rqE '\bfs::copy\b' tests --include='*.rs') ||
    fail "the pattern does not find a full copy"

[ "$fails" -eq 0 ] || exit 1
echo "fixture-copies-are-sparse: all checks passed"
