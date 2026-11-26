#!/usr/bin/env bash
# A failing test binary does not hide the binaries after it (#606): every
# `cargo test` of the guest suite and of the oracle tier runs with
# --no-fail-fast, and the oracle tier judges its executed-case floor on a red
# run too, keeping the failing status.
#
#   bash tests/scripts/oracle-suite-reports-every-binary.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - "$REPO" <<'PY'
import pathlib
import re
import subprocess
import sys
import tempfile

import yaml

repo = pathlib.Path(sys.argv[1])
guest = (repo / "scripts/guest-suite.sh").read_text()
runs = re.findall(r"cargo test[^\n]*", guest)
assert len(runs) >= 10, f"FAIL  guest-suite.sh runs {len(runs)} cargo test commands"
for run in runs:
    assert "--no-fail-fast" in run, f"FAIL  guest-suite.sh stops at a failing binary: {run}"

tasks = yaml.safe_load((repo / "chores.yml").read_text())["tasks"]
commands = [c for c in tasks["test:oracle"]["cmds"] if "tier.sh" in c]
assert len(commands) == 1, "FAIL  test:oracle has one tier command"
command = commands[0]
assert "cargo test --no-fail-fast" in command, "FAIL  the oracle tier stops at a failing binary"

# The command as written, over stand-ins for the tier and the floor.
for tier, floor, want in ((0, 0, 0), (101, 0, 101), (101, 1, 101), (0, 1, 1)):
    with tempfile.TemporaryDirectory() as temporary:
        root = pathlib.Path(temporary)
        scripts = root / "rust-fs-core/scripts"
        scripts.mkdir(parents=True)
        (root / "project").mkdir()
        (scripts / "tier.sh").write_text(f"exit {tier}\n")
        (scripts / "test-floor.sh").write_text(f'echo "floor $*"\nexit {floor}\n')
        ran = subprocess.run(
            ["bash", "-c", command], cwd=root / "project", capture_output=True, text=True
        )
        assert ran.returncode == want, f"FAIL  tier {tier}, floor {floor}: {ran.returncode}"
        assert "floor --refuse-ignored oracle" in ran.stdout, f"FAIL  tier {tier}: no floor"

print("PASS  every oracle binary runs and is counted after an earlier one fails")
PY
