#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo/scripts/guest-write-study.sh" <<'PY'
from pathlib import Path
import re
import subprocess
import sys

source = Path(sys.argv[1]).read_text()
for function, reason in [('stop_fuse', 'owned FUSE process 123 did not stop'),
                         ('cleanup', 'owned tool 123 did not stop; retained')]:
    body = re.search(rf'^{function}\(\) \{{\n.*?^\}}', source, re.M | re.S)
    assert body, f'{function}: function missing'
    control = '''
set -u
scratch='' owner='' owned_watchdog='' mnt=''
fuse_pid='' tool_pid=''
kill() { return 0; }
sleep() { :; }
wait() { echo 'blocking wait reached' >&2; return 0; }
mountpoint() { echo 'unmount reached before termination' >&2; return 1; }
'''
    control += body.group() + '\n'
    control += f'{"fuse_pid" if function == "stop_fuse" else "tool_pid"}=123\n'
    control += function
    result = subprocess.run(['bash', '-c', control], check=False, capture_output=True,
                            text=True, timeout=10)
    assert result.returncode == 1, (function, result.returncode, result.stderr)
    assert reason in result.stderr, (function, result.stderr)
    assert 'blocking wait reached' not in result.stderr, result.stderr
    assert 'unmount reached before termination' not in result.stderr, result.stderr
print('PASS  write-study refuses a tool or FUSE process that survives its bounded termination poll')
PY
