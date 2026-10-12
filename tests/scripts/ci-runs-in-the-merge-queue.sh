#!/usr/bin/env bash
# ci.yml must trigger on merge_group (#130). main merges through a merge
# queue, which reads the required ci-ok check only from a workflow that runs
# on the queue's own event: drop the trigger and every queued pull request
# waits for a check that never comes, so nothing merges. It must keep
# pull_request too, or pull requests are never checked before they queue.
#
#   bash tests/scripts/ci-runs-in-the-merge-queue.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CI="$REPO/.github/workflows/ci.yml"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

# The events a workflow file triggers on: the keys of its top-level `on:` map.
events() {
    awk '
        /^on:[[:space:]]*$/ { inside = 1; next }
        inside && /^[^[:space:]#]/ { exit }
        inside && /^  [a-z_]+:/ { sub(/^  /, ""); sub(/:.*/, ""); print }
    ' "$1"
}

has() { events "$1" | grep -qx "$2"; }

[ -f "$CI" ] || { fail ".github/workflows/ci.yml is missing"; exit 1; }
if has "$CI" merge_group; then ok "ci.yml triggers on merge_group"; else fail "ci.yml does not trigger on merge_group: nothing in the merge queue can merge"; fi
if has "$CI" pull_request; then ok "ci.yml triggers on pull_request"; else fail "ci.yml does not trigger on pull_request"; fi

# The guard has to be able to fail: the same file without the trigger.
tmp="$(mktemp "${TMPDIR:-/tmp}/ci-mq.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
awk '!/^  merge_group:/' "$CI" >"$tmp"
if has "$tmp" merge_group; then fail "the check still finds merge_group in a copy without it"; else ok "a ci.yml without merge_group is caught"; fi

if [ "$fails" -gt 0 ]; then
    echo "ci-runs-in-the-merge-queue: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  ci.yml runs on pull_request and in the merge queue"
