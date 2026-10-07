#!/usr/bin/env bash
# fsck.bcachefs, as installed: clean fixtures exit 0 and report clean, a
# damaged copy exits 4 naming the damage, and repair is refused with 16.
source "$(dirname "$0")/lib.sh"

for set in default zstd block4k aged aged-unclean; do
    need_fixture "$set"
    fsck.bcachefs "$FIXTURES/$set.img" >"$SANDBOX/out.json" 2>"$SANDBOX/err"
    status=$?
    check "$set: fsck exits 0 (got $status: $(cat "$SANDBOX/err"))" test "$status" -eq 0
    check "$set: the report says clean" jq -e '.clean == true and (.problems | length) == 0' "$SANDBOX/out.json" >/dev/null
done

# The superblock's label byte flipped: its checksum no longer holds.
cp "$FIXTURES/default.img" "$SANDBOX/damaged.img"
printf '\377' | dd of="$SANDBOX/damaged.img" bs=1 seek=$((4096 + 72)) conv=notrunc 2>/dev/null
fsck.bcachefs "$SANDBOX/damaged.img" >"$SANDBOX/out.json" 2>"$SANDBOX/err"
status=$?
check "a damaged superblock: fsck exits 4 (got $status)" test "$status" -eq 4
# The copy at 2056 is read instead, and the dead primary is what is reported.
check "a damaged superblock: the report names the dead copy" jq -e '[.problems[].kind] == ["superblock_copy"]' "$SANDBOX/out.json" >/dev/null

fsck.bcachefs -y "$FIXTURES/default.img" >/dev/null 2>&1
status=$?
check "repair is refused with 16 (got $status)" test "$status" -eq 16

finish
