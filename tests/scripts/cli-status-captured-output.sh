#!/usr/bin/env bash
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo/tests/scripts/cli-status-checks-read-the-verb.sh" <<'PY'
from pathlib import Path
import shlex
import subprocess
import sys

commands = [line.strip()[:-3] for line in Path(sys.argv[1]).read_text().splitlines()
            if '"FAIL  test-write: $mode: $what ("' in line
            and line.strip().endswith((' ||', ' &&'))]
assert len(commands) == 2, "both captured-output checks must be exercised"
marker = "FAIL  test-write: in-place: put from stdin (stand-in)\n"
padding = "unrelated output\n" * 131072
failures = []
for number, command in enumerate(commands, 1):
    for label, output, expected in [
        ("early match", marker + padding, 0),
        ("late match", padding + marker, 0),
        ("absent text", padding, 1),
    ]:
        script = ('set -o pipefail\nmode=in-place\nwhat="put from stdin"\n'
                  'failing=' + shlex.quote(output) + '\npassing="$failing"\n'
                  'check_output() {\n' + command + '\n}\n'
                  'for attempt in {1..8}; do\ncheck_output\n'
                  'status=$?\n[ "$status" -eq ' + str(expected) + ' ] || '
                  '{ echo "expected ' + str(expected) + ', got $status" >&2; exit 1; }\ndone\n')
        result = subprocess.run(["bash"], input=script, text=True, capture_output=True)
        if result.returncode:
            failures.append(f"check {number}, {label}: {result.stderr.strip()}")
for failure in failures:
    print("FAIL  captured-output: " + failure, file=sys.stderr)
sys.exit(bool(failures))
PY
status=$?
[ "$status" -eq 0 ] || exit "$status"
echo "PASS  both captured-output checks accept large matches and reject absent text repeatedly"
