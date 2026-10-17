#!/usr/bin/env bash
# The bounded walk contract, over a reference-made image. Runtime checks run in CI.
source "$(dirname "$0")/lib.sh"
need_fixture default
img="$SANDBOX/tree.img"
cp "$FIXTURES/default.img" "$img"
for name in dirlink ancestor dangling; do
    target=dir
    [ "$name" != ancestor ] || target=/
    [ "$name" != dangling ] || target=absent
    fs.bcachefs "$img" ln -s "$target" "/$name" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "create $name" test $? -eq 0
done
cp "$img" "$SANDBOX/before.img"
jcheck() { local what="$1"; shift; check "$what" jq -e "$@" >/dev/null; }

fs.bcachefs "$img" ls -R / >"$SANDBOX/tree.json" 2>"$SANDBOX/err"
check 'recursive ls succeeds' test $? -eq 0
check 'a complete walk has no error' test ! -s "$SANDBOX/err"
jcheck 'byte-sorted depth-first order and relative depths' \
    '. == sort_by(.path | split("/")) and all(.[]; .depth == (.path | split("/") | length) - 1)' "$SANDBOX/tree.json"
jcheck 'ls fields plus path and depth; symlinks are never entered' \
    'all(.[]; has("name") and has("type") and has("ino") and has("size") and has("path") and has("depth") and (has("revisited")|not)) and any(.[]; .path == "/dirlink" and .type == "lnk") and all(.[]; (.path | test("^/(dirlink|ancestor|dangling)/"))|not)' "$SANDBOX/tree.json"
fs.bcachefs "$img" ls / >"$SANDBOX/root.json" 2>/dev/null
jcheck 'plain ls has no recursive fields' 'all(.[]; (has("path") or has("depth") or has("revisited"))|not)' "$SANDBOX/root.json"
check 'depth 1 is plain ls' test "$(jq -c '[.[]|select(.depth == 1)|del(.path,.depth)]|sort_by(.name)' "$SANDBOX/tree.json")" = "$(jq -c 'sort_by(.name)' "$SANDBOX/root.json")"
count="$(jq length "$SANDBOX/tree.json")"
depth="$(jq 'map(.depth)|max' "$SANDBOX/tree.json")"
for args in 'ls --recursive' tree "ls -R --max-depth $depth --limit $count"; do
    read -r -a words <<<"$args"
    fs.bcachefs "$img" "${words[@]}" / >"$SANDBOX/again" 2>"$SANDBOX/err"
    check "$args succeeds, including an exact fit" test $? -eq 0
    check "$args prints identical bytes" cmp -s "$SANDBOX/tree.json" "$SANDBOX/again"
done
for spec in 'max-depth 2' 'max-depth 0' 'limit 2' 'limit 0'; do
    read -r bound value <<<"$spec"
    fs.bcachefs "$img" ls -R "--$bound" "$value" / >"$SANDBOX/partial" 2>"$SANDBOX/err"
    check "$spec exits 1" test $? -eq 1
    if [ "$bound" = limit ]; then n="$value"; else n="$(jq --argjson d "$value" '[.[].depth > $d]|index(true)' "$SANDBOX/tree.json")"; fi
    stopped="$(jq -r --argjson n "$n" '.[$n].path' "$SANDBOX/tree.json")"
    check "$spec keeps exactly the prefix" test "$(jq -c . "$SANDBOX/partial")" = "$(jq -c --argjson n "$n" '.[:$n]' "$SANDBOX/tree.json")"
    jcheck "$spec names its bound and stopping path" --arg b "--$bound $value" --arg p "$stopped" '.code == 1 and (.error|contains($b)) and (.error|contains($p))' "$SANDBOX/err"
done
for args in 'ls -R --limit 0' 'tree --max-depth 0 --limit 0'; do
    read -r -a words <<<"$args"
    fs.bcachefs "$img" "${words[@]}" /lost+found >"$SANDBOX/empty" 2>"$SANDBOX/err"
    check 'zero bounds allow an empty directory' test $? -eq 0
    jcheck 'empty directory is an empty report' '. == []' "$SANDBOX/empty"
done
fs.bcachefs "$img" ls -R dir//sub/ >"$SANDBOX/sub" 2>/dev/null
check 'relative subtree succeeds' test $? -eq 0
jcheck 'subtree paths are volume absolute, depths relative' '[.[]|[.path,.depth]] == [["/dir/sub/b.txt",1]]' "$SANDBOX/sub"
for start in /hello.txt /dirlink; do
    fs.bcachefs "$img" ls -R --max-depth 0 "$start" >"$SANDBOX/file" 2>/dev/null
    check "$start alone succeeds at depth 0" test $? -eq 0
    jcheck "$start is one entry at depth 0" --arg p "$start" 'length == 1 and .[0].path == $p and .[0].depth == 0' "$SANDBOX/file"
    fs.bcachefs "$img" ls -R --limit 0 "$start" >"$SANDBOX/file" 2>"$SANDBOX/err"
    check 'limit 0 refuses even a starting file' test $? -eq 1
    jcheck 'limit 0 retains an empty array' '. == []' "$SANDBOX/file"
done
fs.bcachefs --text "$img" ls -R --json --limit 2 / >"$SANDBOX/partial" 2>"$SANDBOX/err"
check 'partial JSON uses the last format flag' test $? -eq 1
jcheck 'partial JSON has two entries' 'length == 2' "$SANDBOX/partial"
jcheck 'partial error uses JSON too' '.code == 1' "$SANDBOX/err"
fs.bcachefs "$img" tree --text --limit 2 / >"$SANDBOX/text" 2>"$SANDBOX/err"
check 'partial text exits 1' test $? -eq 1
check 'partial text retains two lines' test "$(wc -l <"$SANDBOX/text")" -eq 2
check 'text contains full paths' grep -q ' /ancestor$' "$SANDBOX/text"
for args in '--max-depth 2' '--limit 2' '-R --limit nope' '-R --max-depth -1' '-R --limit 18446744073709551616'; do
    read -r -a words <<<"$args"
    fs.bcachefs "$img" ls "${words[@]}" / >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$args is a usage error" test $? -eq 2
    check 'usage errors have no report' test ! -s "$SANDBOX/out"
    jcheck 'usage error is structured' '.code == 2' "$SANDBOX/err"
done
fs.bcachefs "$img" ls -R /missing >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'missing starting path exits 1' test $? -eq 1
check 'missing starting path has no report' test ! -s "$SANDBOX/out"
jcheck 'missing starting path is named' '.code == 1 and (.error|contains("/missing"))' "$SANDBOX/err"
{ head -c 1048576 /dev/zero; cat "$img"; } >"$SANDBOX/whole.img"
fs.bcachefs "$SANDBOX/whole.img" ls -R / --offset 1048576 >"$SANDBOX/offset" 2>/dev/null
check 'recursive walk honors the device offset' test $? -eq 0
check 'offset walk is identical' cmp -s "$SANDBOX/tree.json" "$SANDBOX/offset"
check 'all listings and refusals leave the image byte-identical' cmp -s "$img" "$SANDBOX/before.img"
for verb in ls tree; do
    fs.bcachefs missing.img "$verb" --help >"$SANDBOX/help"
    check "$verb help documents defaults and an example" grep -q '100000' "$SANDBOX/help"
    check "$verb help documents depth 64" grep -q '64' "$SANDBOX/help"
done
rust-fs-bcachefs generate man "$SANDBOX/share" >/dev/null
check 'man generation succeeds' test $? -eq 0
rust-fs-bcachefs generate completions "$SANDBOX/share" >/dev/null
check 'completion generation succeeds' test $? -eq 0
documents_option() {
    # Generated roff escapes hyphens, including the one inside max-depth.
    # Consume the whole input so pipefail cannot mistake SIGPIPE for failure.
    sed 's/\\-/-/g' "$SANDBOX/share/$2" | grep -F -- "$1" >/dev/null
}
for option in recursive max-depth limit; do
    for doc in man/man1/fs.bcachefs-ls.1 zsh/site-functions/_fs.bcachefs bash-completion/completions/fs.bcachefs fish/vendor_completions.d/fs.bcachefs.fish; do
        check "$doc documents $option" documents_option "$option" "$doc"
    done
done
finish
