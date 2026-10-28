#!/usr/bin/env bash
# fs.bcachefs import (#439), as installed, on copies of the write-study
# base: one local file or stdin streamed through the shared file sink, a
# file published only when complete, an existing name refused without
# --replace and replaced with it, and nothing staged left behind. What was
# imported is read back here with the read verbs, which only shows the
# verb's contract; the reference mount's and the reference checker's
# verdict on the same imports is tests/write_oracle.rs, in the guest.
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/offset-lib.sh"

base="$FIXTURES/write-study/base.img"
if [ ! -f "$base" ]; then
    echo "FAIL  $NAME: $base is missing -- build the fixtures with \`chore fixtures\`" >&2
    echo "test result: FAILED. 0 passed; 1 failed"
    exit 1
fi

# sha FILE: FILE's SHA-256, computed here and not by the tool.
sha() { shasum -a 256 <"$1" | cut -d' ' -f1; }
# jreport [JQ OPTION...] FILTER: the last report satisfies FILTER.
jreport() { jq -e "$@" "$SANDBOX/report" >/dev/null; }
# staged IMAGE: the staged names left in /d, one per line.
staged() { fs.bcachefs "$1" ls /d 2>/dev/null | grep -F '.fs-bcachefs-import-' || true; }

fs.bcachefs image.img import --help >"$SANDBOX/help" 2>"$SANDBOX/err"
check 'import help succeeds without opening an image' test $? -eq 0
check 'help has an import example' grep -q 'fs.bcachefs disk.img import hello.txt /hello.txt' "$SANDBOX/help"
check 'help documents replacement' grep -q -- '--replace' "$SANDBOX/help"
check 'help documents verification' grep -q -- '--verify' "$SANDBOX/help"

img="$SANDBOX/base.img"
cp "$base" "$img"

# A file larger than one transfer chunk, from a local file.
python3 -c 'import random, sys; r = random.Random(439); sys.stdout.buffer.write(bytes(r.getrandbits(8) for _ in range(3 * 1024 * 1024 + 1)))' >"$SANDBOX/big"
fs.bcachefs "$img" import "$SANDBOX/big" /d/big.bin >"$SANDBOX/report" 2>"$SANDBOX/err"
status=$?
check "import a local file ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'the report carries the source digest' jreport --arg s "$(sha "$SANDBOX/big")" '.bytes == 3145729 and .sha256 == $s and .created == true and .replaced == false'
fs.bcachefs "$img" read /d/big.bin >"$SANDBOX/back" 2>/dev/null
check 'the imported file reads back whole' cmp -s "$SANDBOX/big" "$SANDBOX/back"

# stdin, and the empty file.
printf 'from stdin\n' | fs.bcachefs "$img" import - /d/stdin.txt >"$SANDBOX/report" 2>"$SANDBOX/err"
status=$?
check "import from stdin ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'stdin is read back' test "$(fs.bcachefs "$img" read /d/stdin.txt)" = 'from stdin'
fs.bcachefs "$img" import - /d/empty </dev/null >"$SANDBOX/report" 2>"$SANDBOX/err"
status=$?
check "import an empty stdin ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'the empty import is reported empty' jreport '.bytes == 0'

# An existing name is refused, and the image is not changed.
cp "$img" "$SANDBOX/before.img"
printf 'new bytes\n' >"$SANDBOX/new"
fs.bcachefs "$img" import "$SANDBOX/new" /d/existing >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'an existing name is refused without --replace' test $? -eq 1
check 'the refusal has no report' test ! -s "$SANDBOX/out"
check 'the refusal changes no byte of the image' cmp -s "$img" "$SANDBOX/before.img"

# With --replace and --verify it is replaced, by one rename.
fs.bcachefs "$img" import --replace --verify "$SANDBOX/new" /d/existing >"$SANDBOX/report" 2>"$SANDBOX/err"
status=$?
check "--replace --verify succeeds ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'the replacement is reported and verified' jreport '.replaced == true and .created == false and .verified == true'
check 'the replaced name reads the new bytes' test "$(fs.bcachefs "$img" read /d/existing)" = 'new bytes'

# Refusals: a directory is not replaced, a missing parent is not created.
cp "$img" "$SANDBOX/before.img"
fs.bcachefs "$img" import --replace "$SANDBOX/new" /d >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'a directory is not replaced' test $? -ne 0
fs.bcachefs "$img" import "$SANDBOX/new" /no-such-dir/file >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'a missing parent directory is refused' test $? -ne 0
check 'the refusals change no byte of the image' cmp -s "$img" "$SANDBOX/before.img"
fs.bcachefs "$img" import "$SANDBOX/no-such-source" /d/never >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'a missing source is refused' test $? -ne 0
check 'a missing source publishes nothing' test -z "$(fs.bcachefs "$img" ls /d 2>/dev/null | grep -F never || true)"
check 'no staged name is left behind' test -z "$(staged "$img")"

# The device window applies to import as to every verb.
offset_embed "$base" "$SANDBOX/embedded.img" 1048576
fs.bcachefs "$SANDBOX/embedded.img" import "$SANDBOX/big" /d/big.bin --offset 1048576 >"$SANDBOX/report" 2>"$SANDBOX/err"
status=$?
check "import honours the device offset ($(cat "$SANDBOX/err"))" test "$status" -eq 0
check 'the container around the device is untouched' offset_extract "$SANDBOX/embedded.img" "$SANDBOX/extracted.img" 1048576 "$(wc -c <"$base" | tr -d ' ')"
fs.bcachefs "$SANDBOX/extracted.img" read /d/big.bin >"$SANDBOX/back" 2>/dev/null
check 'the offset import reads back whole' cmp -s "$SANDBOX/big" "$SANDBOX/back"

finish
