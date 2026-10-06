#!/usr/bin/env bash
# check-semver.sh -- `chore check:semver`: refuse a public-API break the
# version in Cargo.toml does not declare.
#
# ONCE THIS CRATE IS ON CRATES.IO the baseline is its newest release, and
# this is rust-fs-core's semver-check.sh, run in place.
#
# UNTIL THEN there is no release to compare with, and the family script
# refuses an unpublished crate (it has no former name to fall back on). The
# baseline is then main: a pull request that breaks the API main has must
# move the version in Cargo.toml with it, so the first release's version
# says what changed since the last time anyone could have depended on it.
# That is a real check, not a skip; it needs the history (CI checks out
# with fetch-depth 0).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"
crate="$(sed -n 's/^name *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)"
UA="rust-fs-bcachefs check-semver (github.com/antimatter-studios/rust-fs-bcachefs)"
code="$(curl -s -o /dev/null -w '%{http_code}' -A "$UA" "https://crates.io/api/v1/crates/$crate")"
case "$code" in
    200) exec bash ../rust-fs-core/scripts/semver-check.sh ;;
    404) ;;
    *) echo "check-semver: crates.io answered $code for $crate; cannot tell whether it is published" >&2; exit 1 ;;
esac

git fetch --quiet origin main 2>/dev/null || true
base="$(git merge-base HEAD origin/main 2>/dev/null)" ||
    { echo "check-semver: no merge base with origin/main (a shallow clone? fetch the history)" >&2; exit 1; }
command -v cargo-semver-checks >/dev/null 2>&1 || cargo semver-checks --version >/dev/null 2>&1 ||
    { echo "check-semver: cargo-semver-checks is not installed: cargo install cargo-semver-checks --locked --version ${CARGO_SEMVER_CHECKS_VERSION:-0.50.0}" >&2; exit 1; }
# The baseline is main's tree, unpacked beside a link to the same
# rust-fs-core this tree builds against: its path dependency names
# ../rust-fs-core, which a --baseline-rev checkout inside target/ cannot
# resolve.
scratch="$(mktemp -d "${TMPDIR:-/tmp}/bcachefs-semver.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
mkdir "$scratch/baseline"
git archive "$base" | tar -x -C "$scratch/baseline"
ln -s "$(cd ../rust-fs-core && pwd -P)" "$scratch/rust-fs-core"
echo "check-semver: $crate is unpublished; against main at ${base:0:12}"
cargo semver-checks check-release --package "$crate" --baseline-root "$scratch/baseline"
