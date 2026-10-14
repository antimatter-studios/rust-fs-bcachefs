#!/usr/bin/env bash
# Differential fuzzing's judge and classifier (#164), scripts/differential-fuzz.py,
# against a stand-in for the reference checker: no docker, no fixture, no
# toolchain. The stand-in answers by the byte each case's patch lays at the
# start of a three-MiB image, and refuses an image laid wrong.
#
# It must:
# - count a finding the reference printed under exit status 0 ("repair
#   success") as a finding, and its usual lines on a clean image as none;
# - take a note it prints on the fixture unchanged (its control) as that
#   fixture's normal, not as a finding in every case laid over it;
# - fail on a disagreement class the baseline does not list, name it, and
#   pass once the baseline lists it (or lists it for any structure, `*`);
# - fail on a control that is not clean to both, and on a panic here;
# - keep nothing of a case on which the reference crashed, hung or faulted:
#   no output, no patch, and no seed in the report or on stdout;
# - and run the reference at the version the other oracles use.
#
#   bash tests/scripts/differential-fuzz-classifies.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PY="$REPO/scripts/differential-fuzz.py"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/differential.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/fixtures" "$tmp/out/patches"

# The fixture: `c` at byte 0, data in its second MiB, a hole elsewhere.
python3 - "$tmp/fixtures/tiny.img" <<'PY'
import sys
with open(sys.argv[1], "wb") as f:
    f.write(b"c")
    f.seek(1 << 20)
    f.write(b"DATA" * 1024)
    f.truncate(3 << 20)
PY

# The reference's own lines on a clean image, as a fixture's fsck.txt has them.
cat >"$tmp/clean.txt" <<'EOF'
Running userspace offline fsck
Using encoding defined by superblock: utf8-12.1.0
starting version per_dev_fragmentation_lru (1.39)
  with options: degraded=yes,fsck,fix_errors=no,nochanges,nodirect_io,read_only,noreconcile_enabled
recovering from clean shutdown, journal seq 42
Journal keys: 0 read, 0 after sorting and compacting
check_alloc_info... done (0 seconds)
going read-write
check_extents_to_backpointers...scanning for missing backpointers in 0/2048 buckets, 2 buckets with no backpointers
 done (0 seconds)
clean shutdown complete, journal seq 45
EOF

# This fixture's own note, printed on it unchanged (with the image's name)
# and so on every case laid over it that the note survives.
cat >"$tmp/reference" <<EOF
#!/usr/bin/env bash
img="\$1"
[ "\$(wc -c <"\$img" | tr -d ' ')" = 3145728 ] || { echo "laid wrong: size"; exit 125; }
[ "\$(dd if="\$img" bs=1048576 skip=1 count=1 2>/dev/null | head -c 4)" = DATA ] || { echo "laid wrong: data"; exit 125; }
note() { echo "\$(basename "\$img") has \$1k btree buckets and 4.75M marked in bitmap"; echo "  scheduling recovery pass btree_bitmap_gc (49)"; }
case "\$(head -c 1 "\$img")" in
    c) cat "$tmp/clean.txt"; note 608 ;;
    r) cat "$tmp/clean.txt"; note 612; echo "checksum error at btree node 12 offset 0x40: repair success (rewriting node)" ;;
    e) echo "  u64s 7 type inode 0:4096:1"; echo "bad btree header at 7/24, fixing"; exit 9 ;;
    s) kill -SEGV \$\$ ;;
    a) echo "BUG at fs/thing.c:123"; exit 1 ;;
    h) exec sleep 30 ;;
esac
EOF
chmod +x "$tmp/reference"

# id fixture seed structure what ours signature detail, and the byte laid.
while IFS='|' read -r id seed structure ours sig marker; do
    printf '%s\ttiny.img\t%s\t%s\ta change\t%s\t%s\t%s\n' \
        "$id" "$seed" "$structure" "$ours" "$sig" "${sig:+what it said}" >>"$tmp/out/cases.tsv"
    python3 - "$tmp/out/patches/$id.patch" "$marker" <<'PY'
import struct, sys
with open(sys.argv[1], "wb") as f:
    if sys.argv[2]:
        f.write(struct.pack("<QI", 0, 1) + sys.argv[2].encode())
PY
done <<'EOF'
tiny-control|-|control|clean||
tiny-1|1|node|clean||r
tiny-2|2|node|found|check:node_header|e
tiny-3|3|jset|clean||e
tiny-4|4|superblock|found|open:bad-checksum(superblock)|c
tiny-987654|987654|node|clean||s
tiny-876543|876543|node|clean||a
tiny-765432|765432|node|clean||h
EOF

python3 "$PY" judge "$tmp/out" "$tmp/fixtures" --reference "$tmp/reference" --jobs 3 --timeout 2 \
    >"$tmp/judge.txt" 2>&1 || fail "the judge failed: $(cat "$tmp/judge.txt")"

for id in tiny-987654 tiny-876543 tiny-765432; do
    [ ! -e "$tmp/out/ref/$id.txt" ] || fail "$id: the reference crashed, hung or faulted, and its output was kept"
    [ ! -e "$tmp/out/patches/$id.patch" ] || fail "$id: the reference crashed, hung or faulted, and its patch was kept"
done
[ ! -d "$tmp/out/scratch" ] || [ -z "$(ls -A "$tmp/out/scratch")" ] || fail "scratch images were left behind"

printf 'ref-only\tnode\tchecksum error at btree node # offset #: repair success (rewriting node)\t#0\n' >"$tmp/baseline"
printf 'ours-only\t*\topen:bad-checksum(superblock)\texpected\n' >>"$tmp/baseline"
classify() {
    python3 "$PY" classify "$tmp/out" --baseline "$tmp/baseline" --report "$tmp/report.md" --version 9.9 >"$tmp/stdout" 2>&1
}

classify && fail "a disagreement class missing from the baseline passed: $(cat "$tmp/stdout")"
grep -q 'NEW ref-only / jset / bad btree header at #, fixing' "$tmp/stdout" ||
    fail "the new class is not named on stdout: $(cat "$tmp/stdout")"
for want in '|both clean|1|' '|both found something|1|' "|the reference found something; this crate called it clean|2|" \
    '|this crate found something; the reference did not|1|' '|the reference crashed, hung or faulted (counts only)|3|'; do
    tr -d ' ' <"$tmp/report.md" | grep -qF "$(printf '%s' "$want" | tr -d ' ')" || fail "the report does not count $want"
done
grep -q 'tiny-control.img has 608k btree buckets' "$tmp/report.md" ||
    fail "the report does not show what the reference printed on the fixture unchanged"
for secret in 987654 876543 765432 thing.c; do
    ! grep -q "$secret" "$tmp/report.md" "$tmp/stdout" || fail "the report or stdout names $secret, of a case the reference crashed on"
done

printf 'ref-only\tjset\tbad btree header at #, fixing\t#0\n' >>"$tmp/baseline"
classify || fail "every class is in the baseline, and the run failed: $(cat "$tmp/stdout")"

# A control this crate calls damaged.
cp "$tmp/out/cases.tsv" "$tmp/cases.tsv"
awk -F'\t' -v OFS='\t' '$1 == "tiny-control" { $6 = "found" } 1' "$tmp/cases.tsv" >"$tmp/out/cases.tsv"
classify && fail "a control that is not clean to both passed"
grep -q 'control(s) not clean to both' "$tmp/stdout" || fail "the control is not named: $(cat "$tmp/stdout")"
cp "$tmp/cases.tsv" "$tmp/out/cases.tsv"

# A panic here.
printf 'tiny-9\ttiny.img\t9\tunchanged\t(the case panicked)\tpanicked\tpanic at src/x.rs:1\tboom\n' >>"$tmp/out/cases.tsv"
classify && fail "a panic in this crate passed"

# The same reference everywhere.
versions="$(grep -h '^REF_VERSION=' "$REPO/scripts/vm-setup.sh" "$REPO/scripts/kernel-oracle.sh" \
    "$REPO/scripts/differential-fuzz.sh" | sort -u)"
[ "$(printf '%s\n' "$versions" | wc -l | tr -d ' ')" = 1 ] ||
    fail "the reference tools' version differs between vm-setup.sh, kernel-oracle.sh and differential-fuzz.sh: $versions"

# Lines the reference 1.39.7 printed in the nightly run of 648 cases. On
# aged.img seed 3 (exit 0) its only line outside the usual ones was a pass's
# progress broken by its own header; the rest are real findings it printed
# behind a pass's progress or alone, and must stay findings.
python3 - "$PY" >"$tmp/recorded.txt" 2>&1 <<'PY' || fail "a recorded reference line is judged wrong: $(cat "$tmp/recorded.txt")"
import importlib.util, sys
spec = importlib.util.spec_from_file_location("differential", sys.argv[1])
d = importlib.util.module_from_spec(spec)
spec.loader.exec_module(d)
bad = 0
for line in ("check_allocations...Running userspace offline fsck",):
    if d.findings(line):
        print(f"a pass's progress broken by the reference's header is a finding: {line!r}")
        bad += 1
for line in (
    "check_btree_backpointers...backpointer for nonexistent alloc key: 0:4398046511103:0",
    "check_alloc_info...hole in alloc btree missing in freespace btree",
    "check_extents...incorrect i_sectors: got 18446744073709551615, should be 1",
    "invalid bkey in btree_node btree=logged_ops level=0: u64s 7 type inode_alloc_cursor 1:1:0 len 1 ver 0  : min 2147483648 max 9223372036854775807 consumed 314 idx 2147483962 generation 0",
    "  size != 0, deleting",
    "  bkey_size_nonzero    1",
    # Not recorded: a pass's progress with nothing after it is no usual line.
    "check_allocations...",
):
    if d.findings(line) != [line]:
        print(f"a real finding is not a finding: {line!r}")
        bad += 1
sys.exit(bad)
PY

if [ "$fails" -gt 0 ]; then
    echo "differential-fuzz-classifies: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  the differential classifier counts by printed findings, fails on a new class and keeps nothing of a reference crash"
