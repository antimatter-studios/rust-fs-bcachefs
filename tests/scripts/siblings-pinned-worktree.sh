#!/usr/bin/env bash
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
taskfile="${SIBLINGS_GUARD_TASKFILE:-$repo/chores.yml}"
sandbox="$(mktemp -d)"
trap 'rm -r -- "$sandbox"' EXIT

python3 - "$taskfile" "$sandbox" <<'PY' || exit 1
import os
from pathlib import Path
import subprocess
import sys

import yaml

source = yaml.safe_load(Path(sys.argv[1]).read_text())
sandbox = Path(sys.argv[2])
checkout = sandbox / "rust-fs-bcachefs"
checkout.mkdir()
for name in ("rust-fs-bcachefs", "rust-fs-core", "fs-linux-test-harness"):
    (sandbox / name / ".git").mkdir(parents=True)
taskfile = {"version": source["version"], "vars": source["vars"],
            "tasks": {"siblings": source["tasks"]["siblings"]}}
(checkout / "chores.yml").write_text(yaml.safe_dump(taskfile))
tools = sandbox / "bin"
tools.mkdir()
stub = tools / "git"
stub.write_text('''#!/usr/bin/env bash
set -eu
printf '%s\\n' "$*" >> "$SIBLINGS_GIT_TRACE"
if [ "$1" = rev-parse ]; then
    case "$2" in
        --show-toplevel) printf '%s\\n' "$SIBLINGS_CHECKOUT" ;;
        --path-format=absolute) printf '%s/.git\\n' "$SIBLINGS_CHECKOUT" ;;
        *) exit 2 ;;
    esac
    exit 0
fi
[ "$1" = -C ] || exit 2
name="${2##*/}"
shift 2
case "$1" in
    fetch) exit 0 ;;
    merge-base)
        [ "$2" = --is-ancestor ] || exit 2
        if [ "$name" != "$SIBLINGS_TEST_NAME" ]; then exit 0; fi
        if [ "$3" = HEAD ]; then
            [ "$SIBLINGS_TEST_STATE" = pinned ]
        else
            [ "$SIBLINGS_TEST_STATE" != below ]
        fi
        ;;
    *) printf 'unexpected git command: %s\\n' "$*" >&2; exit 2 ;;
esac
''')
stub.chmod(0o755)
environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"],
                   SIBLINGS_CHECKOUT=str(checkout),
                   SIBLINGS_GIT_TRACE=str(sandbox / "git.trace"))
problems = []
for name, variable in (("rust-fs-core", "FS_CORE_REF"),
                       ("fs-linux-test-harness", "LINUX_HARNESS_REF")):
    reference = source["vars"][variable]
    for state in ("pinned", "ahead", "below"):
        result = subprocess.run(
            ["chore", "--dir", str(checkout), "siblings"],
            env=dict(environment, SIBLINGS_TEST_NAME=name, SIBLINGS_TEST_STATE=state),
            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        output = result.stdout
        if state == "below":
            if result.returncode == 0 or f"{name} is below {reference}" not in output:
                problems.append(f"{name}: a below-pin sibling must still fail")
        elif result.returncode != 0:
            problems.append(f"{name} {state}: chore siblings failed: {output}")
        elif state == "ahead":
            for expected in ("detached worktree", reference, "AGENTS.md",
                             f"rust-fs-bcachefs-<id>/{name}"):
                if expected not in output:
                    problems.append(f"{name} ahead: chore siblings must name {expected}")
        elif "detached worktree" in output:
            problems.append(f"{name}: a pinned sibling must not get an ahead warning")
for problem in problems:
    print("FAIL  " + problem, file=sys.stderr)
if problems:
    sys.exit(1)
PY

echo 'PASS  siblings names pinned detached worktrees when ahead, accepts pins, and refuses older siblings'
