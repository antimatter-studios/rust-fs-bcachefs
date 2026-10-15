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
