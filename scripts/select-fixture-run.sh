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

candidates="$("$gh" run list --repo "$repo" --workflow ci.yml --branch main --status success \
  --limit "$window" --json databaseId --jq '.[].databaseId')"
for candidate in $candidates; do
  artifact="$(artifact_of "$candidate")"
  if [ -n "$artifact" ]; then echo "$candidate $artifact"; exit 0; fi
  echo "run $candidate has no unexpired fixtures artifact; not a fixture producer" >&2
done
echo "none of the last $window green main CI runs holds an unexpired fixtures artifact" >&2
exit 1
