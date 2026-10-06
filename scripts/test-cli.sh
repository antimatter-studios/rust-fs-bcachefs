#!/usr/bin/env bash
# test-cli.sh -- THE `cli` TIER: the command-line tools as installed, found
# on PATH, not a cargo target.
#
# It tests whatever PATH resolves, because that is what a user runs:
# `chore cli:install` stages a checkout's build and prints the PATH line.
#
# STEP 1, BEFORE ANY TOOL IS TESTED: the tools are present and are ours.
#   - jq and rust-fs-bcachefs must resolve, and rust-fs-bcachefs must answer
#     --version as this crate;
#   - `rust-fs-bcachefs doctor` must pass: every dotted name on PATH is our
#     program at our version. If one is shadowed the tier fails with
#     doctor's report: what wins, and the fix.
# NOTHING SKIPS. A missing tool fails the tier naming what provides it.
#
# STEP 2: every tests/cli/test-*.sh, by glob. Each prints its failures, a
# `test result: ok. N passed; ...` line (the count
# ../rust-fs-core/scripts/test-floor.sh reads, as it does cargo's), and LAST
# `<name>: all checks passed`. A file that exits 0 without that last line
# stopped early and is a failure.
#
# Quiet: the tier runs under ../rust-fs-core/scripts/tier.sh, which keeps
# the whole run in tmp/logs/cli.log.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
TESTS="${CLI_TESTS:-$REPO/tests/cli}"
CRATE="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -n 1)"
INSTALL="build and stage it with \`chore cli:install\` (it prints the PATH line to use)"

refuse() {
    echo "test-cli: $*" >&2
    exit 1
}

command -v jq >/dev/null 2>&1 ||
    refuse "jq is not on PATH; the suite reads every JSON report with it. Install it: \`brew install jq\`, or \`apt-get install jq\`."

entry="$(command -v rust-fs-bcachefs 2>/dev/null || true)"
[ -n "$entry" ] || refuse "rust-fs-bcachefs is not on PATH: $INSTALL."
answer="$(rust-fs-bcachefs --version 2>/dev/null || true)"
case "$answer" in
    "rust-fs-bcachefs ($CRATE) "*) ;;
    *) refuse "$entry is not $CRATE's rust-fs-bcachefs: --version answered '$answer'. Put ours first on PATH: $INSTALL." ;;
esac
echo "== $answer, at $entry"

echo "== rust-fs-bcachefs doctor"
if ! report="$(rust-fs-bcachefs doctor --text 2>&1)"; then
    printf '%s\n' "$report" >&2
    refuse "doctor found a tool on PATH that is not this program; its fixes are above. Nothing was tested."
fi
printf '%s\n' "$report"

shopt -s nullglob
files=("$TESTS"/test-*.sh)
[ "${#files[@]}" -gt 0 ] || refuse "no test-*.sh in $TESTS to run"

failed=0
for file in "${files[@]}"; do
    name="$(basename "$file" .sh)"
    echo "== $name"
    output="$(bash "$file" 2>&1)"
    status=$?
    printf '%s\n' "$output"
    last="$(printf '%s\n' "$output" | tail -n 1)"
    if [ "$status" -ne 0 ]; then
        echo "FAIL  $name exited $status" >&2
        failed=$((failed + 1))
    elif [ "$last" != "$name: all checks passed" ]; then
        echo "FAIL  $name exited 0 without its last line, '$name: all checks passed': it stopped early" >&2
        failed=$((failed + 1))
    fi
done

if [ "$failed" -gt 0 ]; then
    echo "test-cli: $failed of ${#files[@]} files failed" >&2
    exit 1
fi
echo "test-cli: ${#files[@]} files, all checks passed"
