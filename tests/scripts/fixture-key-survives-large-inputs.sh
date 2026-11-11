#!/usr/bin/env bash
#
# fixture-key-survives-large-inputs.sh -- scripts/fixture-key.sh reads only
# part of some files it is given (its own GENERATION line, the `fixtures`
# blocks of chores.yml and ci.yml), and runs under `set -o pipefail`. When
# the reader stops early while `git show` is still writing, `git show` dies of
# SIGPIPE and the script exits 141 with nothing on stderr (#626). Whether it
# does depends on the pipe's capacity and on scheduling, so a guest under load
# failed it often and a quiet Mac never did.
#
# This makes it certain: a scratch repository whose three files carry far
# more text after the part the key reads than any pipe holds. The manifest,
# the key and --changed-from must come out exactly as they do without it.
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fails=0
# Quiet on a pass: the scripts tier holds its log to a budget.
ok() { :; }
fail() { echo "FAIL  $*"; fails=$((fails + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/fixture-key-large.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
tree="$tmp/tree"
mkdir -p "$tree"

# Every file the key reads, at this checkout's contents.
inputs="$(bash "$repo/scripts/fixture-key.sh" --files)" || {
    echo "FAIL  scripts/fixture-key.sh --files"
    exit 1
}
while IFS= read -r name; do
    mkdir -p "$tree/$(dirname "$name")"
    cp "$repo/$name" "$tree/$name"
done <<<"$inputs
chores.yml
.github/workflows/ci.yml
scripts/fixture-key.sh"

g() { git -C "$tree" -c user.name=guard -c user.email=guard@example.invalid -c commit.gpgsign=false "$@"; }
commit() {
    g add -A && g commit -q --no-verify -m "$1" && g rev-parse HEAD
}
g init -q || { echo "FAIL  git init"; exit 1; }
base="$(commit base)" || { echo "FAIL  the scratch repository has no base commit"; exit 1; }

# The key, as the script prints it, and its exit status.
run_key() { # OUT ERR ARGS...
    local out="$1" err="$2"
    shift 2
    (cd "$tree" && bash scripts/fixture-key.sh "$@") >"$out" 2>"$err"
}

run_key "$tmp/manifest.small" "$tmp/err" --manifest
status=$?
[ "$status" = 0 ] && ok "the manifest of the small files is made" ||
    fail "the manifest of the small files exited $status: $(cat "$tmp/err")"
run_key "$tmp/key.small" "$tmp/err"
status=$?
[ "$status" = 0 ] && ok "the key of the small files is made" ||
    fail "the key of the small files exited $status: $(cat "$tmp/err")"

# Four MiB of comment lines after the end of each file: none of it is part of
# what the key reads, so the key must not change.
pad() {
    awk 'BEGIN { for (i = 0; i < 65536; i++) printf "# padding line %06d, past what the fixture key reads, xxxxxxxxxxxxxx\n", i }' >>"$1"
}
pad "$tree/chores.yml"
pad "$tree/.github/workflows/ci.yml"
pad "$tree/scripts/fixture-key.sh"
for name in chores.yml .github/workflows/ci.yml scripts/fixture-key.sh; do
    size="$(wc -c <"$tree/$name" | tr -d ' ')"
    [ "$size" -gt 4000000 ] && ok "$name is $size bytes, more than any pipe holds" ||
        fail "$name is only $size bytes after padding"
done
commit padded >/dev/null || fail "the padded files are not committed"

run_key "$tmp/manifest.large" "$tmp/err" --manifest
status=$?
[ "$status" = 0 ] && ok "the manifest of the padded files is made" ||
    fail "the manifest of the padded files exited $status: $(cat "$tmp/err")"
cmp -s "$tmp/manifest.small" "$tmp/manifest.large" &&
    ok "padding past the fixtures blocks leaves the manifest as it was" ||
    fail "the manifest changed with padding past what it reads"

run_key "$tmp/key.large" "$tmp/err"
status=$?
[ "$status" = 0 ] && ok "the key of the padded files is made" ||
    fail "the key of the padded files exited $status: $(cat "$tmp/err")"
[ -s "$tmp/key.large" ] && cmp -s "$tmp/key.small" "$tmp/key.large" &&
    ok "the key is the same: $(cat "$tmp/key.large")" ||
    fail "the key changed with padding: '$(cat "$tmp/key.small")' then '$(cat "$tmp/key.large")'"

run_key "$tmp/changed" "$tmp/err" --changed-from "$base"
status=$?
[ "$status" = 0 ] && [ "$(cat "$tmp/changed")" = "changed=false" ] &&
    ok "--changed-from the small base says changed=false" ||
    fail "--changed-from the small base exited $status and said '$(cat "$tmp/changed")'"

[ "$fails" -eq 0 ] || exit 1
echo "fixture-key-survives-large-inputs: all checks passed"
