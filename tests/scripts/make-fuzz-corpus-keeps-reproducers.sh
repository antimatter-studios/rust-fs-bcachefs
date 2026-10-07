#!/usr/bin/env bash
#
# make-fuzz-corpus-keeps-reproducers.sh -- rebuilding the seed corpus
# leaves every file it did not generate where it was.
#
# fuzz/corpus/ is not all generated. Beside the seeds cut out of the
# fixtures sit the reproducers for defects the fuzzer found, committed
# there because scripts/fuzz-all.sh says to, so that tests/fuzz_decoders.rs
# replays them on every pull request. A corpus script that began with
# `rm -rf fuzz/corpus` would throw every one of them away on each rebuild
# (the xfs sibling's did, once).
#
# So this runs the real script in a sandbox, against stand-ins for the
# fixtures it checks for and the cargo that cuts the seeds, with a
# reproducer already committed, and checks:
#
#   - the reproducer is still there afterwards, byte for byte;
#   - the script ran to the end: it exits 0 and the seeds it generates
#     were written, so a run that stopped early cannot pass by never
#     reaching the line that would have deleted anything;
#   - a stale generated seed is replaced, not left as it was;
#   - without the fixtures it refuses and names `chore fixtures`.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d "${TMPDIR:-/tmp}/make-fuzz-corpus-test.XXXXXX")"
trap 'rm -rf "$sandbox"' EXIT

mkdir -p "$sandbox/repo/scripts" "$sandbox/repo/fuzz/corpus/superblock" \
         "$sandbox/repo/fixtures" "$sandbox/bin"
cp "$REPO/scripts/make-fuzz-corpus.sh" "$sandbox/repo/scripts/make-fuzz-corpus.sh"

reproducer="$sandbox/repo/fuzz/corpus/superblock/regression-some-finding"
printf 'a committed reproducer' > "$reproducer"
stale="$sandbox/repo/fuzz/corpus/superblock/default"
printf 'stale' > "$stale"

# cargo: the seed cutter. It receives the corpus directory as its last
# argument and writes the seeds examples/fuzz-corpus.rs would.
cat > "$sandbox/bin/cargo" <<'STUB'
#!/usr/bin/env bash
corpus="${@: -1}"
mkdir -p "$corpus/superblock" "$corpus/btree_node" "$corpus/jset"
printf 'fresh' > "$corpus/superblock/default"
printf 'fresh' > "$corpus/btree_node/default-dirents"
printf 'fresh' > "$corpus/jset/aged-unclean-1"
STUB
chmod +x "$sandbox/bin/cargo"

# Without the fixtures: refused, naming the task that makes them.
out="$(cd "$sandbox/repo" && PATH="$sandbox/bin:$PATH" bash scripts/make-fuzz-corpus.sh fixtures 2>&1)"
[ $? -ne 0 ] || fail "the script ran without any fixture image"
grep -q "chore fixtures" <<<"$out" || fail "a missing fixture does not name 'chore fixtures'"

for set in default crc64 xxhash lz4 aged aged-unclean; do
    : > "$sandbox/repo/fixtures/$set.img"
done
out="$(cd "$sandbox/repo" && PATH="$sandbox/bin:$PATH" bash scripts/make-fuzz-corpus.sh fixtures 2>&1)"
status=$?

[ "$status" -eq 0 ] || fail "the script exited $status with every tool it needs stood in"
for seed in superblock/default btree_node/default-dirents jset/aged-unclean-1; do
    [ -s "$sandbox/repo/fuzz/corpus/$seed" ] \
        || fail "the generated seed fuzz/corpus/$seed was not written"
done
if [ ! -f "$reproducer" ]; then
    fail "regenerating the corpus deleted the committed reproducer"
elif [ "$(cat "$reproducer")" != 'a committed reproducer' ]; then
    fail "regenerating the corpus rewrote the committed reproducer"
fi
if printf 'stale' | cmp -s - "$stale"; then
    fail "a generated seed was left stale instead of being regenerated"
fi

if [ "$fails" -gt 0 ]; then
    echo "--- scripts/make-fuzz-corpus.sh said:" >&2
    echo "$out" >&2
    exit 1
fi
echo "PASS  make-fuzz-corpus.sh keeps committed reproducers and regenerates its own seeds"
