#!/usr/bin/env bash
# fs.bcachefs stat and tree, as installed, against what the reference tools'
# guest recorded: tree lists every path of the manifest with its type, and
# stat reports each path's type, size and permissions -- and, on the aged
# set, the inode number and link count the mount reported.
source "$(dirname "$0")/lib.sh"

for set in default zstd block4k aged; do
    need_fixture "$set"
    img="$FIXTURES/$set.img"
    manifest="$FIXTURES/$set.json"

    if fs.bcachefs "$img" tree / >"$SANDBOX/tree.json" 2>"$SANDBOX/err"; then
        want="$(jq -r '.entries[] | "\(.path) \(.type)"' "$manifest" | sort)"
        got="$(jq -r '.[] | select(.path != "/lost+found") | "\(.path) \(.type)"' "$SANDBOX/tree.json" | sort)"
        check "$set: tree / lists the manifest's paths and types" test "$want" = "$got"
    else
        fail "$set: tree / failed: $(cat "$SANDBOX/err")"
    fi

    # Every directory and symlink, and every 25th file: stat is per path.
    while IFS="$(printf '\t')" read -r path type size mode ino nlink; do
        if ! fs.bcachefs "$img" stat "$path" >"$SANDBOX/stat.json" 2>"$SANDBOX/err"; then
            fail "$set: stat $path failed: $(cat "$SANDBOX/err")"
            continue
        fi
        check "$set: stat $path type is $type" jq -e --arg t "$type" '.type == $t' "$SANDBOX/stat.json" >/dev/null
        check "$set: stat $path mode is $mode" jq -e --arg m "$(printf '%04o' "$mode")" '.mode == $m' "$SANDBOX/stat.json" >/dev/null
        [ "$size" = "null" ] || check "$set: stat $path size is $size" jq -e --argjson s "$size" '.size == $s' "$SANDBOX/stat.json" >/dev/null
        [ "$ino" = "null" ] || check "$set: stat $path ino is $ino" jq -e --argjson i "$ino" '.ino == $i' "$SANDBOX/stat.json" >/dev/null
        [ "$nlink" = "null" ] || check "$set: stat $path nlink is $nlink" jq -e --argjson n "$nlink" '.nlink == $n' "$SANDBOX/stat.json" >/dev/null
    done < <(jq -r '.entries | to_entries[] | select(.value.type != "file" or (.key % 25 == 0)) | .value | [.path, .type, (.size // "null"), .mode, (.ino // "null"), (.nlink // "null")] | @tsv' "$manifest")
done

if fs.bcachefs "$FIXTURES/default.img" stat /no-such-path >"$SANDBOX/out" 2>&1; then
    fail "stat of a missing path succeeded"
else
    ok
fi

finish
