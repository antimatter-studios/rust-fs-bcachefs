#!/usr/bin/env bash
# The Linux kernel wrote and SHA-256 digested these files in the independent
# reference VM. Snapshot paths must retain their resolved identity.
source "$(dirname "$0")/lib.sh"
need_fixture kernel

for path in /empty /big/transfer.bin /sv/changed.txt /snap/changed.txt; do
    sha="$(jq -r --arg p "$path" '.entries[] | select(.path == $p) | .sha256' "$FIXTURES/kernel.json")"
    size="$(jq -r --arg p "$path" '.entries[] | select(.path == $p) | .size' "$FIXTURES/kernel.json")"
    fs.bcachefs "$FIXTURES/kernel.img" export --replace "$path" "$SANDBOX/file" >"$SANDBOX/report" 2>"$SANDBOX/err"
    check "kernel export $path succeeds" test $? -eq 0
    got="$(shasum -a 256 <"$SANDBOX/file" | cut -d' ' -f1)"
    check "kernel export $path matches its native digest" test "$got" = "$sha"
    check 'kernel export report matches the native manifest' jq -e --arg s "$sha" --argjson n "$size" '.bytes == $n and .sha256 == $s' "$SANDBOX/report" >/dev/null
    check 'kernel export has no stderr' test ! -s "$SANDBOX/err"
done
check 'kernel fixture holds 3 MiB plus one byte' jq -e '.entries[] | select(.path == "/big/transfer.bin") | .size == 3145729' "$FIXTURES/kernel.json" >/dev/null
finish
