#!/usr/bin/env bash
# The Darwin static library check (#705), scripts/check-darwin-staticlib.py,
# on archives built here byte by byte: no compiler and no Apple tool.
#
# It must accept an archive of arm64 Mach-O objects naming the symbol, in the
# BSD and GNU long-name forms, and refuse a Linux object, an x86_64 Mach-O
# object, an empty archive, a missing symbol and a file that is no archive.
# And CI must run it on the library `chore staticlib` publishes.
#
#   bash tests/scripts/darwin-staticlib-is-validated.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHECK="$REPO/scripts/check-darwin-staticlib.py"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-staticlib.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

python3 - "$tmp" <<'PY'
import pathlib
import struct
import sys

out = pathlib.Path(sys.argv[1])


def member(name, body):
    header = b"%-16s%-12s%-6s%-6s%-8s%-10d`\n" % (name, b"0", b"0", b"0", b"644", len(body))
    return header + body + (b"\n" if len(body) % 2 else b"")


def bsd(name, body):
    padded = name + b"\0" * (4 - len(name) % 4)
    return member(b"#1/%d" % len(padded), padded + body)


def macho(cpu):
    return struct.pack("<II", 0xFEEDFACF, cpu) + b"\0_fs_bcachefs_mount\0"


arm = macho(0x0100000C)
magic = b"!<arch>\n"
(out / "bsd.a").write_bytes(magic + bsd(b"__.SYMDEF", b"\0" * 8) + bsd(b"a-long-object-name.o", arm))
(out / "gnu.a").write_bytes(
    magic + member(b"/", b"\0" * 4) + member(b"//", b"a-long-object-name.o/\n") + member(b"/0", arm)
)
(out / "elf.a").write_bytes(magic + bsd(b"a.o", b"\x7fELF" + arm))
(out / "x86.a").write_bytes(magic + bsd(b"a.o", macho(0x01000007)))
(out / "empty.a").write_bytes(magic)
(out / "text.a").write_bytes(b"not an archive")
PY

expect() {
    local want="$1" file="$2" symbol="$3"
    python3 "$CHECK" "$tmp/$file" "$symbol" >"$tmp/out" 2>&1
    local got=$?
    [ "$got" -eq "$want" ] || fail "$file with $symbol: status $got, expected $want: $(cat "$tmp/out")"
}

expect 0 bsd.a fs_bcachefs_mount
expect 0 gnu.a fs_bcachefs_mount
expect 1 bsd.a fs_bcachefs_absent
expect 1 elf.a fs_bcachefs_mount
expect 1 x86.a fs_bcachefs_mount
expect 1 empty.a fs_bcachefs_mount
expect 1 text.a fs_bcachefs_mount
expect 1 absent.a fs_bcachefs_mount

grep -q 'chore staticlib$' "$REPO/.github/workflows/ci.yml" ||
    fail "ci.yml does not build the library through chore staticlib"
grep -q 'python3 scripts/check-darwin-staticlib.py dist/libfs_bcachefs.a fs_bcachefs_mount' \
    "$REPO/.github/workflows/ci.yml" || fail "ci.yml does not check the published Darwin library"

[ "$fails" -eq 0 ] || exit 1
echo "PASS  only an arm64 Mach-O static library passes, and CI checks the published one"
