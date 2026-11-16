#!/usr/bin/env bash
# The focused key-fields route reads only durable proof and fixture assets (#430).
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - "$REPO/.github/workflows/key-fields-focused.yml" <<'PY' || exit 1
import sys
import yaml

workflow = yaml.safe_load(open(sys.argv[1], encoding="utf-8")) or {}
jobs = workflow.get("jobs") or {}
job = jobs.get("key-fields-focused") or {}
steps = job.get("steps") or []
commands = "\n".join(str(step.get("run", "")) for step in steps)

def require(ok, message):
    if not ok:
        raise SystemExit("FAIL  " + message)

require("differential-fixtures-1" in str(workflow.get("env", {})),
        "the focused workflow does not name the durable fixtures release")
require("actions/artifacts/" not in commands,
        "the focused workflow still fetches an expiring Actions artifact")
for expired in ("11977252185", "12011667570"):
    require(expired not in commands,
            "the focused workflow still names expired artifact " + expired)
require("scripts/kept-fixtures.sh fetch" in commands,
        "the fixtures are not fetched and digest-checked through kept-fixtures.sh")
require("default206-reference.zip" in commands,
        "the focused workflow does not fetch the durable physical-image read proof")
require("pinned-default74-read-controls.py" in commands,
        "the durable native read observations are not validated")
require("sha256sum --check" in commands,
        "the durable native read proof is not digest-checked")
fixture_digest = workflow.get("env", {}).get("KEPT_TARBALL", "")
require(len(fixture_digest) == 64 and all(c in "0123456789abcdef" for c in fixture_digest),
        "fixture tarball digest is not a complete SHA-256")
read_digest = next((step.get("env", {}).get("READ_DIGEST", "") for step in steps
                    if step.get("name") == "Fetch durable fixture and physical image read evidence"), "")
require(len(read_digest) == 64 and all(c in "0123456789abcdef" for c in read_digest),
        "the durable native read proof digest is not a complete SHA-256")
require("tests/default74_native_proof.py" not in commands,
        "the focused workflow still binds the proof to expired default-74 fixture bytes")
require("workflow_dispatch" in str(workflow.get("on", workflow.get(True, {}))),
        "the focused workflow lost its manual dispatch route")
print("key-fields-focused-uses-kept-assets: all checks passed")
PY
