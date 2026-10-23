#!/usr/bin/env bash
# No Python bytecode is tracked (#393).
#
# `scripts/__pycache__/differential-fuzz.cpython-314.pyc` was committed once:
# CPython's cache of a module, written wherever something imports it, stale the
# moment the module changes, and joined by a sibling for each other Python
# version that imports it. A cleanup that removed the directory then showed the
# tracked file as deleted. So the index holds no `*.pyc` and no `__pycache__`
# path, and .gitignore keeps a new one out.
#
#   bash tests/scripts/no-bytecode-is-tracked.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fail() { echo "FAIL  $*" >&2; exit 1; }

command -v git >/dev/null 2>&1 || fail "git is required"
tracked="$(git -C "$REPO" ls-files -- '*.pyc' '*__pycache__*')" ||
    fail "git ls-files cannot read the index of $REPO"
[ -z "$tracked" ] || fail "Python bytecode is tracked: $(printf '%s' "$tracked" | tr '\n' ' ')"
for path in scripts/__pycache__/differential-fuzz.cpython-313.pyc tests/scripts/stray.pyc; do
    git -C "$REPO" check-ignore -q --no-index -- "$path" ||
        fail ".gitignore does not keep $path out"
done
echo "PASS  no Python bytecode is tracked, and .gitignore keeps it out"
