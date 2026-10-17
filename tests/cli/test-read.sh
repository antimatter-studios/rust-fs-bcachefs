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
        if got="$(fs.bcachefs "$img" read "$path" 2>"$SANDBOX/err" | shasum -a 256 | cut -d' ' -f1)"; then
            check "$set: read $path hashes to $sha (got $got)" test "$got" = "$sha"
        else
            fail "$set: read $path failed: $(cat "$SANDBOX/err")"
        fi
    done < <(jq -r '.entries[] | select(.type == "file") | [.path, .sha256] | @tsv' "$manifest")

    # Both spellings extract empty, small and multi-chunk binary files.
    for verb in read cat; do
        for path in /empty /hello.txt /big/random.bin; do
            sha="$(jq -r --arg p "$path" '.entries[] | select(.path == $p) | .sha256' "$manifest")"
            if got="$(fs.bcachefs "$img" "$verb" "$path" 2>"$SANDBOX/err" | shasum -a 256 | cut -d' ' -f1)"; then
                check "$set: $verb $path is the guest's bytes" test "$got" = "$sha"
            else
                fail "$set: $verb $path failed: $(cat "$SANDBOX/err")"
            fi
            printf 'previous destination\n' >"$SANDBOX/extracted"
            if fs.bcachefs "$img" "$verb" "$path" -o "$SANDBOX/extracted" >"$SANDBOX/out" 2>"$SANDBOX/err"; then
                ok
                got="$(shasum -a 256 <"$SANDBOX/extracted" | cut -d' ' -f1)"
                check "$set: $verb -o $path is the guest's bytes" test "$got" = "$sha"
                check "$set: $verb -o has no stdout" test ! -s "$SANDBOX/out"
                check "$set: $verb -o has no stderr" test ! -s "$SANDBOX/err"
                check "$set: $verb -o leaves no partial" test ! -e "$SANDBOX/extracted.partial"
            else
                fail "$set: $verb -o $path failed: $(cat "$SANDBOX/err")"
            fi
        done
    done

    # A symlink is refused, naming the target the guest recorded. Neither
    # spelling may print target bytes or replace an output destination.
    while IFS="$(printf '\t')" read -r path target; do
        for verb in read cat; do
            printf 'keep this destination\n' >"$SANDBOX/saved"
            cp "$SANDBOX/saved" "$SANDBOX/dest"
            fs.bcachefs "$img" "$verb" "$path" -o "$SANDBOX/dest" >"$SANDBOX/out" 2>"$SANDBOX/err"
            check "$set: $verb refuses symlink $path" test $? -eq 1
            check "$set: $verb of a symlink has no stdout" test ! -s "$SANDBOX/out"
            check "$set: $verb names symlink target $target in its JSON error" \
                jq -e --arg t "$target" '.code == 1 and (.error | contains("symlink") and contains($t))' "$SANDBOX/err" >/dev/null
            check "$set: symlink refusal preserves destination" cmp -s "$SANDBOX/saved" "$SANDBOX/dest"
            check "$set: symlink refusal creates no partial" test ! -e "$SANDBOX/dest.partial"
        done
    done < <(jq -r '.entries[] | select(.type == "symlink") | [.path, .target] | @tsv' "$manifest")
done

# Refusals preserve existing destinations and keep errors off stdout.
img="$FIXTURES/default.img"
for verb in read cat; do
    for path in / /no-such-file; do
        cp "$SANDBOX/saved" "$SANDBOX/dest"
        fs.bcachefs "$img" "$verb" "$path" -o "$SANDBOX/dest" >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb refuses $path" test $? -eq 1
        check "$verb refusal has no stdout" test ! -s "$SANDBOX/out"
        check "$verb refusal is a shared error" jq -e '.code == 1 and (.error | type) == "string"' "$SANDBOX/err" >/dev/null
        check "$verb refusal preserves destination" cmp -s "$SANDBOX/saved" "$SANDBOX/dest"
        check "$verb refusal creates no partial" test ! -e "$SANDBOX/dest.partial"
    done

    fs.bcachefs "$img" "$verb" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb requires PATH" test $? -eq 2
    check "$verb usage error has no stdout" test ! -s "$SANDBOX/out"
    check "$verb usage error is JSON" jq -e '.code == 2 and (.error | type) == "string"' "$SANDBOX/err" >/dev/null

    fs.bcachefs "$img" "$verb" /link --text >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb --text refuses a symlink" test $? -eq 1
    check "$verb --text refusal has no stdout" test ! -s "$SANDBOX/out"
    check "$verb --text names the link target" grep -q 'symlink to hello.txt' "$SANDBOX/err"

    # An existing partial (including a host symlink) is never truncated.
    cp "$SANDBOX/saved" "$SANDBOX/dest.partial"
    fs.bcachefs "$img" "$verb" /hello.txt -o "$SANDBOX/dest" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb refuses an existing partial" test $? -eq 1
    check "$verb preserves the existing partial" cmp -s "$SANDBOX/saved" "$SANDBOX/dest.partial"
    check "$verb preserves destination when partial exists" cmp -s "$SANDBOX/saved" "$SANDBOX/dest"
    rm "$SANDBOX/dest.partial"
    ln -s "$SANDBOX/dest" "$SANDBOX/dest.partial"
    fs.bcachefs "$img" "$verb" /hello.txt -o "$SANDBOX/dest" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb refuses a symlink at the partial path" test $? -eq 1
    check "$verb preserves the host symlink" test -L "$SANDBOX/dest.partial"
    check "$verb preserves the host symlink's target" cmp -s "$SANDBOX/saved" "$SANDBOX/dest"
    rm "$SANDBOX/dest.partial"

    # A rename failure cleans up only the partial created by this command.
    mkdir "$SANDBOX/directory-$verb"
    cp "$SANDBOX/saved" "$SANDBOX/directory-$verb/saved"
    fs.bcachefs "$img" "$verb" /hello.txt -o "$SANDBOX/directory-$verb" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb refuses an output directory" test $? -eq 1
    check "$verb rename failure has no stdout" test ! -s "$SANDBOX/out"
    check "$verb rename failure is JSON" jq -e '.code == 1 and (.error | type) == "string"' "$SANDBOX/err" >/dev/null
    check "$verb rename failure preserves the directory" cmp -s "$SANDBOX/saved" "$SANDBOX/directory-$verb/saved"
    check "$verb rename failure removes its partial" test ! -e "$SANDBOX/directory-$verb.partial"
done

finish
