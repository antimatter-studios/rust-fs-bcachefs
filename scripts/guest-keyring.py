#!/usr/bin/env python3
"""guest-keyring.py -- print the bcachefs keys the reference unlock left behind.

Runs INSIDE the harness VM (scripts/guest-build-fixtures.sh), right after the
reference unlock command has been given a passphrase file. S1 (9.1.4.2) says
that command derives the passphrase key and adds it to the kernel keyring as
a `user` key described `bcachefs:<UUID>`; this prints every such key in the
user, session and user-session keyrings with its payload in hex, so the
passphrase key can be compared with candidate derivations of the recorded
passphrase outside the VM (docs/clean-room.md, "Encryption: the crypt
field"). It talks to the kernel through the keyctl system call alone, so no
key tool is installed in the VM for it.

Every failure is printed, never hidden: a keyring that cannot be read says
so and why, and the script exits 1 when it found no bcachefs key at all.
"""

import ctypes
import errno
import os
import platform
import struct
import sys

# The keyctl system call's number, per architecture (the kernel's syscall
# tables), and the operations and special keyring ids used here
# (keyctl(2)).
SYS_KEYCTL = {"x86_64": 250, "aarch64": 219}
KEYCTL_DESCRIBE = 6
KEYCTL_READ = 11
KEYRINGS = {"user": -4, "session": -3, "user-session": -5}

libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long


def keyctl(op, *args):
    args = [ctypes.c_long(a) if isinstance(a, int) else a for a in args]
    while len(args) < 4:
        args.append(ctypes.c_long(0))
    r = libc.syscall(
        ctypes.c_long(SYS_KEYCTL[platform.machine()]), ctypes.c_long(op), *args
    )
    if r < 0:
        e = ctypes.get_errno()
        raise OSError(e, f"{errno.errorcode.get(e, e)}: {os.strerror(e)}")
    return r


def read(serial, op=KEYCTL_READ):
    size = keyctl(op, serial, None, 0)
    buf = ctypes.create_string_buffer(size)
    n = keyctl(op, serial, buf, size)
    return buf.raw[: min(n, size)]


def main():
    found = 0
    seen = set()
    for name, ring in KEYRINGS.items():
        try:
            data = read(ring)
        except OSError as e:
            print(f"keyring {name}: cannot read it: {e}")
            continue
        serials = struct.unpack(f"<{len(data) // 4}i", data[: len(data) // 4 * 4])
        print(f"keyring {name}: {len(serials)} keys")
        for s in serials:
            try:
                desc = read(s, KEYCTL_DESCRIBE).rstrip(b"\0").decode(errors="replace")
            except OSError as e:
                print(f"  key {s}: cannot describe it: {e}")
                continue
            kind, _uid, _gid, _perm, description = desc.split(";", 4)
            if kind != "user" or not description.startswith("bcachefs:"):
                print(f"  key {s}: {kind} {description} (not a bcachefs key)")
                continue
            if s in seen:
                print(f"  key {s}: {description} (listed above)")
                continue
            seen.add(s)
            try:
                payload = read(s)
            except OSError as e:
                print(f"  key {s}: {description}: cannot read it: {e}")
                continue
            found += 1
            print(
                f"  key {s}: {description} payload {len(payload)} bytes {payload.hex()}"
            )
    return 0 if found else 1


if __name__ == "__main__":
    sys.exit(main())
