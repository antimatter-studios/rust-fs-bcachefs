#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo" <<'PY'
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

import yaml

repo = Path(sys.argv[1])
jobs = yaml.safe_load((repo / '.github/workflows/ci.yml').read_text())['jobs']
assert any(step.get('run') == 'chore test:scripts' for step in jobs['unit']['steps']), \
    'unit must run the scripts tier'
assert 'unit' in jobs['ci-ok']['needs'], 'the required aggregate must depend on unit'
wrapper = Path('tests/scripts/select-fixture-run.sh')
assert (repo / wrapper).is_file(), \
    'selector controls must run in the scripts tier: tests/scripts/select-fixture-run.sh is missing'
commands = yaml.safe_load((repo / 'chores.yml').read_text())['tasks']['test:scripts']['cmds']
with tempfile.TemporaryDirectory(prefix='fixture-selector-required.') as directory:
    sandbox = Path(directory)
    workspace = sandbox / 'repo'
    (workspace / 'scripts').mkdir(parents=True)
    (workspace / 'tests/scripts').mkdir(parents=True)
    (sandbox / 'rust-fs-core').symlink_to(repo.parent / 'rust-fs-core', target_is_directory=True)
    shutil.copy2(repo / wrapper, workspace / wrapper)
    shutil.copy2(repo / 'scripts/select-fixture-run-test.sh',
                 workspace / 'scripts/select-fixture-run-test.sh')
    (workspace / 'scripts/select-fixture-run.sh').write_text('#!/usr/bin/env bash\nexit 0\n')
    result = subprocess.run(['/bin/bash', '-e', '-c', '\n'.join(commands)], cwd=workspace,
                            env=dict(os.environ, FS_CORE_CALLER=str(workspace)),
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 1, ('the scripts tier must reject a broken selector',
                                    result.returncode, result.stdout, result.stderr)
    log = (workspace / 'tmp/logs/scripts.log').read_text()
    assert 'FAIL: quick-then-full' in log, log
    assert 'FAIL  tests/scripts/select-fixture-run.sh' in log, log
print('fixture-selector-is-required.sh: all checks passed')
PY
