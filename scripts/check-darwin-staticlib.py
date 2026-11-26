#!/usr/bin/env python3
"""Check that a static library is the one a macOS consumer links (#705).

    python3 scripts/check-darwin-staticlib.py LIBRARY SYMBOL...

LIBRARY must be an ar archive whose every member, the symbol tables aside, is
a 64-bit Mach-O object for arm64, and each SYMBOL (a C name, without the
leading underscore) must be named in it. A Linux or x86_64 archive left in
the same place, or an empty one, fails. The check needs no Apple tool, so it
runs where the library is cross-built.
"""

import struct
import sys

ARCHIVE_MAGIC = b"!<arch>\n"
HEADER_BYTES = 60
MACH_O_64 = 0xFEEDFACF
CPU_ARM64 = 0x0100000C


class Refused(Exception):
    pass


def members(data):
    """Each member's name and bytes, in either ar format's long names."""
    if not data.startswith(ARCHIVE_MAGIC):
        raise Refused("not an ar archive")
    at = len(ARCHIVE_MAGIC)
    long_names = b""
    while at < len(data):
        header = data[at : at + HEADER_BYTES]
        if len(header) != HEADER_BYTES or header[58:60] != b"`\n":
            raise Refused(f"a damaged member header at byte {at}")
        name = header[:16].rstrip(b" ")
        try:
            size = int(header[48:58])
        except ValueError as error:
            raise Refused(f"a member size that is not a number at byte {at}") from error
        body = data[at + HEADER_BYTES : at + HEADER_BYTES + size]
        if size < 0 or len(body) != size:
            raise Refused(f"a member cut short at byte {at}")
        at += HEADER_BYTES + size + (size & 1)
        if name.startswith(b"#1/"):  # BSD: the name leads the body.
            length = int(name[3:])
            name, body = body[:length].rstrip(b"\0"), body[length:]
        elif name == b"//":  # GNU: the table of long names.
            long_names = body
            continue
        elif name.startswith(b"/") and name[1:].isdigit():
            name = long_names[int(name[1:]) :].split(b"/\n", 1)[0]
        yield name, body


def check(data, symbols):
    objects = 0
    for name, body in members(data):
        if name in (b"/", b"/SYM64/") or name.startswith(b"__.SYMDEF"):
            continue
        label = name.decode("utf-8", "replace")
        if len(body) < 8:
            raise Refused(f"{label}: too short to be an object")
        magic, cpu = struct.unpack_from("<II", body)
        if magic != MACH_O_64 or cpu != CPU_ARM64:
            raise Refused(f"{label}: not an arm64 Mach-O object")
        objects += 1
    if not objects:
        raise Refused("the archive holds no object")
    for symbol in symbols:
        if b"_" + symbol.encode() + b"\0" not in data:
            raise Refused(f"the archive does not name {symbol}")
    return objects


def main(argv):
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        with open(argv[1], "rb") as stream:
            objects = check(stream.read(), argv[2:])
    except (OSError, Refused) as error:
        print(f"check-darwin-staticlib: {argv[1]}: {error}", file=sys.stderr)
        return 1
    print(f"check-darwin-staticlib: {argv[1]}: {objects} arm64 Mach-O objects")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
