#!/usr/bin/env bash
# fs.bcachefs stat and tree, as installed, against what the reference tools'
# guest recorded: tree lists every path of the manifest with its type, and
# stat reports each path in the shared shape -- its type, size and
# permissions; on the aged set, also the inode number, link count, owner,
# allocated bytes and the three times the reference mount reported -- with
# the creation time, generation, sectors and flags the reference lister
# printed for the same inode.
source "$(dirname "$0")/lib.sh"

# The shared keys, in order, and their types: one shape for every
# fs.<fs> stat. bcachefs stores all four times, so none of them is null.
shared='["path","inode","type","mode","uid","gid","nlink","size","allocated_bytes","atime","atime_nsec","mtime","mtime_nsec","ctime","ctime_nsec","btime","btime_nsec"]'
typed='(.path|type)=="string" and (.inode|type)=="number" and (.type|type)=="string"
    and (.mode|test("^[0-7]{4}$")) and (.uid|type)=="number" and (.gid|type)=="number"
    and (.nlink|type)=="number" and (.size|type)=="number" and (.allocated_bytes|type)=="number"
    and all(.atime, .mtime, .ctime, .btime; type=="number")
    and all(.atime_nsec, .mtime_nsec, .ctime_nsec, .btime_nsec; type=="number" and . >= 0 and . < 1000000000)
    and (.bcachefs|type)=="object" and (.bcachefs.sectors|type)=="number"
    and (.bcachefs.flags|type)=="array" and (.bcachefs.generation|type)=="number"
    and all(.bcachefs.data_checksum, .bcachefs.compression, .bcachefs.background_compression; type=="string" or type=="null")'
# The keys stat printed before it took the shared shape.
old='has("ino") or has("sectors") or has("atime_ns") or has("mtime_ns") or has("ctime_ns")'

# lister FILE: one line per inode the reference lister printed,
# "INODE OTIME MTIME GENERATION SECTORS FLAGS", as it printed them: the
# times in the units the inode stores, the flags word in hex.
lister() {
    awk '/^u64s / { split($5, p, ":"); ino = ($4 == "inode_v3") ? p[2] : ""; next }
        {
            sub(/^[ \t]+/, ""); n = index($0, "=")
            if (!n || ino == "") next
            k = substr($0, 1, n - 1); split(substr($0, n + 1), v, " ")
            if (k == "flags") { sub(/.*\(/, "", v[1]); sub(/\).*/, "", v[1]) }
            f[ino, k] = v[1]; seen[ino] = 1
        }
        END {
            for (i in seen)
                print i, f[i, "bi_otime"], f[i, "bi_mtime"], f[i, "bi_generation"], f[i, "bi_sectors"], f[i, "flags"]
        }' "$1"
}

# flag_json HEX: what stat must report for the lister's flags word: bi_flags
# is its low 20 bits; the one bit the lister has named, then every other
# set bit as one hex value.
flag_json() {
    local f=$((0x$1 & 0xfffff)) out=()
    if ((f & 1 << 10)); then out+=('"has_case_insensitive"'); fi
    if ((f & ~(1 << 10))); then out+=("\"$(printf '0x%08x' $((f & ~(1 << 10))))\""); fi
    local IFS=,
    echo "[${out[*]}]"
}

# ints VALUE...: every VALUE is a decimal integer. Arithmetic on anything
# else would end the file under set -u, before the checks after it ran.
ints() {
    local v
    for v; do [[ "$v" =~ ^-?[0-9]+$ ]] || return 1; done
}

for set in default zstd block4k aged; do
    need_fixture "$set"
    img="$FIXTURES/$set.img"
    manifest="$FIXTURES/$set.json"
    if [ -f "$FIXTURES/$set.inodes.txt" ]; then
        lister "$FIXTURES/$set.inodes.txt" >"$SANDBOX/lister.txt"
    else
        fail "$set: $FIXTURES/$set.inodes.txt is missing -- build the fixtures with \`chore fixtures\`"
        : >"$SANDBOX/lister.txt"
    fi
    # The superblock's "Time of last write" as the reference printed it, in
    # seconds since the epoch (UTC, the guest's zone being unknown here).
    last="$(sed -n 's/^Time of last write: *//p' "$FIXTURES/$set.super.txt" 2>/dev/null |
        jq -Rr 'gsub(" +"; " ") | strptime("%a %b %d %H:%M:%S %Y") | mktime' 2>/dev/null)"
    ints "${last:-}" || fail "$set: no time of last write in $FIXTURES/$set.super.txt -- build the fixtures with \`chore fixtures\`"

    for verb in 'ls --recursive' tree; do
    read -r -a words <<<"$verb"
    if fs.bcachefs "$img" "${words[@]}" / >"$SANDBOX/tree.json" 2>"$SANDBOX/err"; then
        ids="$(jq '.entries | any(has("ino"))' "$manifest")"
        want="$(jq -r '.entries[] | "\(.path) \(.type) \(.ino // "")"' "$manifest" | sort)"
        got="$(jq -r --argjson ids "$ids" '.[] | select(.path != "/lost+found") | "\(.path) \(if .type == "reg" then "file" elif .type == "lnk" then "symlink" else .type end) \(if $ids then .ino else "" end)"' "$SANDBOX/tree.json" | sort)"
        check "$set: $verb / lists the manifest's paths, types and recorded inodes" test "$want" = "$got"
    else
        fail "$set: $verb / failed: $(cat "$SANDBOX/err")"
    fi
    done

    # Every directory and symlink, and every 25th file: stat is per path.
    # The owner, blocks and times are in the manifest only where a mount
    # recorded it (the aged set); elsewhere they are "null".
    while IFS="$(printf '\t')" read -r path type size mode ino nlink target uid gid blocks atime atime_nsec mtime mtime_nsec ctime ctime_nsec; do
        j="$SANDBOX/stat.json"
        if ! fs.bcachefs "$img" stat "$path" >"$j" 2>"$SANDBOX/err"; then
            fail "$set: stat $path failed: $(cat "$SANDBOX/err")"
            continue
        fi
        check "$set: stat $path type is $type" jq -e --arg t "$type" '.type == $t' "$j" >/dev/null
        check "$set: stat $path mode is $mode" jq -e --arg m "$(printf '%04o' "$mode")" '.mode == $m' "$j" >/dev/null
        [ "$size" = "null" ] || check "$set: stat $path size is $size" jq -e --argjson s "$size" '.size == $s' "$j" >/dev/null
        [ "$ino" = "null" ] || check "$set: stat $path inode is $ino" jq -e --argjson i "$ino" '.inode == $i' "$j" >/dev/null
        [ "$nlink" = "null" ] || check "$set: stat $path nlink is $nlink" jq -e --argjson n "$nlink" '.nlink == $n' "$j" >/dev/null
        check "$set: stat $path starts with the shared keys, in order" jq -e "[keys_unsorted[]] | .[0:17] == $shared" "$j" >/dev/null
        check "$set: stat $path has every shared key typed" jq -e "$typed" "$j" >/dev/null
        check "$set: stat $path has none of the old keys" jq -e "($old) | not" "$j" >/dev/null
        # The tree was made, formatted and aged on the day the reference
        # last wrote the image: a time read in the wrong unit, from the
        # wrong base or with the wrong sign lands years away.
        check "$set: stat $path every time is within a day of the last write ($last)" \
            jq -e --argjson l "${last:-null}" '[.atime, .mtime, .ctime, .btime] | all(. >= $l - 86400 and . <= $l + 86400)' "$j" >/dev/null
        if [ "$type" = symlink ]; then
            check "$set: stat $path target is $target" jq -e --arg t "$target" '.target == $t' "$j" >/dev/null
        else
            check "$set: stat $path has no target" jq -e 'has("target") | not' "$j" >/dev/null
        fi

        # What the reference mount reported for the same path.
        if [ "$uid" != "null" ]; then
            check "$set: stat $path uid is $uid" jq -e --argjson u "$uid" '.uid == $u' "$j" >/dev/null
            check "$set: stat $path gid is $gid" jq -e --argjson g "$gid" '.gid == $g' "$j" >/dev/null
            check "$set: stat $path allocated_bytes is $blocks 512-byte blocks" \
                jq -e --argjson b "$blocks" '.allocated_bytes == $b * 512' "$j" >/dev/null
            for t in "atime $atime $atime_nsec" "mtime $mtime $mtime_nsec" "ctime $ctime $ctime_nsec"; do
                read -r key s ns <<<"$t"
                check "$set: stat $path $key is $s s + $ns ns" \
                    jq -e --arg k "$key" --argjson s "$s" --argjson n "$ns" \
                    '.[$k] == $s and .[$k + "_nsec"] == $n' "$j" >/dev/null
            done
        fi

        # What the reference lister printed for the same inode. Every
        # fixture's time precision is 1, so a stored time counts
        # nanoseconds: btime is mtime moved by the lister's otime - mtime.
        inode="$(jq -r '.inode' "$j")"
        read -r _ otime mtime_raw generation sectors flags < <(awk -v i="$inode" '$1 == i' "$SANDBOX/lister.txt")
        read -r bs bn ms mn < <(jq -r '"\(.btime) \(.btime_nsec) \(.mtime) \(.mtime_nsec)"' "$j")
        if ints "${otime:-}" "${mtime_raw:-}" "${generation:-}" "${sectors:-}" "${bs:-}" "${bn:-}" "${ms:-}" "${mn:-}" &&
            [[ "${flags:-}" =~ ^[0-9a-f]+$ ]]; then
            check "$set: stat $path btime is the lister's otime" \
                test $(((bs - ms) * 1000000000 + bn - mn)) -eq $((otime - mtime_raw))
            check "$set: stat $path generation is $generation" \
                jq -e --argjson g "$generation" '.bcachefs.generation == $g' "$j" >/dev/null
            check "$set: stat $path sectors is $sectors" \
                jq -e --argjson s "$sectors" '.bcachefs.sectors == $s' "$j" >/dev/null
            check "$set: stat $path flags are those of $flags" \
                jq -e --argjson f "$(flag_json "$flags")" '.bcachefs.flags == $f' "$j" >/dev/null
        else
            fail "$set: stat $path: no lister entry for inode $inode, or no times to compare"
        fi
    done < <(jq -r '.entries | to_entries[] | select(.value.type != "file" or (.key % 25 == 0)) | .value | [.path, .type, (.size // "null"), .mode, (.ino // "null"), (.nlink // "null"), (.target // "null"), (.uid // "null"), (.gid // "null"), (.blocks // "null"), (.atime // "null"), (.atime_nsec // "null"), (.mtime // "null"), (.mtime_nsec // "null"), (.ctime // "null"), (.ctime_nsec // "null")] | @tsv' "$manifest")
done

if fs.bcachefs "$FIXTURES/default.img" stat /no-such-path >"$SANDBOX/out" 2>&1; then
    fail "stat of a missing path succeeded"
else
    ok
fi

# The refusals: the shared error on stderr, nothing on stdout.
fs.bcachefs "$FIXTURES/default.img" stat /no-such-path >"$SANDBOX/m.out" 2>"$SANDBOX/m.err"
check "stat of a missing path exits 1" test $? -eq 1
check "stat of a missing path prints nothing on stdout" test ! -s "$SANDBOX/m.out"
check "stat of a missing path is the shared error, code 1" jq -e '.code == 1' "$SANDBOX/m.err" >/dev/null
# A non-UTF-8 PATH is resolved as bytes, as ls resolves it: one that names
# nothing is not found (1), not a malformed command line (2).
fs.bcachefs "$FIXTURES/default.img" stat "$(printf '/\377')" >/dev/null 2>&1
check "stat of a non-UTF-8 path that names nothing exits 1" test $? -eq 1
fs.bcachefs "$FIXTURES/default.img" stat >/dev/null 2>&1
check "stat with no PATH is a usage error" test $? -eq 2

fs.bcachefs "$FIXTURES/default.img" stat --text /hello.txt >"$SANDBOX/text" 2>/dev/null
check "stat --text names the type" grep -qx 'type: file' "$SANDBOX/text"
check "stat --text names the size" grep -qx 'size: 12' "$SANDBOX/text"
check "stat --text names the inode" grep -Eqx 'inode: [0-9]+' "$SANDBOX/text"

finish
