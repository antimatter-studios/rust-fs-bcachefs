#!/usr/bin/env bash
# Every job in every workflow takes its runner from a runner gate, and asks
# for the right class: x64, ARM64 or x64 with KVM (#707, CI-01). A call to a
# reusable workflow of this repository is expanded, so its jobs are covered
# through every caller; a call to another repository's must be named here. The
# classes are written out below, apart from the workflows, so a new job or a
# changed class fails until it is judged.
#
#   bash tests/scripts/runner-gate-covers-every-job.sh
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo/.github/workflows" <<'PY'
import copy
from pathlib import Path
import sys

import yaml

X64, ARM, KVM = "linux-x64", "linux-arm64", "linux-x64-kvm"
EXPECTED = {
    "ci.yml": {
        "guest-write-study": X64, "changes": X64, "unit": X64, "lint": X64,
        "fixtures": KVM, "oracle": X64, "test-arm64": ARM, "suite-in-vm": KVM,
        "cli": X64, "semver": X64, "coverage": X64, "ci-ok": X64,
    },
    "crc128-native-repro.yml": {"crc128-native": KVM},
    "differential.yml": {"default74-read-controls": X64, "pinned-default74": X64, "differential": X64},
    "fixtures-fresh.yml": {"compare": X64, "file": X64},
    "fixtures-keep.yml": {"keep": X64},
    "fuzz.yml": {"fuzz": X64},
    "key-fields-focused.yml": {"key-fields-focused": KVM},
    "release.yml": {"verify-tag": X64, "publish": X64},
    "s1-watch.yml": {"watch": X64},
    "branch-reap.yml": {},
}
# Calls into other repositories: their runners are chosen there.
EXTERNAL = {
    ("branch-reap.yml", "reap"): "antimatter-studios/rust-fs-core/.github/workflows/branch-reap.yml@",
    ("release.yml", "cli"): "antimatter-studios/rust-fs-core/.github/workflows/release-cli.yml@",
}
# The only jobs that may name a hosted runner for a gate that gave none.
FALLBACK = {("ci.yml", "ci-ok"): "ubuntu-latest", ("differential.yml", "differential"): "ubuntu-24.04"}
LOCAL = "./.github/workflows/"


def judge(workflows):
    errors, reached = [], set()
    if set(workflows) != set(EXPECTED):
        errors.append(f"workflows differ from the table: {sorted(set(workflows) ^ set(EXPECTED))}")

    def expand(file, seen):
        if file in seen:
            errors.append(f"{file}: reusable workflow calls form a cycle")
            return
        jobs = workflows[file]["jobs"]
        classes = EXPECTED.get(file, {})
        consumed, covered = set(), set()
        for name, job in jobs.items():
            where = f"{file}:{name}"
            if "uses" in job:
                uses = job["uses"]
                if uses.startswith(LOCAL):
                    target = uses[len(LOCAL):]
                    if target in workflows:
                        expand(target, seen | {file})
                    else:
                        errors.append(f"{where}: calls a workflow that is not here: {uses}")
                elif not uses.startswith(EXTERNAL.get((file, name), "\0")):
                    errors.append(f"{where}: unlisted external workflow call: {uses}")
                continue
            if job.get("runs-on") == "antimatter-vps-gate":
                if not name.endswith("-gate"):
                    errors.append(f"{where}: only a gate job runs on the gate itself")
                continue
            reached.add((file, name))
            covered.add(name)
            wanted = classes.get(name)
            if wanted is None:
                errors.append(f"{where}: no runner class is declared for this job")
                continue
            gate = f"{name}-gate"
            needs = job.get("needs", [])
            needs = [needs] if isinstance(needs, str) else needs
            if gate not in jobs or gate not in needs:
                errors.append(f"{where}: does not need its gate {gate}")
                continue
            consumed.add(gate)
            runner = f"needs.{gate}.outputs.runner"
            if (file, name) in FALLBACK:
                runner += f" || '{FALLBACK[file, name]}'"
            if job.get("runs-on") != "${{ " + runner + " }}":
                errors.append(f"{where}: runs on {job.get('runs-on')!r}, not its gate's pick")
            picker = jobs[gate]
            runs = [s.get("run", "").strip() for s in picker.get("steps", []) if s.get("id") == "pick"]
            if (picker.get("runs-on") != "antimatter-vps-gate"
                    or picker.get("outputs") != {"runner": "${{ steps.pick.outputs.runner }}"}
                    or len(picker.get("steps", [])) != 1
                    or runs != [f"runner-gate pick {wanted}"]):
                errors.append(f"{file}:{gate}: does not pick {wanted} through the runner gate")
        gates = {n for n, j in jobs.items() if j.get("runs-on") == "antimatter-vps-gate"}
        for gate in sorted(gates - consumed):
            errors.append(f"{file}:{gate}: no job takes this gate's pick")
        for name in sorted(set(classes) - covered):
            errors.append(f"{file}:{name}: declared here, but no such scheduled job")

    for file in sorted(workflows):
        expand(file, frozenset())
    return sorted(set(errors)), reached


directory = Path(sys.argv[1])
workflows = {p.name: yaml.safe_load(p.read_text()) for p in sorted(directory.glob("*.y*ml"))}
errors, reached = judge(workflows)
if errors:
    sys.exit("\n".join("FAIL  " + e for e in errors))
assert reached == {(f, j) for f, jobs in EXPECTED.items() for j in jobs}, reached


def mutated(change, wanted):
    copy_ = copy.deepcopy(workflows)
    change(copy_)
    found, _ = judge(copy_)
    assert any(wanted in e for e in found), (wanted, found)


def job(w, file, name):
    return w[file]["jobs"][name]


mutated(lambda w: job(w, "ci.yml", "unit").update({"runs-on": "ubuntu-24.04"}), "not its gate's pick")
mutated(lambda w: job(w, "ci.yml", "unit").update({"needs": ["changes"]}), "does not need its gate")
mutated(lambda w: job(w, "ci.yml", "fixtures-gate")["steps"][0].update({"run": "runner-gate pick linux-x64"}),
        "does not pick linux-x64-kvm")
mutated(lambda w: job(w, "crc128-native-repro.yml", "crc128-native").update({"runs-on": "ubuntu-24.04"}),
        "not its gate's pick")
mutated(lambda w: w["fuzz.yml"]["jobs"].update({"extra": {"runs-on": "ubuntu-24.04", "steps": []}}),
        "no runner class is declared")
mutated(lambda w: job(w, "ci.yml", "crc128-native").update({"uses": LOCAL + "gone.yml"}),
        "calls a workflow that is not here")
mutated(lambda w: job(w, "release.yml", "cli").update({"uses": "someone/else/.github/workflows/x.yml@main"}),
        "unlisted external workflow call")
mutated(lambda w: w.update({"new.yml": {"jobs": {}}}), "workflows differ from the table")
print(f"runner gate: {len(reached)} jobs ask for their declared class; 8 altered workflows are refused")
PY
