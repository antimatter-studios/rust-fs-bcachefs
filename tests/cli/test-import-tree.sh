#!/usr/bin/env bash
# fs.bcachefs import --recursive (#440), as installed, on copies of the
# write-study base: a local tree through the shared tree import, directories
# made in the image, every file published through the single-file sink, each
# item reported, a partial tree exiting nonzero. What was imported is read
# back here with the read verbs, which only shows the verb's contract; the
# reference mount's and the reference checker's verdict on such a tree is
# tests/write_oracle.rs, in the guest.
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/offset-lib.sh"
need_image write-study/base

base="$FIXTURES/write-study/base.img"

sha() { shasum -a 256 <"$1" | cut -d' ' -f1; }
# run ARG...: an import on $img, its report in $SANDBOX/report and its status
# in $status.
run() {
    fs.bcachefs "$img" import "$@" >"$SANDBOX/report" 2>"$SANDBOX/err"
    status=$?
}
jreport() { jq -e "$1" "$SANDBOX/report" >/dev/null; }
# jitem DESTINATION FILTER: the report's item for DESTINATION satisfies FILTER.
jitem() { jq -c --arg d "$1" '.items[] | select(.destination == $d)' "$SANDBOX/report" | jq -e "$2" >/dev/null; }
# no_staged: the image lists, and no staged name is among what it lists.
no_staged() {
    fs.bcachefs "$img" ls --recursive / >"$SANDBOX/listing" 2>/dev/null || return 1
    grep -q existing "$SANDBOX/listing" && ! grep -qF '.fs-bcachefs-import-' "$SANDBOX/listing"
}
absent() { ! fs.bcachefs "$img" stat "$1" >/dev/null 2>&1; }
reads() { [ "$(fs.bcachefs "$img" read "$1" 2>/dev/null)" = "$2" ]; }

fs.bcachefs image.img import --help >"$SANDBOX/help" 2>"$SANDBOX/err"
check 'help has a recursive example' grep -q 'fs.bcachefs disk.img import --recursive ' "$SANDBOX/help"
check 'help documents the depth bound' grep -q -- '--max-depth' "$SANDBOX/help"

img="$SANDBOX/base.img"
cp "$base" "$img"

# A tree 32 directories deep, a file in each directory, and beside them an
# empty file, an empty directory, a file larger than one transfer chunk and
# a name that takes quoting.
src="$SANDBOX/src"
mkdir -p "$src/empty-dir"
: >"$src/empty-file"
printf 'spaced\n' >"$src/a name with spaces"
python3 -c 'import random, sys; r = random.Random(440); sys.stdout.buffer.write(bytes(r.getrandbits(8) for _ in range(3 * 1024 * 1024 + 1)))' >"$src/big.bin"
deep="$src"
for n in $(seq -w 1 32); do
    deep="$deep/level$n"
    mkdir "$deep"
    printf 'file at level %s\n' "$n" >"$deep/file$n"
done
want_files="$(find "$src" -type f | wc -l | tr -d ' ')"
want_dirs="$(find "$src" -type d | wc -l | tr -d ' ')"
check 'the tree holds 32 levels below its root' test "$(find "$src" -type d -name 'level*' | wc -l | tr -d ' ')" -eq 32

run --recursive "$src" /d/tree
check "a tree imports ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'every item is completed' jreport ".completed == $((want_files + want_dirs)) and .refused == 0 and .failed == 0 and .skipped == 0"
check 'no directory is merged into a new tree' jreport '.merged_directories == 0'

# Every file reads back with the source's SHA-256, by name in the image.
mismatched=0
reported=0
while IFS= read -r local; do
    rel="${local#"$src"/}"
    fs.bcachefs "$img" read "/d/tree/$rel" >"$SANDBOX/back" 2>/dev/null
    [ "$(sha "$SANDBOX/back")" = "$(sha "$local")" ] || mismatched=$((mismatched + 1))
    jq -e --arg d "/d/tree/$rel" --arg s "$(sha "$local")" \
        '.items[] | select(.destination == $d) | .status == "completed" and .copy.sha256 == $s and .copy.created == true' \
        "$SANDBOX/report" >/dev/null || reported=$((reported + 1))
done < <(find "$src" -type f)
check 'every file reads back with the source SHA-256' test "$mismatched" -eq 0
check 'every file is reported with the source SHA-256' test "$reported" -eq 0
check 'the deepest file is under its 32 directories' reads "/d/tree${deep#"$src"}/file32" 'file at level 32'
check 'an empty directory is made' test "$(fs.bcachefs "$img" stat /d/tree/empty-dir 2>/dev/null | jq -r .type)" = dir
check 'an empty file is made' test -z "$(fs.bcachefs "$img" read /d/tree/empty-file 2>/dev/null)"
check 'no staged name is left behind' no_staged

# The depth bound is the shared one, and reported per item.
run --recursive --max-depth 2 "$src" /d/shallow
check 'a tree over --max-depth is partial' test "$status" -ne 0
check 'the item over the bound is failed by name' jreport '[.items[] | select(.status == "failed" and (.reason | contains("maximum depth")))] | length > 0'
check 'what is inside the bound is imported' reads /d/shallow/level01/file01 'file at level 01'

# An existing file without --replace is reported, its old bytes are kept,
# its directory is merged and its sibling still goes in.
second="$SANDBOX/second"
mkdir -p "$second/sub"
printf 'changed\n' >"$second/sub/file"
printf 'sibling\n' >"$second/sub/zz-sibling"
mkdir -p "$SANDBOX/first/sub"
printf 'original\n' >"$SANDBOX/first/sub/file"
run --recursive "$SANDBOX/first" /d/merge
check "the first tree imports ($(cat "$SANDBOX/err"))" test "$status" -eq 0
run --recursive "$second" /d/merge
check 'an existing file makes the tree partial' test "$status" -ne 0
check 'the partial tree still has a report' jreport '.completed >= 1 and .failed == 1'
check 'both directories are merged' jreport '.merged_directories == 2'
check 'the existing file is reported' jitem /d/merge/sub/file '.status == "failed" and (.reason | contains("--replace"))'
check 'its old bytes are kept' reads /d/merge/sub/file 'original'
check 'its sibling is imported' reads /d/merge/sub/zz-sibling 'sibling'
check 'nothing is staged after the partial tree' no_staged

# With --replace it is replaced, and reported as replaced.
run --recursive --replace "$second" /d/merge
check "--replace replaces inside a tree ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'the replacement is reported' jitem /d/merge/sub/file '.status == "completed" and .copy.replaced == true and .copy.created == false'
check 'the replaced file reads the new bytes' reads /d/merge/sub/file 'changed'

# A name bcachefs or this tool cannot store is refused by name, and its
# sibling still goes in. A host's own rules decide which such name it can
# hold: Linux takes bytes that are not UTF-8, macOS takes more than 255 bytes
# of UTF-8. The other cases are the unit tests' in src/cli/import.rs.
named="$SANDBOX/named"
mkdir -p "$named"
printf 'kept\n' >"$named/zz-kept"
case "$(uname -s)" in
    Darwin) bad="$(python3 -c 'print("日" * 86, end="")')" ;;
    *) bad="$(printf 'bad\377name')" ;;
esac
printf 'refused\n' >"$named/$bad"
check 'the host holds the awkward name' test -e "$named/$bad"
run --recursive "$named" /d/named
check 'a refused name makes the tree partial' test "$status" -ne 0
check 'one item is refused and the rest complete' jreport '.refused == 1 and .failed == 0 and .completed == 2'
check 'the refusal is on the name' jreport '[.items[] | select(.status == "refused") | select(.reason | contains("name"))] | length == 1'
check 'the sibling of a refused name is imported' reads /d/named/zz-kept 'kept'

# A destination name that long is refused the same way, with no image change.
cp "$img" "$SANDBOX/before.img"
long="$(python3 -c 'print("n" * 256, end="")')"
run --recursive "$named" "/d/$long"
check 'a destination name over 255 bytes is refused' test "$status" -ne 0
check 'it is an item of the report' jreport '.refused == 1 and .completed == 0'
check 'it changes no byte of the image' cmp -s "$img" "$SANDBOX/before.img"

# A directory never replaces a file, and nothing lands under it.
mkdir -p "$SANDBOX/onto/existing"
printf 'under\n' >"$SANDBOX/onto/existing/under"
printf 'beside\n' >"$SANDBOX/onto/beside"
fs.bcachefs "$img" read /d/existing >"$SANDBOX/old-existing" 2>/dev/null
run --recursive --replace "$SANDBOX/onto" /d
check 'a directory over a file makes the tree partial' test "$status" -ne 0
check 'the directory is reported as the blocker' jitem /d/existing '.status != "completed" and (.reason | contains("not a directory"))'
check 'nothing is imported under it' jreport '[.items[] | select(.destination == "/d/existing/under")] | length == 0'
fs.bcachefs "$img" read /d/existing >"$SANDBOX/new-existing" 2>/dev/null
check 'the file it would have replaced is intact' cmp -s "$SANDBOX/old-existing" "$SANDBOX/new-existing"
check 'the sibling of that directory is imported' reads /d/beside 'beside'

# A file never replaces a directory, even with --replace.
mkdir -p "$SANDBOX/fileover"
printf 'file\n' >"$SANDBOX/fileover/tree"
run --recursive --replace "$SANDBOX/fileover" /d
check 'a file over a directory makes the tree partial' test "$status" -ne 0
check 'the directory is kept' reads /d/tree/level01/file01 'file at level 01'

# A tree merges into the root.
mkdir -p "$SANDBOX/rooted"
printf 'at the root\n' >"$SANDBOX/rooted/at-root"
run --recursive "$SANDBOX/rooted" /
check "a tree imports into the root ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'the root is merged' jreport '.merged_directories == 1'
check 'its file is at the root' reads /at-root 'at the root'

# A missing parent is not made: only the tree itself is.
cp "$img" "$SANDBOX/before.img"
run --recursive "$SANDBOX/rooted" /no-such-parent/tree
check 'a missing parent makes the tree fail' test "$status" -ne 0
check 'the missing parent is reported' jreport '.failed == 1 and .completed == 0'
check 'it changes no byte of the image' cmp -s "$img" "$SANDBOX/before.img"

# Links and times this sink does not carry yet (#284) are refused per item,
# never dropped; a symbolic link is never followed.
links="$SANDBOX/links"
mkdir -p "$links"
printf 'plain\n' >"$links/plain"
ln -s plain "$links/symlink"
run --recursive "$links" /d/nolinks
check 'a symbolic link makes the tree partial' test "$status" -ne 0
check 'the link is refused asking for --preserve=links' jitem /d/nolinks/symlink '.status == "refused" and (.reason | contains("--preserve=links"))'
check 'the link is not created' absent /d/nolinks/symlink
check 'the plain file beside it is imported' reads /d/nolinks/plain 'plain'
run --recursive --preserve=links "$links" /d/links
check '--preserve=links is partial here' test "$status" -ne 0
check 'the link is refused by format' jitem /d/links/symlink '.status == "refused" and (.reason | contains("bcachefs"))'
run --recursive --preserve=times "$links/plain" /d/times
check '--preserve=times is partial here' test "$status" -ne 0
check 'times are refused by format' jitem /d/times '.status == "refused" and (.reason | contains("bcachefs"))'
run --recursive --preserve=sparse "$links/plain" /d/sparse
check '--preserve=sparse is refused here' test "$status" -ne 0
check 'holes are refused by format' jitem /d/sparse '.status == "refused" and (.reason | contains("bcachefs"))'
check 'a file refused for holes is not published' absent /d/sparse

# What is not a tree import is a usage error, and changes nothing.
cp "$img" "$SANDBOX/before.img"
fs.bcachefs "$img" import "$src" /d/not-recursive >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'a directory needs --recursive' test $? -ne 0
fs.bcachefs "$img" import --recursive --verify "$src" /d/verified >"$SANDBOX/out" 2>"$SANDBOX/err"
check '--verify does not apply to a tree' test $? -eq 2
check 'the refusal names --verify' grep -q -- '--verify' "$SANDBOX/err"
printf 'x' | fs.bcachefs "$img" import --recursive - /d/stdin-tree >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'stdin is not a tree' test $? -eq 2
check 'the usage errors change no byte of the image' cmp -s "$img" "$SANDBOX/before.img"

# One file under --recursive is the one-item tree.
run --recursive "$src/big.bin" /d/big-copy
check "one file imports under --recursive ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'it is one completed item' jreport '.completed == 1'

# The device window applies to a tree as to every import.
offset_embed "$base" "$SANDBOX/embedded.img" 1048576
fs.bcachefs "$SANDBOX/embedded.img" import --recursive "$SANDBOX/first" /d/offset-tree --offset 1048576 >"$SANDBOX/report" 2>"$SANDBOX/err"
status=$?
check "a tree honours the device offset ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'the container around the device is untouched' offset_extract "$SANDBOX/embedded.img" "$SANDBOX/extracted.img" 1048576 "$(wc -c <"$base" | tr -d ' ')"
check 'the offset tree reads back' test "$(fs.bcachefs "$SANDBOX/extracted.img" read /d/offset-tree/sub/file 2>/dev/null)" = 'original'

finish
