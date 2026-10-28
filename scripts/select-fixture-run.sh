#!/usr/bin/env bash
# Pick the CI run whose fixtures the differential run judges, and print
# "<run> <artifact id> <artifact digest>".
#
#   select-fixture-run.sh <owner/repo>                 newest green main run with fixtures
#   select-fixture-run.sh <owner/repo> <run> <digest>  that run, and only with that digest
#
# A green main run is not necessarily a fixture producer: a quick PR-shaped
# pass uploads no fixtures, and an old artifact expires. So the automatic
# choice walks a bounded window of green main runs, newest first, and takes the
# first one still holding an unexpired artifact named "fixtures". Running out of
# candidates fails, naming the window; it never falls back to no fixtures.
#
# Main merges through the merge queue, so a push to main does not rebuild the
# fixtures: the merge_group run that tested the commit did, and main was
# fast-forwarded to that run's head commit. A main run without fixtures of its
# own therefore defers to a green merge_group run at the same commit. Pairing by
# commit is what proves the fixtures reached main: a queue run for a commit no
# green main run carries (a group that was ejected, or not yet landed) is never
# a candidate.
# GH overrides the gh binary, so the controls can feed it a fake catalog.
set -euo pipefail
repo="$1"; run="${2:-}"; want="${3:-}"
gh="${GH:-gh}"; window="${FIXTURE_CANDIDATES:-30}"

artifact_of() {
  "$gh" api "repos/$repo/actions/runs/$1/artifacts" \
    --jq '.artifacts[] | select(.name == "fixtures" and .expired == false) | "\(.id) \(.digest)"' | head -n 1
}

if [ -n "$run" ]; then
  [ -n "$want" ] || { echo "fixture_run $run needs fixture_digest, so a report names the exact input" >&2; exit 1; }
  artifact="$(artifact_of "$run")"
  [ -n "$artifact" ] || { echo "run $run has no unexpired fixtures artifact" >&2; exit 1; }
  [ "${artifact#* }" = "$want" ] || { echo "run $run fixtures artifact is ${artifact#* }, not the expected $want" >&2; exit 1; }
  echo "$run $artifact"; exit 0
fi

candidates="$("$gh" run list --repo "$repo" --workflow ci.yml --branch main --event push --status success \
  --limit "$window" --json databaseId,headSha --jq '.[] | "\(.databaseId) \(.headSha)"')"
while read -r candidate commit; do
  [ -n "$candidate" ] || continue
  queued="$("$gh" run list --repo "$repo" --workflow ci.yml --event merge_group --commit "$commit" \
    --status success --json databaseId --jq '.[].databaseId')"
  for producer in "$candidate" $queued; do
    artifact="$(artifact_of "$producer")"
    if [ -n "$artifact" ]; then echo "$producer $artifact"; exit 0; fi
  done
  queued="${queued//$'\n'/ }"
  echo "run $candidate (commit $commit) and its merge-queue runs ${queued:-(none)} hold no unexpired fixtures artifact" >&2
done <<<"$candidates"
echo "none of the last $window green main CI runs, nor the merge-queue runs that tested their commits, holds an unexpired fixtures artifact" >&2
exit 1
