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
#   read/                 the reference's help, and its FUSE-mounted read of the
#                         reported key's inode on the control and the mutation
set -uo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here" || exit 1

fail() { echo "default74-reference: $*" >&2; exit 1; }

# The pinned test's one observation, printed as "key stored computed". The
# live path below and scripts/pinned-default74-observation-controls.sh both
# read it through this function, so the controls exercise the live parse.
observed_tuple() {
    local lines n
    [ -n "${1:-}" ] && [ -s "$1" ] || fail "OURS_LOG names no log of the pinned test"
    lines="$(grep -E '^default-74: extent_data key [^ ]+ stored 0x[0-9a-f]+ computed 0x[0-9a-f]+$' "$1")"
    n="$(printf '%s\n' "$lines" | grep -c .)"
    [ "$n" = 1 ] || fail "the pinned test's log names $n extent_data keys, not 1"
    printf '%s\n' "$lines" | cut -d' ' -f4,6,8
}
if [ "${1:-}" = --observed-tuple ]; then
    observed_tuple "${2:-}"
    exit
fi

# Replay the same final verdict over kept observations in CI. This entry
# point deliberately shares the live job's grader; it does not run a tool,
# mount a filesystem or manufacture an independent reference observation.
grade_read_evidence() (
    python3 -I - "$1" "$2" <<'PYGRADE' | tee -a "$1/evidence.txt"
from pathlib import Path
import re
import sys

out = Path(sys.argv[1])
inode = sys.argv[2]

def require(condition, reason):
    if not condition:
        raise SystemExit("default74-reference: " + reason)

def read(relative):
    path = out / relative
    require(path.is_file(), "missing observation " + relative)
    return path.read_text()

require(inode == "2147483904", "the observed extent belongs to inode 2147483904")
evidence = read("evidence.txt").splitlines()
for expected in [
    "fixtures sha256:9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e",
    "reference bcachefs-tools 1.39.7: 1.39.7",
    "default.img sha256:70e75cfbe82c927f6914689a0142c0e37e99cae289819d86fbacf62f6ee78342",
    "default-74 mutation.patch sha256:ee572579a8b995cbe6a8950a759560baec5297638f12dcc95f0981ab84c68c67",
    "default-control reference fsck -n status 0 (expected 0)",
    "default-74 reference fsck -n status 0 (expected 0)",
    "default-74 mutated image sha256:3b58375372070ca854c7600c54cca29d64a51921d68facea8eabaf4285e433eb",
    "default-control image sha256:70e75cfbe82c927f6914689a0142c0e37e99cae289819d86fbacf62f6ee78342",
    "reference crc32c of 2147483904:1:U32_MAX: default-74 0x11ba7, default-control 0xa2f1ba7",
]:
    require(evidence.count(expected) == 1, "missing or misbound evidence: " + expected)
require(read("read/fusemount-help.status") == "0\n", "reference data reader help failed")
for cid, checksum in [("default-control", "a2f1ba7"), ("default-74", "11ba7")]:
    require(read(f"listing/{cid}.status") == "0\n", cid + " listing failed")
    record = read(f"listing/{cid}.key.txt")
    headers = [line.split() for line in record.splitlines() if line.startswith("u64s ")]
    require(len(headers) == 1 and headers[0][4] == "2147483904:1:U32_MAX", cid + " lists another extent key")
    require(re.findall(r"csum crc32c (\S+)", record) == ["0:" + checksum], cid + " lists another checksum")
    path = f"/mnt/{cid}/many/file-0242.txt"
    require(read(f"read/{cid}.mount.status") == "0\n", cid + " mount failed")
    require(read(f"read/{cid}.path.txt") == path + "\n", cid + " path is missing or ambiguous")
    require(read(f"read/{cid}.stat.txt") == f"2147483904 10 {path}\n", cid + " inode or size differs")
    if cid == "default-control":
        require(read(f"read/{cid}.read.status") == "0\n", "control read failed")
        require(read(f"read/{cid}.read.txt") == f"fd7023d87ea3679ca779d9e8d671ac04a58df4c91ce26b2774cd06623f1f6a9d  {path}\n", "control read has different bytes or binding")
    else:
        require(read(f"read/{cid}.read.status") == "1\n", "mutation did not return the recorded integrity failure")
        require(read(f"read/{cid}.read.txt") == f"sha256sum: {path}: Input/output error\n", "mutation read is not the recorded EIO")
print("default74-reference: fsck accepts both images; the pinned control reads its expected bytes and the same mutated extent fails with EIO")
PYGRADE
)

if [ "${1:-}" = --grade-read-evidence ]; then
    [ "$#" = 3 ] || fail "--grade-read-evidence needs an evidence directory and inode"
    grade_read_evidence "$2" "$3"
    exit "$?"
fi

PIN_DIGEST="${PIN_DIGEST:?PIN_DIGEST names the pinned fixtures artifact digest}"
# The verdicts the differential report recorded for default-74: the reference
# passes both the fixture and its mutation. Anything else is a change in the
# claim this case rests on, and fails the run.
EXPECT_CONTROL_STATUS="${EXPECT_CONTROL_STATUS:-0}"
EXPECT_MUTATED_STATUS="${EXPECT_MUTATED_STATUS:-0}"
REF_VERSION=1.39.7
fixtures="$here/.vm-share/fixtures"
out="$here/tmp/default74-reference"

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
tuple="$(observed_tuple "${OURS_LOG:-}")" || exit 1
read -r key stored computed <<<"$tuple"
[ -n "$key" ] && [ -n "$stored" ] && [ -n "$computed" ] || fail "the observed tuple '$tuple' is incomplete"
key_lines="default-74: extent_data key $key stored $stored computed $computed"
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
[ "$control_sha" = 70e75cfbe82c927f6914689a0142c0e37e99cae289819d86fbacf62f6ee78342 ] || fail "control physical image differs from the independent read"
[ "$mutated_sha" = 3b58375372070ca854c7600c54cca29d64a51921d68facea8eabaf4285e433eb ] || fail "mutated physical image differs from the independent read"
[ "$key $stored $computed" = "2147483904:1:U32_MAX 0x11ba7 0xa2f1ba7" ] || fail "the observed checksum tuple differs"


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
# A listed key is a header line whose fifth field is the key, exactly, followed by its indented
# value lines; a key whose text merely contains this one is a different key.
key_record() {
    awk -v k="$1" '/^[^ ]/ { inrec = ($1 == "u64s" && $5 == k) } inrec' "$2"
}
# The reference prints a crc32c as `csum crc32c <hi>:<lo>` in bare hex; ours prints 0x<hex>.
# Only a zero high half is the same integer, so anything else is kept verbatim and cannot match.
record_csum() {
    awk '{ for (i = 1; i < NF; i++) if ($i == "csum" && $(i + 1) == "crc32c") print $(i + 2) }' "$1" |
        awk -F: 'NF == 2 && $1 ~ /^0+$/ && $2 ~ /^[0-9a-f]+$/ { v = $2; sub(/^0+/, "", v); print "0x" (v == "" ? "0" : v); next } { print "unparsed:" $0 }'
}
computed="$(printf '%s\n' "$key_lines" | cut -d' ' -f8)"
key_record "$key" "$out/listing/default-control.extents.txt" >"$out/listing/default-control.key.txt"
key_record "$key" "$out/listing/default-74.extents.txt" >"$out/listing/default-74.key.txt"
key_record "${key}0" "$out/listing/default-74.extents.txt" >"$out/listing/absent.key.txt"

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
[ ! -s "$out/listing/absent.key.txt" ] ||
    fail "the record extraction matched key ${key}0, which is not $key"
for cid in default-control default-74; do
    [ "$(grep -c '^u64s ' "$out/listing/$cid.key.txt")" = 1 ] ||
        fail "the reference lists key $key other than once in $cid"
done
ref_stored="$(record_csum "$out/listing/default-74.key.txt")"
ref_control="$(record_csum "$out/listing/default-control.key.txt")"
echo "reference crc32c of $key: default-74 $ref_stored, default-control $ref_control" | tee -a "$out/evidence.txt"
[ "$ref_stored" = "$stored" ] ||
    fail "the reference lists $key in default-74 with checksum '$ref_stored', not the stored checksum $stored"
[ "$ref_control" = "$computed" ] ||
    fail "the reference lists $key in default-control with checksum '$ref_control', not our computed $computed"
[ "$ref_control" != "$stored" ] ||
    fail "the reference lists the stored checksum $stored in the unmutated control too"
echo "default74-reference: the reference lists $key with stored checksum $stored in image sha256:$mutated_sha"

# A listing is not a data read, and fsck acceptance is not read success. The
# reference reads the file that owns $key on the control and on the mutated
# image through its own FUSE mount, in a container with /dev/fuse, and the
# status and hash of each read are recorded. The control must read; the
# mutated image's outcome is the observation, kept for the verdict.
# A reference without a data reader fails, naming what is missing.
inum="${key%%:*}"
mkdir -p "$out/read"
docker exec "$ctr" bcachefs --help >"$out/read/bcachefs-help.txt" 2>&1
docker exec "$ctr" bcachefs fusemount --help >"$out/read/fusemount-help.txt" 2>&1
echo "$?" >"$out/read/fusemount-help.status"
grep -qi 'fusemount' "$out/read/bcachefs-help.txt" ||
    fail "the reference bcachefs-tools $REF_VERSION lists no fusemount command (read/bcachefs-help.txt); a data read needs a FUSE-enabled reference build"
[ -e /dev/fuse ] || fail "this runner has no /dev/fuse; the reference data read needs it"
docker commit "$ctr" "$ctr-image" >/dev/null || fail "the reference container was not committed for the read"
rctr="$ctr-read"
trap 'docker rm -f "$rctr" >/dev/null 2>&1 || true; docker rmi -f "$ctr-image" >/dev/null 2>&1 || true; cleanup' EXIT
docker run -d --name "$rctr" --memory 6g --device /dev/fuse --cap-add SYS_ADMIN \
    --security-opt apparmor:unconfined -v "$out:/work" "$ctr-image" sleep infinity >/dev/null ||
    fail "the reference read container did not start"
docker exec "$rctr" bash -euc 'apt-get install -y -qq --no-install-recommends fuse3 >/dev/null' ||
    fail "fuse3 did not install in the reference read container"
for cid in default-control default-74; do
    cp "$out/scratch/$cid.img" "$out/scratch/read-$cid.img" || fail "the $cid image was not copied for the read"
    [ "$(sha256sum "$out/scratch/read-$cid.img" | cut -d' ' -f1)" = "$(sha256sum "$out/scratch/$cid.img" | cut -d' ' -f1)" ] || fail "the read copy differs from the listed physical image"
    docker exec -e cid="$cid" -e inum="$inum" "$rctr" timeout -k 5 300 bash -uc '
        m="/mnt/$cid"; mkdir -p "$m"
        bcachefs fusemount "/work/scratch/read-$cid.img" "$m" >"/work/read/$cid.mount.txt" 2>&1
        echo "$?" >"/work/read/$cid.mount.status"
        for _ in $(seq 60); do mountpoint -q "$m" && break; sleep 1; done
        mountpoint -q "$m" || { echo "not mounted" >>"/work/read/$cid.mount.txt"; exit 0; }
        find "$m" -xdev -inum "$inum" >"/work/read/$cid.path.txt" 2>&1
        p="$(head -n 1 "/work/read/$cid.path.txt")"
        if [ -n "$p" ]; then
            stat -c "%i %s %n" "$p" >"/work/read/$cid.stat.txt" 2>&1
            timeout -k 5 120 sha256sum "$p" >"/work/read/$cid.read.txt" 2>&1
            echo "$?" >"/work/read/$cid.read.status"
        fi
        fusermount3 -u "$m" || umount "$m" || true
    '
    s=$?
    [ "$s" = 0 ] || fail "the reference read operation failed before grading (status $s)"
done
grade_read_evidence "$out" "$inum" || fail "the strict native read proof failed"
python3 -I scripts/differential-fuzz.py export-read-proof "$out" \
    "$out/scratch/default-control.img" "$out/scratch/default-74.img" || fail "the native read manifest was not exported"
python3 scripts/differential-read-classification-controls.py "$out" || fail "the bound-read classification controls failed"
