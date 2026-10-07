#!/usr/bin/env bash
# Rebuild fuzz/corpus from the fixtures the reference tools made.
#
# The corpus is not a pile of random bytes. Every seed is a structure the
# reference formatter or the reference implementation wrote, cut out of a
# fixture image at the place this crate's reader found it (the cutting is
# examples/fuzz-corpus.rs). That is what makes mutation productive:
# flipping a field in a structure that is otherwise valid reaches the
# decoder's interesting paths, where random bytes are refused by the magic
# number on the first line and never reach anything.
#
# The images come from `chore fixtures`, which runs the reference tools
# inside the harness VM; nothing of theirs runs here. This script only
# needs the images to exist on the share.
#
# It rewrites only the seeds it generates. Anything else under
# fuzz/corpus -- a committed reproducer above all -- is left alone:
# scripts/fuzz-all.sh says to commit each reproducer beside the seeds so
# tests/fuzz_decoders.rs replays it on every pull request, and a rebuild
# that threw them away would un-fix every defect the fuzzer ever found.
# tests/scripts/make-fuzz-corpus-keeps-reproducers.sh holds it to that.
#
# Usage: scripts/make-fuzz-corpus.sh [fixtures-dir]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"
fixtures="${1:-.vm-share/fixtures}"

for set in default crc64 xxhash lz4 aged aged-unclean; do
    [ -f "$fixtures/$set.img" ] || {
        echo "make-fuzz-corpus: $fixtures/$set.img is missing: build the fixtures with 'chore fixtures'" >&2
        exit 1
    }
done

cargo run --quiet --release --example fuzz-corpus -- "$fixtures" fuzz/corpus
echo "fuzz/corpus rebuilt from $fixtures; committed reproducers untouched"
