#!/usr/bin/env bash
# ci.yml's event profile and its ci-ok aggregate, exercised from the
# workflow's own text: the planner step and the aggregate step are extracted
# and run under every event, so no VM and no fixture is needed. Pull requests
# and pushes to main are quick; the merge queue, manual dispatches, tags and
# anything unexpected run the full pipeline. ci-ok must accept only the
# declared full jobs' skips, and only on a quick run.
#
#   bash tests/scripts/ci-depth.sh
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo/.github/workflows/ci.yml" <<'PY'
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

import yaml

doc = yaml.safe_load(Path(sys.argv[1]).read_text())
events = doc.get("on", doc.get(True))
assert {"pull_request", "merge_group", "workflow_dispatch", "workflow_call"} <= set(events), events
assert not (events.get("pull_request") or {}).get("branches"), "all stack bases need PR checks"
jobs = doc["jobs"]
planner = next(s for s in jobs["changes"]["steps"] if s.get("id") == "c")["run"]
aggregate = next(s for s in jobs["ci-ok"]["steps"] if "NEEDS" in s.get("env", {}))
full = set(json.loads(aggregate["env"]["FULL_JOBS"]))
assert full <= set(jobs), "the aggregate must not exempt nonexistent jobs"
assert set(jobs["ci-ok"]["needs"]) == set(jobs) - {"ci-ok"}

# Derive every intentionally skipped job from the dependency graph. A new
# full job must be declared, and a quick job must never receive an exemption.
expected = {n for n, j in jobs.items()
            if j.get("if") == "needs.changes.outputs.code == 'true'"}
while True:
    expanded = set(expected)
    for name, job in jobs.items():
        needs = job.get("needs", [])
        needs = [needs] if isinstance(needs, str) else needs
        if name != "ci-ok" and set(needs) & expected:
            expanded.add(name)
    if expanded == expected:
        break
    expected = expanded
assert full == expected, (full, expected)
assert {"unit", "lint", "semver"}.isdisjoint(full), "quick jobs are never exempt"
focused = {"guest-write-study", "crc128-native"}
assert focused <= set(jobs)
assert events["workflow_call"]["inputs"]["reproducer"]["default"] == "full"

for event, ref, decision in [
    ("pull_request", "refs/pull/1/merge", "false"),
    ("merge_group", "refs/heads/gh-readonly-queue/main/pr-1", "true"),
    ("workflow_dispatch", "refs/heads/ci/oracle-red", "true"),
    ("push", "refs/heads/main", "false"),
    ("push", "refs/tags/v1.0.0", "true"),
    ("unexpected", "", "true"),
]:
    with tempfile.TemporaryDirectory() as tmp:
        output = Path(tmp) / "output"
        env = dict(os.environ, EVENT=event, REF=ref, GITHUB_OUTPUT=str(output))
        result = subprocess.run(["bash", "-c", planner], env=env, capture_output=True, text=True)
        assert result.returncode == 0, result.stderr
        assert output.read_text().splitlines() == ["code=" + decision], (event, result.stdout)

def verdict(code, results, event=None, reproducer=None):
    event = event or ("workflow_dispatch" if code == "true" else "pull_request")
    if reproducer is None:
        reproducer = "full" if event == "workflow_dispatch" else ""
    env = dict(os.environ, CODE=code, NEEDS=json.dumps(results), FULL_JOBS=json.dumps(sorted(full)),
               EVENT=event, REPRODUCER=reproducer)
    return subprocess.run(["bash", "-c", aggregate["run"]], env=env,
                          cwd=Path(sys.argv[1]).resolve().parents[2], capture_output=True, text=True)

success = {n: {"result": "skipped" if n in focused else "success"} for n in jobs["ci-ok"]["needs"]}
quick = {n: {"result": "skipped" if n in full | focused else "success"} for n in success}
for code, results in [("true", success), ("false", quick)]:
    result = verdict(code, results)
    assert result.returncode == 0, result.stdout + result.stderr

for code in ["", "invalid"]:
    assert verdict(code, success).returncode != 0, "a missing decision must fail closed"
assert verdict("true", quick).returncode != 0, "a full run with skipped full jobs must fail"
for name in success.keys() - focused:
    for state in ["failure", "cancelled", "skipped"]:
        bad = dict(success)
        bad[name] = {"result": state}
        assert verdict("true", bad).returncode != 0, ("full", name, state)
        if name not in full or state != "skipped":
            bad = dict(quick)
            bad[name] = {"result": state}
            assert verdict("false", bad).returncode != 0, ("quick", name, state)
for selected, reproducer in [("guest-write-study", "write-study-lifecycle"),
                             ("crc128-native", "crc128-native")]:
    isolated = {n: {"result": "success" if n == selected else "skipped"} for n in success}
    assert verdict("", isolated, "workflow_dispatch", reproducer).returncode == 0
    for name in isolated:
        for state in ["success", "failure", "cancelled", "skipped"]:
            if state == isolated[name]["result"]:
                continue
            bad = dict(isolated)
            bad[name] = {"result": state}
            assert verdict("", bad, "workflow_dispatch", reproducer).returncode != 0, (selected, name, state)
for results, code in [(success, "true"), (quick, "false")]:
    for name in focused:
        for state in ["success", "failure", "cancelled"]:
            bad = dict(results)
            bad[name] = {"result": state}
            assert verdict(code, bad).returncode != 0, (code, name, state)
print("ci-depth: all checks passed")
PY
