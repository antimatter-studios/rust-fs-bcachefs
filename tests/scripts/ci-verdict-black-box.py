#!/usr/bin/env python3
"""Exercise the shipped verdict CLI with independently specified job records."""

import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
COMMAND = [sys.executable, str(REPO / "scripts/ci-verdict.py")]
ACCEPTED = (
    "all applicable CI jobs succeeded; only explicitly inactive jobs were skipped"
)

# These are supplied completed-job records, not imports of the verdict's job
# sets or calls to its self-check. Inactive jobs must really be skipped.
RECORDS = {
    "quick": {
        "changes": "success",
        "unit": "success",
        "lint": "success",
        "semver": "success",
        "fixtures": "skipped",
        "oracle": "skipped",
        "test-arm64": "skipped",
        "test-darwin": "skipped",
        "suite-in-vm": "skipped",
        "cli": "skipped",
        "coverage": "skipped",
        "guest-write-study": "skipped",
        "crc128-native": "skipped",
    },
    "full": {
        "changes": "success",
        "unit": "success",
        "lint": "success",
        "semver": "success",
        "fixtures": "success",
        "oracle": "success",
        "test-arm64": "success",
        "test-darwin": "success",
        "suite-in-vm": "success",
        "cli": "success",
        "coverage": "success",
        "guest-write-study": "skipped",
        "crc128-native": "skipped",
    },
    "lifecycle": {
        "changes": "skipped",
        "unit": "skipped",
        "lint": "skipped",
        "semver": "skipped",
        "fixtures": "skipped",
        "oracle": "skipped",
        "test-arm64": "skipped",
        "test-darwin": "skipped",
        "suite-in-vm": "skipped",
        "cli": "skipped",
        "coverage": "skipped",
        "guest-write-study": "success",
        "crc128-native": "skipped",
    },
    "native": {
        "changes": "skipped",
        "unit": "skipped",
        "lint": "skipped",
        "semver": "skipped",
        "fixtures": "skipped",
        "oracle": "skipped",
        "test-arm64": "skipped",
        "test-darwin": "skipped",
        "suite-in-vm": "skipped",
        "cli": "skipped",
        "coverage": "skipped",
        "guest-write-study": "skipped",
        "crc128-native": "success",
    },
}

# Event and planner outputs are supplied as CI does, through the process
# environment. A reusable release invocation uses the full profile.
PROFILES = [
    ("quick", "pull_request", "", "false"),
    ("quick", "push", "", "false"),
    ("full", "merge_group", "", "true"),
    ("full", "push", "", "true"),
    ("full", "workflow_call", "full", "true"),
    ("full", "workflow_dispatch", "full", "true"),
    ("lifecycle", "workflow_dispatch", "write-study-lifecycle", ""),
    ("native", "workflow_dispatch", "crc128-native", ""),
]


def environment(profile):
    record, event, reproducer, code = profile
    needs = {name: {"result": result} for name, result in RECORDS[record].items()}
    # GitHub also supplies outputs inside needs, but CODE is the workflow's
    # explicit projection of the planner output into the verdict environment.
    needs["changes"]["outputs"] = {"code": code} if code else {}
    env = dict(os.environ)
    for key in ("NEEDS", "EVENT", "REPRODUCER", "CODE"):
        env.pop(key, None)
    env.update(NEEDS=json.dumps(needs), EVENT=event, REPRODUCER=reproducer, CODE=code)
    return env


def grade(result, accepted, diagnostic=""):
    output = result.stdout + result.stderr
    expected_status = 0 if accepted else 1
    if result.returncode != expected_status:
        raise AssertionError(
            ("exit status", expected_status, result.returncode, output)
        )
    if (ACCEPTED in result.stdout) != accepted:
        raise AssertionError(("success announcement", accepted, output))
    if diagnostic and diagnostic not in output:
        raise AssertionError(("missing rejection reason", diagnostic, output))


def run_case(label, env, accepted, diagnostic=""):
    result = subprocess.run(
        COMMAND,
        env=env,
        cwd=REPO,
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    try:
        grade(result, accepted, diagnostic)
    except AssertionError as error:
        raise AssertionError((label, error.args)) from error


def main():
    # Calibrate the grading independently: an always-green command cannot
    # pass a negative case, and an always-red command cannot pass a positive.
    for status, output, wanted in [(0, ACCEPTED, False), (1, "::error::refused", True)]:
        canary = subprocess.CompletedProcess([], status, output, "")
        try:
            grade(canary, wanted)
        except AssertionError:
            pass
        else:
            raise AssertionError("constant verdict escaped the grading controls")

    checked = 0
    for profile in PROFILES:
        base = environment(profile)
        run_case((profile, "all applicable jobs succeeded"), base, True)
        checked += 1
        needs = json.loads(base["NEEDS"])
        for name, body in needs.items():
            # Every required leg is individually failed, cancelled, skipped,
            # missing and result-less. Every inactive leg also rejects failure,
            # cancellation or an unexpected execution instead of its legal skip.
            for state in ("success", "failure", "cancelled", "skipped", "unknown", ""):
                if state == body["result"]:
                    continue
                bad = dict(needs, **{name: {"result": state}})
                run_case(
                    (profile, name, state),
                    dict(base, NEEDS=json.dumps(bad)),
                    False,
                    f"::error::{name}:",
                )
                checked += 1
            missing = {key: value for key, value in needs.items() if key != name}
            run_case(
                (profile, name, "missing job"),
                dict(base, NEEDS=json.dumps(missing)),
                False,
                "dependency set differs",
            )
            no_result = dict(needs, **{name: {"outputs": {}}})
            run_case(
                (profile, name, "missing result"),
                dict(base, NEEDS=json.dumps(no_result)),
                False,
                f"::error::{name}:",
            )
            checked += 2
        extra = dict(needs, unjudged={"result": "failure"})
        run_case(
            (profile, "unjudged failed job"),
            dict(base, NEEDS=json.dumps(extra)),
            False,
            "dependency set differs",
        )
        checked += 1
        for key in ("NEEDS", "EVENT", "REPRODUCER", "CODE"):
            missing_env = dict(base)
            del missing_env[key]
            run_case((profile, "missing environment", key), missing_env, False, key)
            checked += 1

    for record, event, reproducer, code in [
        ("quick", "pull_request", "", ""),
        ("full", "merge_group", "", "invalid"),
        ("full", "workflow_dispatch", "full", "false"),
        ("full", "workflow_dispatch", "", "true"),
        ("native", "workflow_dispatch", "unknown", ""),
        ("native", "workflow_dispatch", "crc128-native", "true"),
        ("lifecycle", "workflow_dispatch", "write-study-lifecycle", "false"),
    ]:
        profile = (record, event, reproducer, code)
        run_case(
            (profile, "malformed profile"), environment(profile), False, "::error::"
        )
        checked += 1
    for malformed in ("{", "null", "[]"):
        run_case(
            ("malformed needs", malformed),
            dict(environment(PROFILES[0]), NEEDS=malformed),
            False,
        )
        checked += 1
    print(
        f"ci-verdict-black-box: {checked} subprocess cases and 2 grading controls passed"
    )


if __name__ == "__main__":
    main()
