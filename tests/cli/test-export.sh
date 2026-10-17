#!/usr/bin/env bash
# Export the bytes the independent reference formatter imported and the
# guest digested; never use this driver's own read verb as the oracle.
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/offset-lib.sh"

for set in default lz4 zstd gzip; do
    need_fixture "$set"
    img="$FIXTURES/$set.img"
    manifest="$FIXTURES/$set.json"
    for path in /empty /hello.txt /big/transfer.bin; do
        dest="$SANDBOX/$set-$(basename "$path")"
        sha="$(jq -r --arg p "$path" '.entries[] | select(.path == $p) | .sha256' "$manifest")"
        size="$(jq -r --arg p "$path" '.entries[] | select(.path == $p) | .size' "$manifest")"
        if fs.bcachefs "$img" export "$path" "$dest" >"$SANDBOX/report" 2>"$SANDBOX/err"; then
            ok
            got="$(shasum -a 256 <"$dest" | cut -d' ' -f1)"
            check "$set: export $path matches the guest digest" test "$got" = "$sha"
            check "$set: export report matches the guest" jq -e --arg s "$sha" --argjson n "$size" '.bytes == $n and .sha256 == $s and .replaced == false' "$SANDBOX/report" >/dev/null
            check "$set: success has no stderr" test ! -s "$SANDBOX/err"
        else
            fail "$set: export $path failed: $(cat "$SANDBOX/err")"
        fi
        cp "$dest" "$SANDBOX/saved"
        fs.bcachefs "$img" export "$path" "$dest" >"$SANDBOX/out" 2>"$SANDBOX/err"
        check "$set: existing destination is refused" test $? -eq 1
        check "$set: refusal preserves destination" cmp -s "$dest" "$SANDBOX/saved"
        printf 'old destination\n' >"$dest"
        fs.bcachefs "$img" export --replace "$path" "$dest" >"$SANDBOX/report" 2>"$SANDBOX/err"
        check "$set: explicit replacement succeeds" test $? -eq 0
        check "$set: replacement is complete" cmp -s "$dest" "$SANDBOX/saved"
        check "$set: replacement is reported" jq -e '.replaced == true' "$SANDBOX/report" >/dev/null
        fs.bcachefs "$img" export "$path" - >"$SANDBOX/raw" 2>"$SANDBOX/err"
        check "$set: stdout succeeds" test $? -eq 0
        check "$set: stdout contains only file bytes" cmp -s "$dest" "$SANDBOX/raw"
        check "$set: stdout has no stderr" test ! -s "$SANDBOX/err"
    done
done

img="$FIXTURES/default.img"
# Require the boundary case explicitly, rather than silently using an old fixture.
check 'the guest recorded 3 MiB plus one byte' jq -e '.entries[] | select(.path == "/big/transfer.bin") | .size == 3145729' "$FIXTURES/default.json" >/dev/null
for path in / /link /no-such-file; do
    printf 'keep destination\n' >"$SANDBOX/saved"
    cp "$SANDBOX/saved" "$SANDBOX/refused"
    fs.bcachefs "$img" export --replace "$path" "$SANDBOX/refused" >"$SANDBOX/out" 2>"$SANDBOX/err"
    check "export refuses $path" test $? -eq 1
    check 'refusal has no stdout' test ! -s "$SANDBOX/out"
    check 'refusal preserves destination' cmp -s "$SANDBOX/saved" "$SANDBOX/refused"
done
fs.bcachefs "$img" export --replace /empty - >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'replace with stdout is a usage error' test $? -eq 2

# Embed without interpreting bcachefs structures; the device window must
# apply to the new verb just as it applies to read and stat.
offset_embed "$img" "$SANDBOX/embedded.img" 1048576
fs.bcachefs "$SANDBOX/embedded.img" export /big/transfer.bin "$SANDBOX/offset" --offset 1048576 >"$SANDBOX/report" 2>"$SANDBOX/err"
check 'export honours device offset' test $? -eq 0
check 'offset export matches independently formatted bytes' cmp -s "$SANDBOX/offset" "$SANDBOX/default-transfer.bin"

# Damage a raw data span after the first transfer chunk. Locate it by the
# deterministic source bytes, not by this driver's interpretation of extents.
python3 - "$img" "$SANDBOX/damaged.img" <<'PY'
import mmap, random, shutil, sys
rng = random.Random(282)
data = bytes(rng.getrandbits(8) for _ in range(3 * 1024 * 1024 + 1))
needle = data[2 * 1024 * 1024:2 * 1024 * 1024 + 4096]
shutil.copyfile(sys.argv[1], sys.argv[2])
with open(sys.argv[2], 'r+b') as f, mmap.mmap(f.fileno(), 0) as image:
    pos = image.find(needle)
    assert pos >= 0, 'reference fixture has no raw transfer data; run chore fixtures'
    assert image.find(needle, pos + 1) == -1, 'raw source span is not unique'
    image[pos + 111] ^= 1
PY
check 'the independent source span was damaged' test $? -eq 0
for replace in no yes; do
    dest="$SANDBOX/damage-$replace"
    if [ "$replace" = yes ]; then
        cp "$SANDBOX/saved" "$dest"
        fs.bcachefs "$SANDBOX/damaged.img" export --replace /big/transfer.bin "$dest" >"$SANDBOX/out" 2>"$SANDBOX/err"
    else
        fs.bcachefs "$SANDBOX/damaged.img" export /big/transfer.bin "$dest" >"$SANDBOX/out" 2>"$SANDBOX/err"
    fi
    check 'damaged extent fails export' test $? -eq 1
    check 'failure has no success report' test ! -s "$SANDBOX/out"
    check 'failure is a shared JSON error' jq -e '.code == 1 and (.error | type) == "string"' "$SANDBOX/err" >/dev/null
    if [ "$replace" = yes ]; then
        check 'failed replacement preserves existing bytes' cmp -s "$SANDBOX/saved" "$dest"
    else
        check 'failed export publishes no destination' test ! -e "$dest"
    fi
    leftovers="$(find "$SANDBOX" -name '.*.partial' -print)"
    check 'failure leaves no staging file' test -z "$leftovers"
done

finish
