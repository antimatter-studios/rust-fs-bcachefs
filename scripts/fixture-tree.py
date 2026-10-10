#!/usr/bin/env python3
"""The deterministic source tree every formatter-made fixture is built from:
the same bytes on every run, so a fixture that changes means the formatter
changed, not the input. Shared by scripts/guest-build-fixtures.sh (in the
harness guest) and scripts/kernel-oracle.sh (on the CI host).

    python3 scripts/fixture-tree.py ROOT
"""
import os, random, sys
root = sys.argv[1]
rng = random.Random(20261006)
def write(path, data):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "wb") as f:
        f.write(data)
write("hello.txt", b"hello world\n")
write("empty", b"")
write("dir/sub/b.txt", b"abc")
write("dir/notes.md", b"# notes\n" + b"line of text\n" * 300)
# Compressible: repeated text, several hundred KiB, crossing extent limits.
write("big/text.log", b"".join(b"%08d the quick brown fox jumps over the lazy dog\n" % i for i in range(20000)))
# Incompressible: 3 MiB of seeded pseudo-random bytes.
write("big/random.bin", bytes(rng.getrandbits(8) for _ in range(3 * 1024 * 1024)))
# An odd size that ends mid-sector.
write("big/odd.bin", bytes(rng.getrandbits(8) for _ in range(70001)))
# Many entries in one directory, so the dirents and inodes btrees grow.
for i in range(300):
    write("many/file-%04d.txt" % i, b"entry %d\n" % i)
os.symlink("hello.txt", os.path.join(root, "link"))
os.symlink("dir/sub/b.txt", os.path.join(root, "dir/link-to-b"))
