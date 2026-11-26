#!/usr/bin/env bash
# `fs.bcachefs import` streams: its peak memory does not grow with its input
# (#704, WRT-01), from a file, from stdin and from a tree. Each large import
# is measured against a 1 MiB one of the same kind, so what the tool needs to
# open the image is not counted against it. A program that reads the same
# input whole is the control: the measurement must see that.
source "$(dirname "$0")/lib.sh"
need_image aged
base="$FIXTURES/aged.img"

# The input, and how much more memory than a 1 MiB import a large one may
# use. The tool gathers at most 16 MiB before it commits and copies a run to
# commit it, so twice that and the extents' bookkeeping stay under the bound,
# which is well under the input.
LARGE_MIB=64
BOUND_KIB=$((48 * 1024))

# peak_kib STDIN COMMAND...: COMMAND's peak resident memory in KiB on stdout,
# its own output discarded, its stderr kept in $SANDBOX/err and its status
# returned. The measuring program is a file, so stdin stays free for COMMAND.
cat >"$SANDBOX/peak.py" <<'PEAK'
import resource, subprocess, sys
with open(sys.argv[1], "wb") as err:
    status = subprocess.run(sys.argv[2:], stdout=subprocess.DEVNULL, stderr=err).returncode
print(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)
sys.exit(status)
PEAK
peak_kib() {
    local input="$1"
    shift
    python3 "$SANDBOX/peak.py" "$SANDBOX/err" "$@" <"$input"
}

bytes() {
    python3 -c 'import random, sys; r = random.Random(int(sys.argv[2])); n = int(sys.argv[1]) << 20
while n:
    step = min(n, 1 << 20); sys.stdout.buffer.write(r.randbytes(step)); n -= step' "$1" "$2"
}
bytes 1 704 >"$SANDBOX/small"
bytes "$LARGE_MIB" 705 >"$SANDBOX/large"
mkdir -p "$SANDBOX/small-tree" "$SANDBOX/large-tree"
bytes 1 706 >"$SANDBOX/small-tree/one"
for n in 1 2 3 4; do bytes $((LARGE_MIB / 4)) $((706 + n)) >"$SANDBOX/large-tree/part-$n"; done

# bounded WHAT SMALL_PEAK LARGE_PEAK
bounded() {
    check "$1: a ${LARGE_MIB} MiB import peaked at $3 KiB, a 1 MiB one at $2 KiB" \
        test "$(($3 - $2))" -lt "$BOUND_KIB"
}

img="$SANDBOX/memory.img"

# A local file, with the staged copy read back before it is published.
cp --sparse=always "$base" "$img"
small="$(peak_kib /dev/null fs.bcachefs "$img" import --verify "$SANDBOX/small" /memory-small)"
status=$?
check "the small file import ($(cat "$SANDBOX/err"))" test "$status" -eq 0
large="$(peak_kib /dev/null fs.bcachefs "$img" import --verify "$SANDBOX/large" /memory-large)"
status=$?
check "the large file import ($(cat "$SANDBOX/err"))" test "$status" -eq 0
bounded 'a file' "$small" "$large"
fs.bcachefs "$img" read /memory-large >"$SANDBOX/back" 2>/dev/null
check 'the large file import reads back whole' cmp -s "$SANDBOX/large" "$SANDBOX/back"

# stdin.
cp --sparse=always "$base" "$img"
small="$(peak_kib "$SANDBOX/small" fs.bcachefs "$img" import - /memory-small)"
status=$?
check "the small stdin import ($(cat "$SANDBOX/err"))" test "$status" -eq 0
large="$(peak_kib "$SANDBOX/large" fs.bcachefs "$img" import - /memory-large)"
status=$?
check "the large stdin import ($(cat "$SANDBOX/err"))" test "$status" -eq 0
bounded 'stdin' "$small" "$large"
fs.bcachefs "$img" read /memory-large >"$SANDBOX/back" 2>/dev/null
check 'the large stdin import reads back whole' cmp -s "$SANDBOX/large" "$SANDBOX/back"

# A tree.
cp --sparse=always "$base" "$img"
small="$(peak_kib /dev/null fs.bcachefs "$img" import --recursive "$SANDBOX/small-tree" /memory-small)"
status=$?
check "the small tree import ($(cat "$SANDBOX/err"))" test "$status" -eq 0
large="$(peak_kib /dev/null fs.bcachefs "$img" import --recursive "$SANDBOX/large-tree" /memory-large)"
status=$?
check "the large tree import ($(cat "$SANDBOX/err"))" test "$status" -eq 0
bounded 'a tree' "$small" "$large"
fs.bcachefs "$img" read /memory-large/part-4 >"$SANDBOX/back" 2>/dev/null
check 'the large tree import reads back whole' cmp -s "$SANDBOX/large-tree/part-4" "$SANDBOX/back"

# The control: reading the input whole is over the bound.
read_whole='import sys; data = open(sys.argv[1], "rb").read(); assert data'
small="$(peak_kib /dev/null python3 -c "$read_whole" "$SANDBOX/small")"
large="$(peak_kib /dev/null python3 -c "$read_whole" "$SANDBOX/large")"
check "the control: reading the input whole is seen ($small KiB, then $large KiB)" \
    test "$((large - small))" -ge "$BOUND_KIB"

finish
