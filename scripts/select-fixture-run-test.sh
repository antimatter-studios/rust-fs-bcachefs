#!/usr/bin/env bash
# Controls for select-fixture-run.sh, over a fake gh that answers from a
# catalog directory: runs.json for `gh run list`, artifacts-<run>.json for
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
jq_filter=""; args=("$@")
for i in "${!args[@]}"; do [ "${args[$i]}" = --jq ] && jq_filter="${args[$((i + 1))]}"; done
case "$1" in
  run) jq -r "$jq_filter" "$CATALOG/runs.json" ;;
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
catalog '[{"databaseId":300},{"databaseId":200}]'
artifacts 300 '{"artifacts":[{"name":"logs","id":5,"digest":"sha256:q","expired":false}]}'
artifacts 200 "$full"
out="$(select_run)" && [ "$out" = "200 77 sha256:good" ] && ok "a quick newest run is passed over for the full producer" \
  || fail "quick-then-full chose '$out'"

# No candidate has fixtures at all.
catalog '[{"databaseId":300},{"databaseId":299}]'
if out="$(select_run)"; then fail "no producer still selected '$out'"; \
  elif grep -q 'none of the last 30' "$sandbox/err"; then ok "no producer in the window fails"; \
  else fail "no producer failed without naming the window"; fi

# The only fixtures artifact has expired.
catalog '[{"databaseId":200}]'
artifacts 200 '{"artifacts":[{"name":"fixtures","id":77,"digest":"sha256:good","expired":true}]}'
if out="$(select_run)"; then fail "expired artifact still selected '$out'"; else ok "an expired fixtures artifact never qualifies"; fi

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
