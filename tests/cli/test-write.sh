#!/usr/bin/env bash
# fs.bcachefs's write verbs, as installed, on a copy of the write-study base:
# each verb's effect read back with the read verbs, in place and through the
# journal. The reference checker's verdict on the same operations is
# tests/write_oracle.rs, in the guest.
source "$(dirname "$0")/lib.sh"

if [ ! -f "$FIXTURES/write-study/base.img" ]; then
    echo "FAIL  $NAME: $FIXTURES/write-study/base.img is missing -- build the fixtures with \`chore fixtures\`" >&2
    echo "test result: FAILED. 0 passed; 1 failed"
    exit 1
fi

for mode in in-place journal; do
    img="$SANDBOX/$mode.img"
    cp "$FIXTURES/write-study/base.img" "$img"
    j=""
    [ "$mode" = journal ] && j="--journal"

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
    fs.bcachefs "$img" mkdir /d/gone $j >/dev/null && fs.bcachefs "$img" rmdir /d/gone $j >/dev/null
    check "$mode: mkdir then rmdir" test $? -eq 0
    printf 'x' | fs.bcachefs "$img" put /d/doomed $j >/dev/null && fs.bcachefs "$img" rm /d/doomed $j >/dev/null
    check "$mode: put then rm" test $? -eq 0

    got="$(fs.bcachefs "$img" cat /d/hello)"
    check "$mode: cat /d/hello (got '$got')" test "$got" = "hello from the cli"
    check "$mode: cat /d/big is 200000 z's" test "$(fs.bcachefs "$img" cat /d/big | tr -d z | wc -c | tr -d ' ')" = 0
    check "$mode: cat /d/sub/moved" test "$(fs.bcachefs "$img" cat /d/sub/moved)" = "an existing file"
    check "$mode: cat /d/link is its target" test "$(fs.bcachefs "$img" cat /d/link)" = "sub/moved"
    names="$(fs.bcachefs "$img" ls /d | jq -r '.[].name' | sort | tr '\n' ' ')"
    check "$mode: ls /d (got $names)" test "$names" = "big hello hello2 link sub "
    fs.bcachefs "$img" stat /d/hello >"$SANDBOX/stat.json"
    check "$mode: stat /d/hello: mode, owner, links" \
        jq -e '.mode == "0600" and .uid == 1000 and .gid == 1000 and .nlink == 2' "$SANDBOX/stat.json" >/dev/null
    if command -v fsck.bcachefs >/dev/null 2>&1; then
        fsck.bcachefs "$img" >/dev/null 2>&1
        check "$mode: fsck.bcachefs finds nothing" test $? -eq 0
    fi
done

# Refusals: a name that exists, a directory that is not empty.
cp "$FIXTURES/write-study/base.img" "$SANDBOX/refuse.img"
printf 'x' | fs.bcachefs "$SANDBOX/refuse.img" put /d >/dev/null 2>&1
check "put over a directory is refused" test $? -ne 0
fs.bcachefs "$SANDBOX/refuse.img" rmdir /d >/dev/null 2>&1
check "rmdir of a directory with entries is refused" test $? -ne 0

finish
