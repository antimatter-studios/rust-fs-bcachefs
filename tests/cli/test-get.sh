#!/usr/bin/env bash
# Shared properties are compared with independent VM printer/lister reports.
# shellcheck disable=SC2016 # Dollar names in single quotes are jq variables.
source "$(dirname "$0")/lib.sh"

for set in default lz4 zstd gzip nocsum xxhash crc64 block4k labeled aged write-study/base; do
    need_fixture "$set"
    img="$FIXTURES/$set.img"
    check "$set: VM property oracle exists" python3 "$REPO/tests/cli/get-oracle.py" \
        "$FIXTURES/$set" >"$SANDBOX/want.json"
    if [ "$set" = labeled ]; then
        check "the labeled fixture has a filesystem label" jq -e '.label == "PROPERTIES"' "$SANDBOX/want.json" >/dev/null
    fi
    for verb in get info; do
        fs.bcachefs "$img" "$verb" >"$SANDBOX/got.json" 2>"$SANDBOX/err"
        check "$set: $verb succeeds" test $? -eq 0
        check "$set: $verb has no stderr" test ! -s "$SANDBOX/err"
        check "$set: $verb has exactly the shared keys in order" jq -e \
            'keys_unsorted == ["fs","label","total_bytes","free_bytes","block_size","dirty","bcachefs"]' "$SANDBOX/got.json" >/dev/null
        check "$set: $verb shared types" jq -e '
            (.fs | type) == "string" and (.label == null or (.label | type) == "string") and
            ([.total_bytes,.free_bytes,.block_size] | all(type == "number" and . >= 0 and floor == .)) and
            (.dirty | type) == "boolean" and (.bcachefs | type) == "object"' "$SANDBOX/got.json" >/dev/null
        check "$set: every $verb property matches the VM reports" jq -e --slurpfile want "$SANDBOX/want.json" \
            '. == $want[0]' "$SANDBOX/got.json" >/dev/null

        # JSON selection keeps the literal dotted key; text is the scalar.
        for key in fs label total_bytes free_bytes block_size dirty bcachefs bcachefs.uuid bcachefs.version bcachefs.btree_node_size bcachefs.devices bcachefs.encrypted bcachefs.clean; do
            fs.bcachefs "$img" "$verb" "$key" >"$SANDBOX/one.json" 2>"$SANDBOX/err"
            check "$set: $verb $key succeeds" test $? -eq 0
            check "$set: $verb $key selects the same value" jq -e --arg k "$key" --slurpfile all "$SANDBOX/got.json" \
                '. == {($k): ($all[0] | getpath($k | split(".")))}' "$SANDBOX/one.json" >/dev/null
        done
    done
    fs.bcachefs "$img" get >"$SANDBOX/get.json" 2>"$SANDBOX/err"
    fs.bcachefs "$img" info >"$SANDBOX/info.json" 2>"$SANDBOX/err"
    check "$set: get/info are byte-identical" cmp -s "$SANDBOX/get.json" "$SANDBOX/info.json"
    fs.bcachefs "$img" get --text >"$SANDBOX/get.text" 2>"$SANDBOX/err"
    fs.bcachefs "$img" info --text >"$SANDBOX/info.text" 2>"$SANDBOX/err"
    check "$set: get/info text is byte-identical" cmp -s "$SANDBOX/get.text" "$SANDBOX/info.text"
    for verb in get info; do
        fs.bcachefs "$img" "$verb" --text >"$SANDBOX/text" 2>"$SANDBOX/err"
        check "$set: $verb --text succeeds" test $? -eq 0
        check "$set: $verb --text includes dotted fields" grep -q '^bcachefs.uuid: ' "$SANDBOX/text"
        fs.bcachefs "$img" "$verb" bcachefs.uuid --text >"$SANDBOX/uuid" 2>"$SANDBOX/err"
        check "$set: $verb scalar text succeeds" test $? -eq 0
        check "$set: $verb scalar text has no key or quotes" test "$(cat "$SANDBOX/uuid")" = "$(jq -r '.bcachefs.uuid' "$SANDBOX/want.json")"
        fs.bcachefs "$img" "$verb" label --text >"$SANDBOX/label" 2>"$SANDBOX/err"
        check "$set: $verb label text succeeds" test $? -eq 0
        check "$set: $verb null label text is empty" test "$(cat "$SANDBOX/label")" = "$(jq -r '.label // ""' "$SANDBOX/want.json")"
    done
done

# The superblock printer recorded both clean states. Replay must not hide dirty.
need_fixture aged-unclean
for verb in get info; do
    fs.bcachefs "$FIXTURES/aged-unclean.img" "$verb" dirty >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb reports an unclean image" test $? -eq 0
    check "$verb dirty agrees with the reference printer" grep -q '^Clean: *0' "$FIXTURES/aged-unclean.super.txt"
    check "$verb dirty is true" jq -e '. == {dirty: true}' "$SANDBOX/out" >/dev/null
    for key in missing uuid bcachefs.missing bcachefs.uuid.extra 'bcachefs.' ''; do
        fs.bcachefs "$FIXTURES/default.img" "$verb" "$key" >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb refuses unknown key $key" test $? -eq 2
        check "$verb unknown key has no stdout" test ! -s "$SANDBOX/out"
        check "$verb unknown key gives shared usage error" jq -e '.code == 2 and (.error | contains("no key"))' "$SANDBOX/err" >/dev/null
    done
done

# Capability refusals happen before any target open, even for missing targets.
cp "$FIXTURES/default.img" "$SANDBOX/image"
cp "$SANDBOX/image" "$SANDBOX/saved"
for target in "$SANDBOX/image" "$SANDBOX/missing"; do
    for format in --json --text; do
        for verb in set resize; do
            args=(label BACKUP)
            [ "$verb" != resize ] || args=(-512M --force)
            fs.bcachefs "$target" "$verb" "${args[@]}" "$format" >"$SANDBOX/out" 2>"$SANDBOX/err"
            check "$verb $format refuses before opening $target" test $? -eq 3
            check "$verb $format has no stdout" test ! -s "$SANDBOX/out"
            if [ "$format" = --json ]; then
                check "$verb has a shared unsupported error" jq -e '.code == 3 and (.error | type) == "string"' "$SANDBOX/err" >/dev/null
            else
                check "$verb has a text error" grep -q 'not implemented' "$SANDBOX/err"
            fi
            check "$verb refusal leaves the image byte-identical" cmp -s "$SANDBOX/image" "$SANDBOX/saved"
        done
    done
done
for verb in set resize; do
    fs.bcachefs "$SANDBOX/missing" "$verb" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb requires arguments" test $? -eq 2
    check "$verb usage error has no stdout" test ! -s "$SANDBOX/out"
    check "$verb usage error is shared JSON" jq -e '.code == 2' "$SANDBOX/err" >/dev/null
done
fs.bcachefs "$SANDBOX/missing" set label >"$SANDBOX/out" 2>"$SANDBOX/err"
check "set requires VALUE" test $? -eq 2

for verb in get info set resize; do
    fs.bcachefs missing.img "$verb" --help >"$SANDBOX/help" 2>"$SANDBOX/err"
    check "$verb help succeeds without an image" test $? -eq 0
    check "$verb help includes an example" grep -q "fs.bcachefs disk.img $verb" "$SANDBOX/help"
done
rust-fs-bcachefs generate man "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check "man generation succeeds" test $? -eq 0
rust-fs-bcachefs generate completions "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check "completion generation succeeds" test $? -eq 0
for verb in get info set resize; do
    check "$verb man page carries its example" grep -q "disk.img $verb" "$SANDBOX/share/man/man1/fs.bcachefs-$verb.1"
    for completion in zsh/site-functions/_fs.bcachefs bash-completion/completions/fs.bcachefs fish/vendor_completions.d/fs.bcachefs.fish; do
        check "$completion offers $verb" grep -qw "$verb" "$SANDBOX/share/$completion"
    done
done
finish
