#!/usr/bin/env bash
# The device offset is global, unsigned, and applied before every verb.
# shellcheck disable=SC2016 # Dollar names in single quotes are jq variables.
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/offset-lib.sh"
need_fixture default
img="$SANDBOX/whole.img"
check 'embed the reference-made filesystem at byte 1048576' \
    offset_embed "$FIXTURES/default.img" "$img" 1048576
cp "$img" "$SANDBOX/saved.img"
cp "$FIXTURES/default.img" "$SANDBOX/raw.img"
size="$(wc -c <"$FIXTURES/default.img" | tr -d ' ')"

# Discover the actual parser surface, including verbs added by parallel
# slices. New verbs must get a row, and must reject a past-end offset on
# an otherwise valid offset-zero filesystem before doing anything else.
fs.bcachefs --help >"$SANDBOX/help"
check 'the command help is available' test $? -eq 0
while read -r verb; do
    expected=1
    case "$verb" in
        help) continue ;;
        info|get) args=() ;;
        ls|tree) args=(/) ;;
        stat|read) args=(/hello.txt) ;;
        write) args=(/hello.txt --from /dev/null) ;;
        mkdir|rm|rmdir) args=(/hello.txt) ;;
        mv|ln) args=(/hello.txt /other) ;;
        chmod) args=(600 /hello.txt) ;;
        chown) args=(1000:1000 /hello.txt) ;;
        setfattr) args=(user.offset yes /hello.txt) ;;
        rmfattr) args=(user.offset /hello.txt) ;;
        set) args=(label BACKUP); expected=3 ;;
        resize) args=(20G --force); expected=3 ;;
        *) fail "$verb needs an offset coverage row"; continue ;;
    esac
    for bad in '' -1 nope 1.5 18446744073709551616; do
        fs.bcachefs "$img" "$verb" "${args[@]}" --offset="$bad" >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb refuses malformed offset '$bad' with exit 2" test $? -eq 2
        check "$verb usage error has no stdout" test ! -s "$SANDBOX/out"
        check "$verb uses the shared error" jq -e '.code == 2 and (.error | contains("offset"))' "$SANDBOX/err" >/dev/null
    done
    for past in "$size" "$((size + 1))" 18446744073709551615; do
        fs.bcachefs "$SANDBOX/raw.img" "$verb" "${args[@]}" --offset "$past" >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb refuses a past-end device window or has no implementation" test $? -eq "$expected"
        check "$verb applies the offset or refuses before opening" \
            jq -e --argjson code "$expected" '.code == $code and (.error | contains(if $code == 3 then "not implemented" else "--offset" end))' "$SANDBOX/err" >/dev/null
    done
    case "$verb" in
        set|resize)
            fs.bcachefs "$img" "$verb" "${args[@]}" --offset 1048576 >"$SANDBOX/out" 2>"$SANDBOX/err"
            check "$verb at a nonzero offset remains explicitly unsupported" test $? -eq 3 ;;
        info|get|ls|tree|stat|read)
            fs.bcachefs "$FIXTURES/default.img" "$verb" "${args[@]}" >"$SANDBOX/want" 2>"$SANDBOX/err"
            check "$verb reads the reference-made image at zero" test $? -eq 0
            for placement in before-target before-verb after-verb after-path; do
                case "$placement" in
                    before-target) cmd=(--offset 1048576 "$img" "$verb" "${args[@]}") ;;
                    before-verb) cmd=("$img" --offset=1048576 "$verb" "${args[@]}") ;;
                    after-verb) cmd=("$img" "$verb" --offset 1048576 "${args[@]}") ;;
                    after-path) cmd=("$img" "$verb" "${args[@]}" --offset 1048576) ;;
                esac
                fs.bcachefs "${cmd[@]}" >"$SANDBOX/out" 2>"$SANDBOX/err"
                check "$verb $placement reads the embedded filesystem" test $? -eq 0
                check "$verb $placement gives byte-identical output" cmp -s "$SANDBOX/want" "$SANDBOX/out"
            done ;;
    esac
done < <(awk '/^Commands:/{commands=1; next} /^Options:/{commands=0} commands && /^  [a-z]/{print $1}' "$SANDBOX/help")

# Every regular file agrees with the original guest-recorded manifest,
# including an empty file and a file large enough to need multiple reads.
while IFS="$(printf '\t')" read -r path sha; do
    for verb in read cat; do
        fs.bcachefs "$img" "$verb" "$path" --offset 1048576 >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb $path succeeds at the device offset" test $? -eq 0
        got="$(shasum -a 256 <"$SANDBOX/out" | cut -d' ' -f1)"
        check "$verb $path agrees with the guest's SHA-256" test "$got" = "$sha"
    done
done < <(jq -r '.entries[] | select(.type == "file") | [.path, .sha256] | @tsv' "$FIXTURES/default.json")
for verb in read cat; do
    fs.bcachefs "$img" "$verb" /hello.txt --offset 1048576 -o "$SANDBOX/extracted" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "$verb extracts from the selected device" test $? -eq 0
    check "$verb extraction has no stdout" test ! -s "$SANDBOX/out"
    sha="$(jq -r '.entries[] | select(.path == "/hello.txt") | .sha256' "$FIXTURES/default.json")"
    got="$(shasum -a 256 <"$SANDBOX/extracted" | cut -d' ' -f1)"
    check "$verb extraction agrees with the guest's SHA-256" test "$got" = "$sha"
done
for verb in write put; do
    for refusal in / /missing/child; do
        fs.bcachefs "$img" "$verb" "$refusal" --offset 1048576 --from /dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$verb refuses $refusal at the selected offset" test $? -ne 0
        check "$verb refusal preserves the whole container" cmp -s "$img" "$SANDBOX/saved.img"
    done
done
check 'all read and refused commands preserve the container' cmp -s "$img" "$SANDBOX/saved.img"
check 'past-end refusals preserve the entire raw image' cmp -s "$SANDBOX/raw.img" "$FIXTURES/default.img"
finish
