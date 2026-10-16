#!/usr/bin/env python3
"""Judge mode-specific CI results independently of static ci-gate exemptions."""

import json
import os
import sys

QUICK = {"changes", "unit", "lint", "semver"}
FULL = {
    "fixtures",
    "oracle",
    "test-arm64",
    "test-darwin",
    "suite-in-vm",
    "cli",
    "coverage",
}
FOCUSED = {"guest-write-study", "crc128-native"}
ALL = QUICK | FULL | FOCUSED


def verdict(needs, event, reproducer, code):
    if set(needs) != ALL:
        return ["dependency set differs from the declared CI contract"]
    if event == "workflow_dispatch" and reproducer != "full":
        selected = {
            "write-study-lifecycle": "guest-write-study",
            "crc128-native": "crc128-native",
        }.get(reproducer)
        if selected is None or code != "":
            return ["invalid isolated evidence selection or unexpected change output"]
        required = {selected}
    else:
        if code not in {"true", "false"}:
            return ["change classification must explicitly be true or false"]
        if event == "workflow_dispatch" and code != "true":
            return ["full dispatch must run every full tier"]
        required = QUICK | (FULL if code == "true" else set())
    errors = []
    for name in sorted(ALL):
        expected = "success" if name in required else "skipped"
        actual = needs[name].get("result")
        if actual != expected:
            errors.append(f"{name}: {actual!r}, required {expected}")
    return errors


def check_contract():
    # Independent expected-result tables: every altered result must fail,
    # including skips of quick jobs on documentation-only changes.
    cases = [
        ("pull_request", "", "false", QUICK),
        ("pull_request", "", "true", QUICK | FULL),
        ("merge_group", "", "true", QUICK | FULL),
        ("workflow_call", "", "true", QUICK | FULL),
        ("push", "", "true", QUICK | FULL),
        ("workflow_dispatch", "full", "true", QUICK | FULL),
        ("workflow_dispatch", "write-study-lifecycle", "", {"guest-write-study"}),
        ("workflow_dispatch", "crc128-native", "", {"crc128-native"}),
    ]
    checked = 0
    for event, reproducer, code, required in cases:
        needs = {
            name: {"result": "success" if name in required else "skipped"}
            for name in ALL
        }
        if verdict(needs, event, reproducer, code):
            raise AssertionError((event, reproducer, "valid path refused"))
        for name in ALL:
            for result in ("success", "failure", "cancelled", "skipped", "", "unknown"):
                if result == needs[name]["result"]:
                    continue
                changed = {**needs, name: {"result": result}}
                if not verdict(changed, event, reproducer, code):
                    raise AssertionError(
                        (event, reproducer, name, result, "false green")
                    )
                checked += 1
            missing = {key: value for key, value in needs.items() if key != name}
            if not verdict(missing, event, reproducer, code):
                raise AssertionError((name, "missing dependency accepted"))
        if not verdict(
            {**needs, "unjudged": {"result": "failure"}}, event, reproducer, code
        ):
            raise AssertionError("unjudged dependency accepted")
    for event, reproducer, code in [
        ("pull_request", "", ""),
        ("pull_request", "", "unknown"),
        ("workflow_dispatch", "full", "false"),
        ("workflow_dispatch", "unknown", ""),
        ("workflow_dispatch", "crc128-native", "true"),
    ]:
        if not verdict(needs, event, reproducer, code):
            raise AssertionError((event, reproducer, code, "invalid mode accepted"))
    print(f"ci-verdict: 8 valid paths and {checked} altered-result controls passed")


def main():
    if sys.argv[1:] == ["--check-contract"]:
        check_contract()
        return 0
    if sys.argv[1:]:
        raise ValueError("usage: ci-verdict.py [--check-contract]")
    needs = json.loads(os.environ["NEEDS"])
    for name, body in sorted(needs.items()):
        print(f"{name}: {body.get('result')}")
    errors = verdict(
        needs, os.environ["EVENT"], os.environ["REPRODUCER"], os.environ["CODE"]
    )
    for error in errors:
        print(f"::error::{error}")
    if not errors:
        print(
            "all applicable CI jobs succeeded; only explicitly inactive jobs were skipped"
        )
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
