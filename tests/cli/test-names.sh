#!/usr/bin/env bash
# Every name this repository installs resolves on PATH and answers
# --version as itself and this crate, at the one version the entry point
# reports.
source "$(dirname "$0")/lib.sh"

# Written here, not read from the binary: a binary that forgot one would
# otherwise agree with itself.
EXPECTED="fs.bcachefs"

version="$(rust-fs-bcachefs --version | sed -n "s/^rust-fs-bcachefs ($CRATE) //p")"
check "rust-fs-bcachefs --version names a version" test -n "$version"

listed="$(rust-fs-bcachefs generate names | tr '\n' ' ' | sed 's/ $//')"
check "rust-fs-bcachefs generate names lists exactly '$EXPECTED' (got '$listed')" \
    test "$listed" = "$EXPECTED"

for name in $EXPECTED rust-fs-bcachefs; do
    path="$(command -v "$name" 2>/dev/null || true)"
    if [ -z "$path" ]; then
        fail "$name is not on PATH"
        continue
    fi
    ok
    got="$("$name" --version 2>&1)"
    check "$path --version answered '$got', not '$name ($CRATE) $version'" \
        test "$got" = "$name ($CRATE) $version"
done

# The repository-named form reaches the tool.
got="$(rust-fs-bcachefs fs --version 2>&1)"
check "rust-fs-bcachefs fs --version answered '$got'" test "$got" = "fs.bcachefs ($CRATE) $version"

# A bare entry point shows its help and says nothing was done.
rust-fs-bcachefs >"$SANDBOX/bare.out" 2>&1
check "a bare rust-fs-bcachefs exits 2" test $? -eq 2
check "a bare rust-fs-bcachefs lists doctor" grep -q 'doctor' "$SANDBOX/bare.out"

finish
