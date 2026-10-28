#!/usr/bin/env bash
# Controls for select-fixture-run.sh, over a fake gh that answers from a
# catalog directory: runs.json for `gh run list` (filtered by --event, --branch
# and --commit as GitHub would), artifacts-<run>.json for
# `gh api .../runs/<run>/artifacts`. Each case is graded on the exit status and
# on the exact line printed, so a selector that picks the wrong run fails.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
fails=0
ok() { echo "ok: $1"; }
fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }
sandbox="$(mktemp -d)"
trap 'rm -r "$sandbox"' EXIT

cat > "$sandbox/gh" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
jq_filter=""; event=""; branch=""; commit=""; args=("$@")
for i in "${!args[@]}"; do
  case "${args[$i]}" in
    --jq) jq_filter="${args[$((i + 1))]}" ;;
    --event) event="${args[$((i + 1))]}" ;;
    --branch) branch="${args[$((i + 1))]}" ;;
    --commit) commit="${args[$((i + 1))]}" ;;
  esac
done
case "$1" in
  run) jq --arg e "$event" --arg b "$branch" --arg c "$commit" \
         '[.[] | select(($e == "" or .event == $e) and ($b == "" or .headBranch == $b) and ($c == "" or .headSha == $c))]' \
         "$CATALOG/runs.json" | jq -r "$jq_filter" ;;
  api) id="$(echo "$2" | sed -E 's#.*/runs/([0-9]+)/artifacts#\1#')"
       file="$CATALOG/artifacts-$id.json"
       [ -f "$file" ] || file="$CATALOG/none.json"
       jq -r "$jq_filter" "$file" ;;
  *) echo "fake gh: unexpected $*" >&2; exit 2 ;;
esac
FAKE
chmod +x "$sandbox/gh"

catalog() { rm -rf "$sandbox/cat"; mkdir "$sandbox/cat"; echo '{"artifacts":[]}' > "$sandbox/cat/none.json"; echo "$1" > "$sandbox/cat/runs.json"; }
artifacts() { echo "$2" > "$sandbox/cat/artifacts-$1.json"; }
select_run() { CATALOG="$sandbox/cat" GH="$sandbox/gh" bash "$here/select-fixture-run.sh" owner/repo "$@" 2>"$sandbox/err"; }

full='{"artifacts":[{"name":"logs","id":1,"digest":"sha256:log","expired":false},{"name":"fixtures","id":77,"digest":"sha256:good","expired":false}]}'

# The newest green run is a quick one with no fixtures; the next is a full producer.
catalog '[{"databaseId":300,"event":"push","headBranch":"main","headSha":"c3"},{"databaseId":200,"event":"push","headBranch":"main","headSha":"c2"}]'
artifacts 300 '{"artifacts":[{"name":"logs","id":5,"digest":"sha256:q","expired":false}]}'
artifacts 200 "$full"
out="$(select_run)" && [ "$out" = "200 77 sha256:good" ] && ok "a quick newest run is passed over for the full producer" \
  || fail "quick-then-full chose '$out'"

# No candidate has fixtures at all.
catalog '[{"databaseId":300,"event":"push","headBranch":"main","headSha":"c3"},{"databaseId":299,"event":"push","headBranch":"main","headSha":"c2"}]'
if out="$(select_run)"; then fail "no producer still selected '$out'"; \
  elif grep -q 'none of the last 30' "$sandbox/err"; then ok "no producer in the window fails"; \
  else fail "no producer failed without naming the window"; fi

# The only fixtures artifact has expired.
catalog '[{"databaseId":200,"event":"push","headBranch":"main","headSha":"c2"}]'
artifacts 200 '{"artifacts":[{"name":"fixtures","id":77,"digest":"sha256:good","expired":true}]}'
if out="$(select_run)"; then fail "expired artifact still selected '$out'"; else ok "an expired fixtures artifact never qualifies"; fi

# Main merges through the queue: a push to main skips the fixtures job, and the
# merge_group run that tested the same commit built them. The newest queue run
# is for a commit main never took, so its fixtures are not main's.
catalog '[{"databaseId":600,"event":"merge_group","headBranch":"gh-readonly-queue/main/pr-9-c5","headSha":"c9"},
  {"databaseId":500,"event":"push","headBranch":"main","headSha":"c5"},
  {"databaseId":499,"event":"merge_group","headBranch":"gh-readonly-queue/main/pr-8-c4","headSha":"c5"},
  {"databaseId":400,"event":"push","headBranch":"main","headSha":"c4"},
  {"databaseId":399,"event":"merge_group","headBranch":"gh-readonly-queue/main/pr-7-c3","headSha":"c4"}]'
artifacts 600 '{"artifacts":[{"name":"fixtures","id":96,"digest":"sha256:off-main","expired":false}]}'
artifacts 500 '{"artifacts":[{"name":"logs","id":6,"digest":"sha256:q","expired":false}]}'
artifacts 499 '{"artifacts":[{"name":"fixtures","id":88,"digest":"sha256:queued","expired":false}]}'
artifacts 399 '{"artifacts":[{"name":"fixtures","id":77,"digest":"sha256:older","expired":false}]}'
out="$(select_run)" && [ "$out" = "499 88 sha256:queued" ] && ok "a main commit's fixtures come from the queue run that tested it" \
  || fail "queue-built fixtures chose '$out': $(cat "$sandbox/err")"

# An explicit run with the matching digest is taken as named.
catalog '[]'
artifacts 200 "$full"
out="$(select_run 200 sha256:good)" && [ "$out" = "200 77 sha256:good" ] && ok "an explicit run with its digest is accepted" \
  || fail "explicit matching digest gave '$out'"

# An explicit run whose digest differs is refused.
if out="$(select_run 200 sha256:other)"; then fail "wrong digest still selected '$out'"; \
  elif grep -q 'not the expected sha256:other' "$sandbox/err"; then ok "a wrong explicit digest fails"; \
  else fail "wrong digest failed for another reason: $(cat "$sandbox/err")"; fi

# An explicit run with no digest is refused.
if out="$(select_run 200)"; then fail "explicit run without digest selected '$out'"; else ok "an explicit run needs a digest"; fi

[ "$fails" -eq 0 ] && echo "select-fixture-run-test.sh: all checks passed"
exit $(( fails > 0 ))
