#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo/tests/scripts/support" <<'PY'
from pathlib import Path
import sys
import tempfile
from unittest.mock import Mock, patch

sys.path.insert(0, sys.argv[1])
from supervisor_guard import SupervisorProgress

with tempfile.TemporaryDirectory(prefix='supervisor-progress-test.') as directory:
    sandbox = Path(directory)
    case = sandbox / 'stuck-control'
    case.mkdir()
    log = case / 'log'
    log.touch()
    progress = SupervisorProgress(sandbox, 3, 9)
    clock = [0.0]

    def advance(seconds):
        clock[0] += 1

    def expect_failure(what, done, *messages):
        clock[0] = 0
        with patch('supervisor_guard.time.monotonic', side_effect=lambda: clock[0]), \
             patch('supervisor_guard.time.sleep', side_effect=advance):
            try:
                progress.wait_for(case, what, done)
            except AssertionError as error:
                for message in ('stuck-control', what, *messages):
                    assert message in str(error), (message, str(error))
            else:
                raise AssertionError('a stuck supervisor passed')

    expect_failure('the ready file', lambda: False, 'no progress for')
    progress.idle_limit, progress.case_limit = 60, 3

    def still_tracing():
        with log.open('a') as output:
            output.write('another command\n')
        return False

    expect_failure('the supervisor to exit', still_tracing, 'still running commands')
    proc = Mock(returncode=42)
    proc.poll.return_value = 42
    try:
        progress.await_ready(proc, case)
    except AssertionError as error:
        assert 'stuck-control' in str(error) and 'status 42' in str(error), str(error)
    else:
        raise AssertionError('an early supervisor exit passed')
    (case / 'ready').touch()
    progress.await_ready(proc, case)
print('PASS  supervisor progress waits name idle, continuously tracing and early-exit failures')
PY
