#!/usr/bin/env python3
"""guest-age.py MOUNTPOINT -- age a mounted bcachefs, deterministically.

Runs INSIDE the harness VM (scripts/guest-build-fixtures.sh), against a
filesystem the reference implementation has mounted. Every operation a real
disk sees over time, so the btrees end up the way a running filesystem
leaves them rather than the way a formatter writes them:

  - thousands of small files in one directory (node splits in the inodes
    and dirents btrees), then half of them deleted (whiteouts, merges);
  - files overwritten whole, in the middle, and truncated;
  - renames within and across directories;
  - hard links;
  - a large file written in interleaved chunks with a second one, with a
    sync between chunks, so its extents are fragmented;
  - a sparse file with a hole;
  - deep directories and symlinks.

Seeded, so the same image contents come out on every run; only the
filesystem's own choices (allocation, timestamps) differ.
"""

import os
import random
import sys

root = sys.argv[1]
rng = random.Random(20261007)


def p(*parts):
    return os.path.join(root, *parts)


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def blob(n):
    return bytes(rng.getrandbits(8) for _ in range(n))


# Many small files in one directory: enough entries to split nodes.
for i in range(4000):
    write(p("many", f"f{i:05d}"), b"small file %d\n" % i * (1 + i % 7))
os.sync()

# Half of them deleted again, every other one, so surviving keys sit beside
# deletions in the same nodes.
for i in range(0, 4000, 2):
    os.unlink(p("many", f"f{i:05d}"))
os.sync()

# Renames: within the directory and out of it.
for i in range(1, 400, 4):
    os.rename(p("many", f"f{i:05d}"), p("many", f"renamed-{i:05d}"))
os.makedirs(p("moved"), exist_ok=True)
for i in range(3, 400, 4):
    os.rename(p("many", f"f{i:05d}"), p("moved", f"f{i:05d}"))
os.sync()

# Overwrites: whole, in the middle, and a truncate.
write(p("over", "whole.bin"), blob(200_000))
os.sync()
write(p("over", "whole.bin"), blob(150_000))
write(p("over", "middle.bin"), blob(300_000))
os.sync()
with open(p("over", "middle.bin"), "r+b") as f:
    f.seek(100_000)
    f.write(blob(4096 * 3))
    f.seek(250_001)
    f.write(b"patched in the middle")
write(p("over", "trunc.bin"), blob(500_000))
os.sync()
os.truncate(p("over", "trunc.bin"), 123_457)

# Hard links.
write(p("links", "orig.txt"), b"one inode, three names\n")
os.link(p("links", "orig.txt"), p("links", "second.txt"))
os.makedirs(p("links", "sub"), exist_ok=True)
os.link(p("links", "orig.txt"), p("links", "sub", "third.txt"))

# A fragmented large file: two files grown in interleaved chunks.
os.makedirs(p("frag"), exist_ok=True)
with open(p("frag", "a.bin"), "wb") as a, open(p("frag", "b.bin"), "wb") as b:
    for _ in range(96):
        a.write(blob(65536))
        a.flush()
        os.fsync(a.fileno())
        b.write(blob(16384))
        b.flush()
        os.fsync(b.fileno())

# A sparse file: data, a hole, data.
with open(p("sparse.bin"), "wb") as f:
    f.write(b"head" * 1024)
    f.seek(8 * 1024 * 1024)
    f.write(b"tail" * 1024)

# Deep directories and symlinks.
deep = p(*[f"d{i}" for i in range(12)])
os.makedirs(deep, exist_ok=True)
write(os.path.join(deep, "leaf.txt"), b"at the bottom\n")
os.symlink("d0/d1/d2", p("shortcut"))
os.symlink("x" * 200, p("long-dangling-link"))

# A file removed after being written, and a directory removed after being
# filled and emptied: neither must appear.
write(p("gone.txt"), b"soon deleted")
os.sync()
os.unlink(p("gone.txt"))
os.makedirs(p("emptied", "inner"), exist_ok=True)
write(p("emptied", "inner", "x"), b"x")
os.sync()
os.unlink(p("emptied", "inner", "x"))
os.rmdir(p("emptied", "inner"))
os.sync()
