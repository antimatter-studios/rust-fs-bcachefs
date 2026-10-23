#!/usr/bin/env bash
# fs.bcachefs's write verbs, as installed, on a copy of the write-study base:
# each verb's effect read back with the read verbs, in place and through the
# journal. The reference checker's verdict on the same operations is
# tests/write_oracle.rs, in the guest.
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/offset-lib.sh"

# Run the same complete verb sequence on a raw device and an embedded one.
offset=0
fs.bcachefs() { command fs.bcachefs --offset "$offset" "$@"; }

if [ ! -f "$FIXTURES/write-study/base.img" ]; then
    echo "FAIL  $NAME: $FIXTURES/write-study/base.img is missing -- build the fixtures with \`chore fixtures\`" >&2
    echo "test result: FAILED. 0 passed; 1 failed"
    exit 1
fi

for mode in in-place journal in-place-offset journal-offset; do
    img="$SANDBOX/$mode.img"
    offset=0
    case "$mode" in *-offset) offset=1048576 ;; esac
    check "$mode: embed the reference-made write fixture" \
        offset_embed "$FIXTURES/write-study/base.img" "$img" "$offset"
    j=""
    case "$mode" in journal*) j="--journal" ;; esac

    printf 'hello from the cli\n' | fs.bcachefs "$img" put /d/hello $j >/dev/null 2>"$SANDBOX/err"
    check "$mode: put from stdin ($(cat "$SANDBOX/err"))" test $? -eq 0
    head -c 200000 /dev/zero | tr '\0' 'z' >"$SANDBOX/big"
    fs.bcachefs "$img" put /d/big --from "$SANDBOX/big" $j >/dev/null 2>"$SANDBOX/err"
    check "$mode: put --from a 200000-byte file ($(cat "$SANDBOX/err"))" test $? -eq 0
    fs.bcachefs "$img" mkdir /d/sub $j >/dev/null
    check "$mode: mkdir" test $? -eq 0
    fs.bcachefs "$img" mv /d/existing /d/sub/moved $j >/dev/null
    check "$mode: mv" test $? -eq 0
    fs.bcachefs "$img" ln -s sub/moved /d/link $j >/dev/null
    check "$mode: ln -s" test $? -eq 0
    fs.bcachefs "$img" ln /d/hello /d/hello2 $j >/dev/null
    check "$mode: ln" test $? -eq 0
    fs.bcachefs "$img" chmod 600 /d/hello $j >/dev/null
    check "$mode: chmod" test $? -eq 0
    fs.bcachefs "$img" chown 1000:1000 /d/hello $j >/dev/null
    check "$mode: chown" test $? -eq 0
    fs.bcachefs "$img" setfattr user.k v /d/hello $j >/dev/null
    check "$mode: setfattr" test $? -eq 0
    fs.bcachefs "$img" rmfattr user.k /d/hello $j >/dev/null
    check "$mode: rmfattr" test $? -eq 0
    fs.bcachefs "$img" mkdir /d/gone $j >/dev/null && fs.bcachefs "$img" rmdir /d/gone $j >/dev/null
    check "$mode: mkdir then rmdir" test $? -eq 0
    printf 'x' | fs.bcachefs "$img" put /d/doomed $j >/dev/null && fs.bcachefs "$img" rm /d/doomed $j >/dev/null
    check "$mode: put then rm" test $? -eq 0

    got="$(fs.bcachefs "$img" cat /d/hello)"
    check "$mode: cat /d/hello (got '$got')" test "$got" = "hello from the cli"
    check "$mode: cat /d/big is 200000 z's" test "$(fs.bcachefs "$img" cat /d/big | tr -d z | wc -c | tr -d ' ')" = 0
    check "$mode: cat /d/sub/moved" test "$(fs.bcachefs "$img" cat /d/sub/moved)" = "an existing file"
    fs.bcachefs "$img" cat /d/link >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$mode: cat refuses a symlink" test $? -eq 1
    check "$mode: symlink refusal has no stdout" test ! -s "$SANDBOX/out"
    check "$mode: symlink refusal names its target" \
        jq -e '.code == 1 and (.error | contains("symlink to sub/moved"))' "$SANDBOX/err" >/dev/null
    names="$(fs.bcachefs "$img" ls /d | jq -r '.[].name' | sort | tr '\n' ' ')"
    check "$mode: ls /d (got $names)" test "$names" = "big hello hello2 link sub "
    fs.bcachefs "$img" stat /d/hello >"$SANDBOX/stat.json"
    check "$mode: stat /d/hello: mode, owner, links" \
        jq -e '.mode == "0600" and .uid == 1000 and .gid == 1000 and .nlink == 2' "$SANDBOX/stat.json" >/dev/null
    size="$(wc -c <"$FIXTURES/write-study/base.img" | tr -d ' ')"
    check "$mode: writes preserve prefix and suffix sentinels" \
        offset_extract "$img" "$SANDBOX/partition.img" "$offset" "$size"
    fsck.bcachefs "$SANDBOX/partition.img" >/dev/null 2>&1
    check "$mode: fsck.bcachefs finds nothing" test $? -eq 0
done
offset=0

# Refusals: a name that exists, a directory that is not empty. What the
# volume's state refuses is exit 4, `refused`, in the CLI contract
# (EXIT_REFUSED), with the JSON error and the image byte-identical.
cp "$FIXTURES/write-study/base.img" "$SANDBOX/refuse.img"
printf 'x' | fs.bcachefs "$SANDBOX/refuse.img" put /d >/dev/null 2>&1
check "put over a directory is refused" test $? -ne 0
cp "$SANDBOX/refuse.img" "$SANDBOX/refuse-before.img"
state_refused() {
    local why="$1"
    shift
    fs.bcachefs "$SANDBOX/refuse.img" "$@" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$* exits 4" test $? -eq 4
    check "$* prints nothing on stdout" test ! -s "$SANDBOX/out"
    check "$* is refused: $why" \
        jq -e --arg why "$why" '.code == 4 and (.error | contains($why))' "$SANDBOX/err" >/dev/null
    check "$* leaves the image byte-identical" cmp -s "$SANDBOX/refuse.img" "$SANDBOX/refuse-before.img"
}
state_refused "not empty" rmdir /d
state_refused "already exists" mkdir /d

# The shared spelling and its alias have the same input, mode and journal
# options, and every predictable refusal preserves the complete image.
refusal() {
    local code="$1" description="$2"
    shift 2
    fs.bcachefs "$img" "$verb" "$@" < /dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb: $description status" test $? -eq "$code"
    check "$verb: $description has no stdout" test ! -s "$SANDBOX/out"
    # shellcheck disable=SC2016 # $c belongs to jq's --argjson.
    check "$verb: $description is a shared JSON error" \
        jq -e --argjson c "$code" '.code == $c and (.error | type) == "string"' "$SANDBOX/err" >/dev/null
    check "$verb: $description preserves the image" cmp -s "$img" "$SANDBOX/saved.img"
}
printf '\0\377binary\n' >"$SANDBOX/input"
for fixture in casefold.img kernel-rootsnap.img; do
    if [ ! -f "$FIXTURES/$fixture" ]; then
        fail "$FIXTURES/$fixture is missing -- build it with \`chore fixtures\`"
        finish
    fi
done
for verb in write put; do
    for mode in in-place journal; do
        img="$SANDBOX/shared-$verb-$mode.img"
        cp "$FIXTURES/write-study/base.img" "$img"
        args=()
        [ "$mode" = journal ] && args+=(--journal)
        fs.bcachefs "$img" "$verb" /d/shared --mode 640 ${args[@]+"${args[@]}"} <"$SANDBOX/input" >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb $mode: stdin succeeds" test $? -eq 0
        # shellcheck disable=SC2016 # $j belongs to jq's --argjson.
        check "$verb $mode: the report uses the canonical verb" \
            jq -e --argjson j "$([ "$mode" = journal ] && echo true || echo false)" \
            '.verb == "write" and .size == 9 and .ino > 0 and .journal == $j' "$SANDBOX/out" >/dev/null
        check "$verb $mode: no success diagnostic" test ! -s "$SANDBOX/err"
        fs.bcachefs "$img" read /d/shared >"$SANDBOX/bytes"
        check "$verb $mode: binary stdin is exact" cmp -s "$SANDBOX/input" "$SANDBOX/bytes"
        fs.bcachefs "$img" stat /d/shared >"$SANDBOX/stat"
        check "$verb $mode: mode is applied on creation" jq -e '.mode == "0640"' "$SANDBOX/stat" >/dev/null
        fs.bcachefs "$img" "$verb" /d/shared --from "$SANDBOX/big" --mode 600 ${args[@]+"${args[@]}"} >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb $mode: --from replaces the file" test $? -eq 0
        fs.bcachefs "$img" read /d/shared >"$SANDBOX/bytes"
        check "$verb $mode: replacement bytes are exact" cmp -s "$SANDBOX/big" "$SANDBOX/bytes"
        fs.bcachefs "$img" stat /d/shared >"$SANDBOX/stat"
        check "$verb $mode: replacement keeps the existing mode" jq -e '.mode == "0640"' "$SANDBOX/stat" >/dev/null
        fs.bcachefs "$img" "$verb" /d/shared ${args[@]+"${args[@]}"} </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb $mode: empty stdin truncates" test $? -eq 0
        fs.bcachefs "$img" read /d/shared >"$SANDBOX/bytes"
        check "$verb $mode: the file is empty" test ! -s "$SANDBOX/bytes"
        cp "$img" "$SANDBOX/saved.img"
        refusal 1 'directory destination' /d ${args[@]+"${args[@]}"}
        refusal 1 'missing parent' /missing/file ${args[@]+"${args[@]}"}
        refusal 1 'file parent' /d/shared/file ${args[@]+"${args[@]}"}
        refusal 1 'missing input' /d/new --from "$SANDBOX/missing" ${args[@]+"${args[@]}"}
        refusal 1 'directory input' /d/new --from "$SANDBOX" ${args[@]+"${args[@]}"}
        refusal 2 'invalid mode' /d/new --mode 888 ${args[@]+"${args[@]}"}
        refusal 2 'out-of-range mode' /d/new --mode 10000 ${args[@]+"${args[@]}"}
        refusal 2 'missing path' ${args[@]+"${args[@]}"}
        refusal 2 'root path' / ${args[@]+"${args[@]}"}
        refusal 2 'dot name' /d/. ${args[@]+"${args[@]}"}
        refusal 2 'dot-dot name' /d/../new ${args[@]+"${args[@]}"}
        refusal 2 'long name' "/d/$(printf '%0256d' 0)" ${args[@]+"${args[@]}"}
        if [ "$mode" = journal ]; then
            refusal 3 'unclean in-place target' /d/new
        fi
    done
    img="$SANDBOX/symlink-$verb.img"
    cp "$FIXTURES/write-study/base.img" "$img"
    fs.bcachefs "$img" ln -s existing /d/link >/dev/null
    cp "$img" "$SANDBOX/saved.img"
    refusal 1 'symlink destination' /d/link
    refusal 1 'symlink parent' /d/link/new
    for fixture in casefold kernel-rootsnap; do
        img="$SANDBOX/capability.img"
        cp "$FIXTURES/$fixture.img" "$img"
        cp "$img" "$SANDBOX/saved.img"
        refusal 3 "$fixture creation" /new
        if [ "$fixture" = kernel-rootsnap ]; then
            refusal 3 'snapshotted file replacement' /kept.txt
        fi
    done
done

finish
