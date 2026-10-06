#!/usr/bin/env bash
# fs.bcachefs, as installed, reads every fixture the way the reference
# tools' guest recorded it: the root listing has every top-level name of the
# manifest, and every regular file's bytes hash to the manifest's SHA-256.
source "$(dirname "$0")/lib.sh"

for set in default lz4 zstd gzip nocsum xxhash crc64 block4k; do
    need_fixture "$set"
    img="$FIXTURES/$set.img"
    manifest="$FIXTURES/$set.json"

    if fs.bcachefs "$img" info >"$SANDBOX/info.json" 2>"$SANDBOX/err"; then
        check "$set: info reports fs bcachefs" jq -e '.fs == "bcachefs"' "$SANDBOX/info.json" >/dev/null
    else
        fail "$set: info failed: $(cat "$SANDBOX/err")"
    fi

    if fs.bcachefs "$img" ls / >"$SANDBOX/ls.json" 2>"$SANDBOX/err"; then
        want="$(jq -r '.entries[] | .path | select(test("^/[^/]+$")) | ltrimstr("/")' "$manifest" | sort)"
        got="$(jq -r '.[].name' "$SANDBOX/ls.json" | grep -vx 'lost+found' | sort)"
        check "$set: ls / lists the manifest's top level (want: $(echo $want), got: $(echo $got))" \
            test "$want" = "$got"
    else
        fail "$set: ls / failed: $(cat "$SANDBOX/err")"
    fi

    while IFS="$(printf '\t')" read -r path sha; do
        got="$(fs.bcachefs "$img" cat "$path" 2>"$SANDBOX/err" | shasum -a 256 | cut -d' ' -f1)"
        check "$set: cat $path hashes to $sha (got $got; $(cat "$SANDBOX/err"))" test "$got" = "$sha"
    done < <(jq -r '.entries[] | select(.type == "file") | [.path, .sha256] | @tsv' "$manifest")

    while IFS="$(printf '\t')" read -r path target; do
        got="$(fs.bcachefs "$img" cat "$path" 2>"$SANDBOX/err")"
        check "$set: cat $path is the link target $target (got $got)" test "$got" = "$target"
    done < <(jq -r '.entries[] | select(.type == "symlink") | [.path, .target] | @tsv' "$manifest")
done

# A path that does not exist is refused, not printed as empty.
if fs.bcachefs "$FIXTURES/default.img" cat /no-such-file >"$SANDBOX/out" 2>&1; then
    fail "cat of a missing path succeeded"
else
    ok
fi

finish
