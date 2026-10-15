#!/usr/bin/env bash
# differential-fuzz.sh [CASES [FIRST_SEED]] -- this crate and the reference
# checker on the same damaged images (#164). `chore fuzz:differential` and the
# scheduled workflow (.github/workflows/differential.yml) run it; it needs the
# fixtures (`chore fixtures`), docker and python3, on Linux.
#
# 1. EMIT. examples/differential.rs lays CASES of the whole-image gate's cases
#    (seeds FIRST_SEED on) over each of the gate's fixtures, with each fixture
#    unchanged as a control, and records this crate's verdict on each: open,
#    the bounded walk, the checker. It writes each case's changed pages.
# 2. JUDGE. A Debian 13 container installs the reference tools' packaged
#    binaries from apt.bcachefs.org, at the version every other oracle here
#    uses (nothing is built, no source is fetched).
#    scripts/differential-fuzz.py lays each case over a sparse copy of its
#    fixture and runs the reference checker, `fsck -n`, under a time limit.
#    An optional same-run independent native-read proof is strictly graded
#    and bound to both physical image digests and the exact extent finding.
# 3. CLASSIFY. Each case is both clean, both found something, found by the
#    reference alone, found by this crate alone, a crash or hang of the
#    reference, or a panic here. Each disagreement is keyed by its direction,
#    the kind of structure changed and what was found. A key not in
#    fuzz/differential-baseline.tsv fails the run, as do a panic here and a
#    control that is not clean to both.
#
# THE REPORT is tmp/differential/report.md: the counts, every disagreement
# class and an example of each. A case on which the reference crashed or hung
# is counted and nothing else, and its patch is deleted once judged
# (AGENTS.md, "A crash in the reference stays private"). Nothing else under
# tmp/differential is published.
#
# NOT A PULL-REQUEST GATE: what it finds depends on its seeds.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"
CASES="${1:-80}"
FIRST_SEED="${2:-0}"
# The reference tools' version: the same as scripts/vm-setup.sh's and
# scripts/kernel-oracle.sh's (tests/scripts/differential-fuzz-classifies.sh
# holds the three together).
REF_VERSION=1.39.7
JOBS="${DIFFERENTIAL_JOBS:-4}"
fixtures="${BCACHEFS_FIXTURES:-$here/.vm-share/fixtures}"
out="$here/tmp/differential"

missing=
[ -d "$fixtures" ] || missing="$missing the fixtures ($fixtures: run 'chore fixtures')"
for tool in docker python3 timeout; do
    command -v "$tool" >/dev/null || missing="$missing $tool"
done
if [ -n "$missing" ]; then
    echo "differential: this host lacks$missing" >&2
    exit 1
fi

rm -rf "$out"
mkdir -p "$out"

echo "differential: emitting $CASES cases per fixture from seed $FIRST_SEED"
cargo build --locked --release --quiet --features write --example differential
timeout 1800 target/release/examples/differential "$fixtures" "$out" "$CASES" "$FIRST_SEED" || {
    status=$?
    echo "differential: the emitter failed (status $status) on $(cat "$out/inflight.txt" 2>/dev/null || echo 'no case')" >&2
    exit 1
}

ctr="rust-fs-bcachefs-differential-$$"
cleanup() {
    docker rm -f "$ctr" >/dev/null 2>&1 || true
    rm -rf "$out/scratch"
}
trap cleanup EXIT
echo "differential: installing the reference tools $REF_VERSION in a Debian 13 container"
docker run -d --name "$ctr" --memory 6g -v "$out:/work" debian:trixie sleep infinity >/dev/null
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
' >"$out/reference-install.log" 2>&1 || {
    status=$?
    echo "differential: the reference tools did not install (status $status); see $out/reference-install.log" >&2
    exit 1
}

# DIFFERENTIAL_KEEP names cases (space-separated) whose patch, mutation and
# reference output are kept under $out/kept when judged without a crash.
keep=()
for id in ${DIFFERENTIAL_KEEP:-}; do keep+=(--keep "$id"); done
# DIFFERENTIAL_LISTINGS names btrees (space-separated) the reference lists for
# each kept case, on the fixture unchanged and on the mutated image (#230).
for b in ${DIFFERENTIAL_LISTINGS:-}; do keep+=(--listing "$b"); done
read_evidence=()
if [ -n "${DIFFERENTIAL_READ_EVIDENCE:-}" ]; then
    read_evidence=(--read-evidence "$DIFFERENTIAL_READ_EVIDENCE")
fi
python3 scripts/differential-fuzz.py judge "$out" "$fixtures" --container "$ctr" --jobs "$JOBS" --timeout 60 --version "$REF_VERSION" ${read_evidence[@]+"${read_evidence[@]}"} ${keep[@]+"${keep[@]}"}
python3 scripts/differential-fuzz.py classify "$out" --baseline fuzz/differential-baseline.tsv \
    --report "$out/report.md" --version "$REF_VERSION" ${read_evidence[@]+"${read_evidence[@]}"}
