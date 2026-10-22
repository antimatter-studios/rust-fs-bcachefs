#!/usr/bin/env bash
# Replay #265 with the differential harness's emitter and black-box judge.
# Only this issue's class is gated here; the normal differential job still
# classifies every disagreement against the unchanged baseline.
set -euo pipefail
cd "$(dirname "$0")/.."
pin=sha256:9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e
[ "$(cat .vm-share/fixtures-pin)" = "$pin" ] || {
    echo "snapshot-tree-subvolume: fetch the pinned fixtures in differential.yml" >&2
    exit 1
}
out="$PWD/tmp/snapshot-tree-subvolume"
fixtures="$PWD/.vm-share/fixtures"
mkdir -p "$out"
# Refuse stale output rather than accidentally grading a previous run.
[ ! -e "$out/cases.tsv" ] || { echo "snapshot-tree-subvolume: output already exists" >&2; exit 1; }
cargo build --locked --release --quiet --features write --example differential
timeout 1800 target/release/examples/differential "$fixtures" "$out" 80 1659440

ctr="rust-fs-bcachefs-snapshot-tree-subvolume-$$"
cleanup() { docker rm -f "$ctr" >/dev/null 2>&1 || true; }
trap cleanup EXIT
docker run -d --name "$ctr" --memory 6g -v "$out:/work" debian:trixie sleep infinity >/dev/null
docker exec "$ctr" bash -euc '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends ca-certificates curl >/dev/null
    install -d -m 0755 /etc/apt/keyrings
    curl -fsSL -o /etc/apt/keyrings/apt.bcachefs.org.asc https://apt.bcachefs.org/apt.bcachefs.org.asc
    echo "deb [signed-by=/etc/apt/keyrings/apt.bcachefs.org.asc] https://apt.bcachefs.org/trixie bcachefs-tools-release main" >/etc/apt/sources.list.d/apt.bcachefs.org.list
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends bcachefs-tools=1:1.39.7 >/dev/null
    bcachefs version
' >"$out/reference-install.log" 2>&1
python3 scripts/differential-fuzz.py judge "$out" "$fixtures" --container "$ctr" \
    --jobs 4 --timeout 60 --version 1.39.7 --keep kernel-control --keep kernel-1659511

python3 - "$out" <<'PY'
import importlib.util
from pathlib import Path
import sys

spec = importlib.util.spec_from_file_location("differential", "scripts/differential-fuzz.py")
differential = importlib.util.module_from_spec(spec)
sys.path.insert(0, "scripts")
spec.loader.exec_module(differential)
out = sys.argv[1]

def require(condition, message):
    if not condition:
        raise SystemExit("snapshot-tree-subvolume: " + message)

cases = differential.read_cases(out)
fixtures = {
    "default.img", "block4k.img", "lz4.img", "aged.img", "aged-unclean.img",
    "kernel.img", "kernel-rootsnap.img", "kernel-written.img",
}
expected = {(name, "-") for name in fixtures}
expected |= {(name, str(seed)) for name in fixtures for seed in range(1659440, 1659520)}
require(len(cases) == 648, "expected 648 executed cases, including eight controls")
require({(c["fixture"], c["seed"]) for c in cases} == expected, "the replay's fixtures or seeds changed")
normal = differential.controls(out, cases)
needle = "snapshot tree points to subvolume that does not point to snapshot in this tree"
observed = False
crashes = 0
for case in cases:
    require(case["ours"] in ("clean", "found"), "the crate panicked or returned an invalid verdict")
    ref = differential.judged(out, case, normal.get(case["fixture"]))
    require(ref is not None, "a case was never judged")
    if ref["verdict"] == "crashed":
        crashes += 1  # The existing judge discards the identity and output.
        require(case["structure"] != "control", "a reference control crashed")
        continue
    if case["structure"] == "control":
        require(case["ours"] == "clean" and ref["verdict"] == "clean" and ref["status"] == 0,
                "an unchanged fixture is not clean to both checkers")
    matches = any(needle in line for line in ref["lines"])
    require(not matches or case["ours"] == "found", "the unresolved #265 class remains")
    if case["id"] == "kernel-1659511":
        require(case["what"] == "btree 8 key at 0:2:0: offset changed, insert committed",
                "kernel-1659511 no longer has the recorded mutation")
        require(case["signature"] == "check:snapshot_tree_to_wrong_subvol", "the exact corruption was not named")
        require(matches and ref["status"] == 1, "the reference did not reproduce the recorded rejection")
        observed = True
require(observed, "the exact non-crashing reference rejection was not observed")
message = f"snapshot-tree-subvolume: 648 cases, eight clean controls; #265 resolved; {crashes} reference crashes (counts only)"
Path(out, "evidence.txt").write_text(message + "\n")
print(message)
PY
