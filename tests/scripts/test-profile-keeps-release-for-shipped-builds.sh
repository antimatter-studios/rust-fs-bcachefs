#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo" <<'PY'
from pathlib import Path
import re
import sys

repo = Path(sys.argv[1])
manifest = (repo / 'Cargo.toml').read_text()
release = re.search(r'(?ms)^\[profile\.release\]\n(.*?)(?=^\[|\Z)', manifest)
test = re.search(r'(?ms)^\[profile\.test\]\n(.*?)(?=^\[|\Z)', manifest)
assert release, 'the shipped release profile is missing'
assert re.search(r'(?m)^lto\s*=\s*true\s*$', release.group(1)), \
    'shipped release builds must keep fat LTO'
assert re.search(r'(?m)^codegen-units\s*=\s*1\s*$', release.group(1)), \
    'shipped release builds must keep one codegen unit'
assert test, 'test commands need a dedicated test profile'
assert re.search(r'(?m)^opt-level\s*=\s*1\s*$', test.group(1)), \
    'the test profile keeps measured optimization without release LTO'
assert not re.search(r'(?m)^lto\s*=\s*true\s*$', test.group(1)), \
    'test binaries must not use fat LTO'
assert not re.search(r'(?m)^codegen-units\s*=\s*1\s*$', test.group(1)), \
    'test binaries must keep parallel code generation'

paths = [repo / 'chores.yml', *(repo / 'scripts').glob('*.sh')]
paths += list((repo / '.github/workflows').glob('*.yml'))
release_flag = '-' * 2 + 'release'
violations = []
for path in paths:
    source = re.sub(r'\\\s*\n', ' ', path.read_text())
    for number, line in enumerate(source.splitlines(), 1):
        command = line.split('#', 1)[0]
        if 'cargo test' in command and release_flag in command:
            violations.append(f'{path.relative_to(repo)}:{number}: {line.strip()}')
assert not violations, 'test command selects the shipped release profile:\n' + '\n'.join(violations)
print('test-profile-keeps-release-for-shipped-builds: all checks passed')
PY
