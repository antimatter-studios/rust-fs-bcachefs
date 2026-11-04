#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo" <<'PY'
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

repo = Path(sys.argv[1])
tier = repo.parent / 'rust-fs-core/scripts/tier.sh'
shell = shutil.which('bash')
guards = (
    ('guest-write-study-cleans-up', 'STUDY', 'scratch', 'owned scratch survived'),
    ('guest-write-study-cleans-up', 'STUDY', 'session', 'owned child does not lead its own session'),
    ('guest-build-fixtures-cleans-up', 'FIXTURE', 'allocation', 'invocation must allocate one private root'),
    ('guest-build-fixtures-cleans-up', 'FIXTURE', 'session', 'owned child does not lead its own session'),
)
wrapper = r'''#!/usr/bin/env bash
status=0
bash "$DIAGNOSTIC_CONTROL_SUPERVISOR" "$@" || status=$?
"$DIAGNOSTIC_CONTROL_PYTHON" - "$status" <<'CONTROL'
import json
import os
from pathlib import Path
import sys

case = Path(os.environ[os.environ['DIAGNOSTIC_CONTROL_CASE_VAR']])
records = [json.loads(line) for line in (case / 'events').read_text().splitlines()]
mutation = os.environ['DIAGNOSTIC_CONTROL_MUTATION']
if mutation == 'scratch':
    scratch = next(event['scratch'] for event in records if event['kind'] == 'format')
    Path(scratch).mkdir()
elif mutation == 'allocation':
    records = [event for event in records if event['kind'] != 'allocation']
elif mutation == 'session':
    child = next(event for event in records if event['kind'] in ('format', 'reference'))
    child['pid'] = 0
else:
    raise AssertionError(mutation)
(case / 'events').write_text(''.join(json.dumps(event) + '\n' for event in records))
Path(os.environ['DIAGNOSTIC_CONTROL_WITNESS']).write_text(
    json.dumps(dict(case=str(case), status=int(sys.argv[1]))))
print(os.environ['DIAGNOSTIC_CONTROL_MARKER'], file=sys.stderr)
print('control log padding\n' * 100, end='', file=sys.stderr)
CONTROL
exit "$status"
'''
failures = []
with tempfile.TemporaryDirectory(prefix='guest-guard-log-control.') as tmp:
    sandbox = Path(tmp)
    copied = sandbox / 'repo'
    shutil.copytree(repo / 'scripts', copied / 'scripts')
    shutil.copytree(repo / 'tests/scripts/support', copied / 'tests/scripts/support')
    supervisor = sandbox / 'supervisor.sh'
    supervisor.write_text(wrapper)
    for guard in {row[0] for row in guards}:
        shutil.copy2(repo / f'tests/scripts/{guard}.sh', copied / f'tests/scripts/{guard}.sh')
    for guard, prefix, mutation, assertion in guards:
        label = f'{prefix.lower()}-{mutation}'
        witness = sandbox / f'{label}.json'
        marker = f'diagnostic control: {label} underlying supervisor error'
        tier_name = f'guest-guard-control-{label}'
        tier_log = repo / f'tmp/logs/{tier_name}.log'
        diagnostic = copied / f'tmp/logs/{guard}.log'
        if diagnostic.exists():
            diagnostic.rename(sandbox / f'{label}-previous.log')
        real = 'guest-write-study.sh' if prefix == 'STUDY' else 'guest-build-fixtures.sh'
        env = dict(os.environ, FS_CORE_CALLER=str(repo),
                   DIAGNOSTIC_CONTROL_SUPERVISOR=str(copied / 'scripts' / real),
                   DIAGNOSTIC_CONTROL_PYTHON=sys.executable,
                   DIAGNOSTIC_CONTROL_CASE_VAR=f'{prefix}_CASE',
                   DIAGNOSTIC_CONTROL_MUTATION=mutation,
                   DIAGNOSTIC_CONTROL_WITNESS=str(witness),
                   DIAGNOSTIC_CONTROL_MARKER=marker,
                   **{f'{prefix}_GUARD_SUPERVISOR': str(supervisor),
                      f'{prefix}_GUARD_CASES': 'format-fail'})
        result = subprocess.run(
            [shell, str(tier), 'diagnostic-control', tier_name, '80', '4900', '--',
             shell, str(copied / f'tests/scripts/{guard}.sh')],
            env=env, capture_output=True, text=True, timeout=960)
        try:
            assert result.returncode == 1, f'failed guard returned {result.returncode}, not 1'
            evidence = json.loads(witness.read_text())
            assert evidence['status'] == 31, 'control changed the supervisor exit status'
            case = Path(evidence['case'])
            assert not case.parent.exists(), 'the guard temporary directory survived cleanup'
            output = tier_log.read_text()
            assert assertion in output, f'control did not reach {assertion!r}; see {tier_log}'
            verdict = result.stdout + result.stderr
            assert len(verdict.splitlines()) == 1 and str(tier_log) in verdict, 'tier verdict changed'
            assert marker not in verdict, 'tier streamed the supervisor log'
            location = f'{guard}: diagnostics: '
            paths = [line.removeprefix(location) for line in output.splitlines()
                     if line.startswith(location)]
            assert paths == [str(diagnostic)], f'guard names no retained diagnostic log; see {tier_log}'
            assert diagnostic.is_file(), 'named diagnostic did not survive temporary-directory cleanup'
            assert marker in diagnostic.read_text(), 'retained log lost the underlying supervisor error'
            assert str(case / 'log') in diagnostic.read_text(), 'retained log lost its case identity'
            assert 'path: tmp/logs/*.log' in (repo / '.github/workflows/ci.yml').read_text(), 'CI does not upload the diagnostic'
        except AssertionError as error:
            failures.append(f'FAIL  {label}: {error}')
if failures:
    print('\n'.join(failures), file=sys.stderr)
    sys.exit(1)
print('PASS  guest guard diagnostics survive temporary cleanup')
PY
