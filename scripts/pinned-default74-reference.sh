#!/usr/bin/env bash
# pinned-default74-reference.sh -- the reference checker's own verdict on
# case default-74 (#219), over the digest-pinned fixtures only.
#
# The pinned test proves the image holds an extent whose data does not match
# its stored checksum. This records what the reference checker says about the
# same bytes: `bcachefs fsck -n` on the `default` fixture unchanged (its
# control) and on default-74's mutation, through the differential harness's
# own emitter, container and judge, so the verdict is taken exactly as the
# differential report took it.
#
# It refuses fixtures other than the pinned artifact, fails on a missing tool,
# a missing output or a case that was not judged, and fails unless each
# verdict is the one expected below. A crash or hang of the reference is kept
# as a status only, never its output (AGENTS.md, "A crash in the reference
# stays private"); the judge already discards it, and this then fails.
#
# Evidence, kept by the workflow as an artifact: tmp/default74-reference/
#   evidence.txt          fixture and patch sha256s, statuses, verdicts
#   kept/default-control/ the reference's whole output on the fixture as is
#   kept/default-74/      the patch, the mutation and the reference's output
#   listing/              the reference's `list -b extents` of the control and
#                         of the exact mutated image, its help, and the lines
#                         naming the key the pinned test reported ($OURS_LOG)
set -uo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here" || exit 1

PIN_DIGEST="${PIN_DIGEST:?PIN_DIGEST names the pinned fixtures artifact digest}"
# The verdicts the differential report recorded for default-74: the reference
# passes both the fixture and its mutation. Anything else is a change in the
# claim this case rests on, and fails the run.
EXPECT_CONTROL_STATUS="${EXPECT_CONTROL_STATUS:-0}"
EXPECT_MUTATED_STATUS="${EXPECT_MUTATED_STATUS:-0}"
REF_VERSION=1.39.7
fixtures="$here/.vm-share/fixtures"
out="$here/tmp/default74-reference"

fail() { echo "default74-reference: $*" >&2; exit 1; }

missing=
for tool in docker python3 timeout sha256sum cargo; do
    command -v "$tool" >/dev/null || missing="$missing $tool"
done
[ -z "$missing" ] || fail "this host lacks$missing"
[ -f "$fixtures/default.img" ] || fail "no $fixtures/default.img (the pinned job fetches it)"
[ "$(cat .vm-share/fixtures-pin 2>/dev/null)" = "$PIN_DIGEST" ] ||
    fail ".vm-share/fixtures-pin does not name $PIN_DIGEST"

rm -rf "$out"
mkdir -p "$out"

# One case per fixture from seed 74: each fixture's control and its seed-74
# mutation, the same bytes the 80-case run from seed 0 judged.
cargo build --locked --release --quiet --features write --example differential ||
    fail "the emitter did not build"
timeout 1800 target/release/examples/differential "$fixtures" "$out" 1 74 ||
    fail "the emitter failed on $(cat "$out/inflight.txt" 2>/dev/null || echo 'no case')"
grep -q $'^default-74\tdefault.img\t74\t' "$out/cases.tsv" || fail "the emitter wrote no case default-74"
grep -q $'^default-control\tdefault.img\t' "$out/cases.tsv" || fail "the emitter wrote no control for default.img"

ctr="rust-fs-bcachefs-default74-$$"
cleanup() {
    docker rm -f "$ctr" >/dev/null 2>&1 || true
    rm -rf "$out/scratch"
}
trap cleanup EXIT
docker run -d --name "$ctr" --memory 6g -v "$out:/work" debian:trixie sleep infinity >/dev/null ||
    fail "the reference container did not start"
docker exec -e REF_VERSION="$REF_VERSION" "$ctr" bash -euc '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends ca-certificates curl >/dev/null
    install -d -m 0755 /etc/apt/keyrings
    curl -fsSL -o /etc/apt/keyrings/apt.bcachefs.org.asc https://apt.bcachefs.org/apt.bcachefs.org.asc
    echo "deb [signed-by=/etc/apt/keyrings/apt.bcachefs.org.asc] https://apt.bcachefs.org/trixie bcachefs-tools-release main" \
        >/etc/apt/sources.list.d/apt.bcachefs.org.list
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends "bcachefs-tools=1:$REF_VERSION" >/dev/null
    bcachefs version
' >"$out/reference-install.log" 2>&1 || fail "the reference tools did not install; see $out/reference-install.log"

python3 -I scripts/differential-fuzz.py judge "$out" "$fixtures" --container "$ctr" --jobs 2 --timeout 60 \
    --keep default-control --keep default-74 || fail "the judge failed"

status_of() { tr -d '\n' <"$out/ref/$1.status" 2>/dev/null; }
control_status="$(status_of default-control)"
mutated_status="$(status_of default-74)"
[ -n "$control_status" ] || fail "no reference status for default-control"
[ -n "$mutated_status" ] || fail "no reference status for default-74"

{
    echo "fixtures $PIN_DIGEST"
    echo "reference bcachefs-tools $REF_VERSION: $(tail -n 1 "$out/reference-install.log")"
    echo "default.img sha256:$(sha256sum "$fixtures/default.img" | cut -d' ' -f1)"
    if [ -f "$out/kept/default-74/mutation.patch" ]; then
        echo "default-74 mutation.patch sha256:$(sha256sum "$out/kept/default-74/mutation.patch" | cut -d' ' -f1)"
    fi
    echo "default-74 ours: $(grep $'^default-74\t' "$out/cases.tsv" | cut -f5-7)"
    echo "default-control reference fsck -n status $control_status (expected $EXPECT_CONTROL_STATUS)"
    echo "default-74 reference fsck -n status $mutated_status (expected $EXPECT_MUTATED_STATUS)"
} >"$out/evidence.txt"
cat "$out/evidence.txt"

# A crash or hang left a status and nothing else: the outputs below are
# missing, and the run fails naming the status only.
for cid in default-control default-74; do
    for f in case.json reference.txt; do
        [ -s "$out/kept/$cid/$f" ] || fail "$cid: no kept $f (reference status $(status_of "$cid"))"
    done
done
[ -s "$out/kept/default-74/mutation.patch" ] || fail "default-74: no kept mutation.patch"
[ "$control_status" = "$EXPECT_CONTROL_STATUS" ] ||
    fail "the reference gave the unchanged fixture status $control_status, not $EXPECT_CONTROL_STATUS"
[ "$mutated_status" = "$EXPECT_MUTATED_STATUS" ] ||
    fail "the reference gave default-74 status $mutated_status, not $EXPECT_MUTATED_STATUS"
echo "default74-reference: the reference's verdicts match on the pinned fixtures"

# The reference's own listing of the extents btree, on the fixture as is and
# on default-74's exact mutated bytes (the fixture with the kept patch laid
# over it, as the judge lays it). The key the pinned test reported, in the
# reference lister's spelling (src/bkey.rs), must be listed in the mutated
# image carrying the stored checksum the test reported. A crash or hang of
# the lister keeps its status only, as the judge does.
[ -n "${OURS_LOG:-}" ] && [ -s "$OURS_LOG" ] || fail "OURS_LOG names no log of the pinned test"
key_lines="$(grep -E '^default-74: extent_data key [^ ]+ stored 0x[0-9a-f]+ computed 0x[0-9a-f]+$' "$OURS_LOG")"
[ "$(printf '%s\n' "$key_lines" | grep -c .)" = 1 ] ||
    fail "the pinned test's log names $(printf '%s\n' "$key_lines" | grep -c .) extent_data keys, not 1"
key="$(printf '%s\n' "$key_lines" | cut -d' ' -f4)"
stored="$(printf '%s\n' "$key_lines" | cut -d' ' -f6)"
stored_hex="${stored#0x}"

mkdir -p "$out/scratch" "$out/listing"
cp "$fixtures/default.img" "$out/scratch/default-control.img" || fail "the control image was not copied"
python3 -I - "$fixtures/default.img" "$out/kept/default-74/mutation.patch" "$out/scratch/default-74.img" <<'EOF' ||
import shutil, struct, sys
src, patch, dst = sys.argv[1:4]
shutil.copyfile(src, dst)
with open(patch, "rb") as p:
    data = p.read()
with open(dst, "r+b") as f:
    i = 0
    while i < len(data):
        at, n = struct.unpack_from("<QI", data, i)
        i += 12
        f.seek(at)
        f.write(data[i : i + n])
        i += n
EOF
    fail "the mutated image was not laid from the kept patch"
control_sha="$(sha256sum "$out/scratch/default-control.img" | cut -d' ' -f1)"
mutated_sha="$(sha256sum "$out/scratch/default-74.img" | cut -d' ' -f1)"
[ "$control_sha" != "$mutated_sha" ] || fail "the kept patch leaves default.img unchanged"

docker exec "$ctr" bcachefs list --help >"$out/listing/list-help.txt" 2>&1
echo "$?" >"$out/listing/list-help.status"
listing_status() { tr -d '\n' <"$out/listing/$1.status" 2>/dev/null; }
for cid in default-control default-74; do
    docker exec "$ctr" timeout -k 5 300 bcachefs list -b extents "/work/scratch/$cid.img" \
        >"$out/listing/$cid.extents.txt" 2>&1
    s=$?
    echo "$s" >"$out/listing/$cid.status"
    if [ "$s" = 124 ] || [ "$s" -ge 128 ]; then
        rm -f "$out/listing/$cid.extents.txt"
        fail "the reference lister crashed or hung on $cid (status $s)"
    fi
done
grep -F -- "$key " "$out/listing/default-control.extents.txt" >"$out/listing/default-control.key.txt"
grep -F -- "$key " "$out/listing/default-74.extents.txt" >"$out/listing/default-74.key.txt"

{
    echo "default-74 mutated image sha256:$mutated_sha"
    echo "default-control image sha256:$control_sha"
    echo "default-74 ours: extent_data key $key stored $stored ($key_lines)"
    echo "reference list -b extents status: control $(listing_status default-control), default-74 $(listing_status default-74)"
    echo "reference listing of $key in default-control:"
    sed 's/^/  /' "$out/listing/default-control.key.txt"
    echo "reference listing of $key in default-74:"
    sed 's/^/  /' "$out/listing/default-74.key.txt"
} | tee -a "$out/evidence.txt"

for cid in default-control default-74; do
    [ "$(listing_status "$cid")" = 0 ] ||
        fail "the reference lister gave $cid status $(listing_status "$cid"); see listing/$cid.extents.txt"
    [ -s "$out/listing/$cid.extents.txt" ] || fail "the reference lister printed nothing for $cid"
done
[ -s "$out/listing/default-74.key.txt" ] ||
    fail "the reference lists no key $key in default-74's mutated image"
grep -qiF -- "$stored_hex" "$out/listing/default-74.key.txt" ||
    fail "the reference lists $key in default-74 without the stored checksum $stored"
echo "default74-reference: the reference lists $key with stored checksum $stored in image sha256:$mutated_sha"
