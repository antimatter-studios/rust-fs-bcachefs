#!/usr/bin/env bash
# guest-build-fixtures.sh -- runs as root INSIDE the harness VM (see
# scripts/build-fixtures.sh). Makes every fixture image with the reference
# formatter and records, beside it, what the reference tools say about it.
#
# For each set NAME this writes, into /share/fixtures:
#   NAME.img          the image (sparse)
#   NAME.json         the tree that was put in: every path, its type, size,
#                     mode, symlink target and the SHA-256 of its contents
#   NAME.super.txt    the reference superblock printer's report
#   NAME.<btree>.txt  the reference lister's keys for inodes, dirents, extents
#   NAME.fsck.txt     the reference checker's verdict (must be clean)
#
# The reference tools are reached only through `bcachefs-ref`, the wrapper
# scripts/vm-setup.sh installs. Nothing here mounts: the formatter populates
# the image from a directory itself.
set -euo pipefail
# One deadline per step, five minutes unless the step says otherwise
# (scripts/guest-watchdog.sh).
# shellcheck source=scripts/guest-watchdog.sh
source /repo/scripts/guest-watchdog.sh

out=/share/fixtures
work=/share/fixtures-work
rm -rf "$out" "$work"
mkdir -p "$out" "$work"

# A deterministic source tree: the same bytes on every run, so a fixture
# that changes means the formatter changed, not the input.
make_tree() {
    python3 /repo/scripts/fixture-tree.py "$1"
}

# manifest ROOT DEST [live]: every path under ROOT with its type, mode, size,
# symlink target and SHA-256. `live` is for a mounted filesystem: it also
# records each inode number and link count, and leaves out lost+found, which
# the filesystem made and nobody put there.
manifest() {
    local root="$1" dest="$2" live="${3:-}"
    python3 - "$root" "$dest" "$live" <<'PY'
import hashlib, json, os, stat, sys
root, dest, live = sys.argv[1], sys.argv[2], sys.argv[3] == "live"
entries = []
for dirpath, dirnames, filenames in os.walk(root):
    if live and dirpath == root and "lost+found" in dirnames:
        dirnames.remove("lost+found")
    dirnames.sort()
    for name in sorted(dirnames) + sorted(filenames):
        full = os.path.join(dirpath, name)
        rel = "/" + os.path.relpath(full, root)
        if live and rel == "/lost+found":
            continue
        st = os.lstat(full)
        e = {"path": rel, "mode": st.st_mode & 0o7777}
        if live:
            e["ino"] = st.st_ino
            e["nlink"] = st.st_nlink
            # Extended attributes as one flat, sorted "name=hex;..." string.
            names = sorted(os.listxattr(full, follow_symlinks=False))
            if names:
                e["xattrs"] = ";".join(
                    n + "=" + os.getxattr(full, n, follow_symlinks=False).hex() for n in names
                )
        if stat.S_ISLNK(st.st_mode):
            e["type"] = "symlink"
            e["target"] = os.readlink(full)
            e["size"] = len(e["target"])
        elif stat.S_ISDIR(st.st_mode):
            e["type"] = "dir"
        else:
            e["type"] = "file"
            data = open(full, "rb").read()
            e["size"] = len(data)
            e["sha256"] = hashlib.sha256(data).hexdigest()
        entries.append(e)
entries.sort(key=lambda e: e["path"])
json.dump({"entries": entries}, open(dest, "w"), indent=1)
PY
}

# name | formatter options
sets=(
    "default|"
    "lz4|--compression=lz4"
    "zstd|--compression=zstd"
    "gzip|--compression=gzip"
    "nocsum|--data_checksum=none --metadata_checksum=none"
    "xxhash|--data_checksum=xxhash --metadata_checksum=xxhash"
    "crc64|--data_checksum=crc64 --metadata_checksum=crc64"
    "block4k|--block_size=4096"
    # Names hashed with crc32c instead of SipHash (S1 7.7): the reader must
    # still find every name (by scanning), the writer must refuse to place
    # one, and the lister's `hash_type=` pairs the number with the name.
    "strhash|--str_hash=crc32c"
    # Background compression asks the reconcile subsystem to recompress
    # extents later (S1 2.1.3, 9.1.9): if the formatter marks its extents
    # with a reconcile entry (S1 9.1.3.5), this set's extents listing is
    # where its layout is first seen (docs/clean-room.md, open question 19).
    "bgcompress|--background_compression=lz4"
)

src="$work/src"
make_tree "$src"

for entry in "${sets[@]}"; do
    name="${entry%%|*}"
    opts="${entry#*|}"
    img="$out/$name.img"
    step "$name ($opts)"
    truncate -s 64M "$img"
    # shellcheck disable=SC2086
    bcachefs-ref format -q $opts --source="$src" "$img" > "$out/$name.format.txt" 2>&1
    manifest "$src" "$out/$name.json"
    bcachefs-ref show-super "$img" > "$out/$name.super.txt" 2>&1
    for b in inodes dirents extents; do
        bcachefs-ref list -b "$b" "$img" > "$out/$name.$b.txt" 2>&1
    done
    if ! bcachefs-ref fsck -n "$img" > "$out/$name.fsck.txt" 2>&1; then
        echo "the reference checker did not pass $name:" >&2
        tail -n 30 "$out/$name.fsck.txt" >&2
        exit 1
    fi
done

# THE LARGE SET: one directory of 30000 entries, so the inodes and dirents
# btrees are many leaves wide and a lookup that reads only its path can be
# told from one that reads the whole tree (tests/oracle_cursor.rs).
# The formatter populates 30000 files: 401 seconds in a green build.
step "large (30000 files in one directory)" 900
large_src="$work/large-src"
python3 - "$large_src" <<'PY'
import os, sys
root = sys.argv[1]
os.makedirs(os.path.join(root, "wide"))
for i in range(30000):
    with open(os.path.join(root, "wide", "entry-%05d" % i), "wb") as f:
        f.write(b"%d\n" % i)
PY
truncate -s 256M "$out/large.img"
bcachefs-ref format -q --source="$large_src" "$out/large.img" > "$out/large.format.txt" 2>&1
manifest "$large_src" "$out/large.json"
bcachefs-ref show-super "$out/large.img" > "$out/large.super.txt" 2>&1
for b in inodes dirents; do
    bcachefs-ref list -b "$b" -m formats "$out/large.img" > "$out/large.$b.formats.txt" 2>&1
done
if ! bcachefs-ref fsck -n "$out/large.img" > "$out/large.fsck.txt" 2>&1; then
    echo "the reference checker did not pass large:" >&2
    tail -n 30 "$out/large.fsck.txt" >&2
    exit 1
fi
rm -rf "$large_src"

# THE AGED SETS. The formatter writes every node fresh; a disk that has been
# used looks different -- narrower key formats, nodes split and rewritten,
# several bsets per node, deletions beside live keys. So one filesystem is
# mounted by the reference implementation (its userspace copy, through
# FUSE: scripts/vm-setup.sh says why not a kernel module) and aged by
# scripts/guest-age.py.
#
# The FUSE daemon of the pinned release aborts on unmount (an assertion in
# its RCU library, after `destroy`), so the filesystem it leaves is never
# cleanly shut down. That is two fixtures, not a problem:
#   aged-unclean  the image exactly as the daemon left it: the newest keys
#                 are in the journal only, and the superblock says unclean;
#   aged          the same image after the reference checker replayed the
#                 journal and shut down cleanly (`fsck -y`).
# The manifest is taken through the mount, after the last write was synced,
# and taken again from a read-only mount of the replayed image; the two
# must be identical, or the set fails.
ROOT=/srv/ref-trixie
age_dir="$ROOT/var/tmp/age"
mnt=/mnt/aged
mkdir -p "$age_dir" "$ROOT$mnt"

fuse_mount() { # IMAGE OPTIONS LOG
    (bcachefs-ref fusemount -f -o "$2" "$1" "$mnt" >"$3" 2>&1 &)
    for _ in $(seq 1 60); do
        mountpoint -q "$ROOT$mnt" && return 0
        sleep 1
    done
    echo "the reference implementation did not mount $1:" >&2
    grep -v '^\[<0>\]' "$3" | tail -n 30 >&2
    exit 1
}
# The way the burst's mount ends: the daemon is killed, so the filesystem
# is never shut down, whatever the daemon would have done on unmount.
fuse_kill() {
    pkill -KILL -f "bcachefs fusemount" || true
    for _ in $(seq 1 30); do
        pgrep -f "bcachefs fusemount" >/dev/null || break
        sleep 1
    done
    fusermount3 -uz "$ROOT$mnt" 2>/dev/null || umount -l "$ROOT$mnt" 2>/dev/null || true
}
fuse_unmount() {
    sync
    fusermount3 -u "$ROOT$mnt"
    for _ in $(seq 1 60); do
        pgrep -f "bcachefs fusemount" >/dev/null || return 0
        sleep 1
    done
    echo "the FUSE daemon did not exit" >&2
    exit 1
}

step "aged (mounted and aged by the reference implementation)"
img=/var/tmp/age/aged.img
rm -f "$ROOT$img"
truncate -s 256M "$ROOT$img"
bcachefs-ref format -q "$img" > "$out/aged.format.txt" 2>&1
# noatime: reading the tree back for the manifest must not itself write.
fuse_mount "$img" rw,noatime,journal_reclaim_delay=60000 "$work/fuse-age.log"
python3 /repo/scripts/guest-age.py "$ROOT$mnt"
sync
manifest "$ROOT$mnt" "$work/aged.mounted.json" live
# The burst, then the daemon is killed: what the burst did reaches the disk
# through the journal (it waits for a flush first), and the btree nodes
# have not all caught up.
python3 /repo/scripts/guest-age.py "$ROOT$mnt" --burst
fuse_kill

cp --sparse=always "$ROOT$img" "$out/aged-unclean.img"
bcachefs-ref show-super "$img" > "$out/aged-unclean.super.txt" 2>&1
grep -q '^Clean: *0' "$out/aged-unclean.super.txt" || {
    echo "aged-unclean: the image is marked clean; it was meant to be the unreplayed one" >&2
    exit 1
}
# The journal as the reference reads it: every entry's header, and the keys
# of the entries a replay applies.
bcachefs-ref list_journal -a -H "$img" > "$out/aged-unclean.journal-headers.txt" 2>&1
bcachefs-ref list_journal -d -V false "$img" > "$out/aged-unclean.journal-dirty.txt" 2>&1

bcachefs-ref fsck -y "$img" > "$out/aged.replay.txt" 2>&1 || {
    echo "the reference checker could not replay the aged image:" >&2
    tail -n 30 "$out/aged.replay.txt" >&2
    exit 1
}
fuse_mount "$img" ro,noatime "$work/fuse-ro.log"
manifest "$ROOT$mnt" "$work/aged.replayed.json" live
fuse_unmount
# What the reference sees after its own replay is the expected view of both
# images. Everything outside /late must also be what the mount wrote.
python3 - "$work/aged.mounted.json" "$work/aged.replayed.json" <<'PY'
import json, sys
before = json.load(open(sys.argv[1]))["entries"]
after = [e for e in json.load(open(sys.argv[2]))["entries"]
         if e["path"] != "/late" and not e["path"].startswith("/late/")]
if before != after:
    print("aged: the replayed tree differs from what the mount wrote, outside /late", file=sys.stderr)
    sys.exit(1)
PY
cp "$work/aged.replayed.json" "$out/aged.json"
cp "$work/aged.replayed.json" "$out/aged-unclean.json"
cp --sparse=always "$ROOT$img" "$out/aged.img"
bcachefs-ref show-super "$img" > "$out/aged.super.txt" 2>&1
grep -q '^Clean: *1' "$out/aged.super.txt" || {
    echo "aged: the replayed image is not marked clean" >&2
    exit 1
}
bcachefs-ref list -b xattrs "$img" > "$out/aged.xattrs.txt" 2>&1
for b in inodes dirents extents; do
    bcachefs-ref list -b "$b" "$img" > "$out/aged.$b.txt" 2>&1
    bcachefs-ref list -b "$b" -m formats "$img" > "$out/aged.$b.formats.txt" 2>&1
done
if ! bcachefs-ref fsck -n "$img" > "$out/aged.fsck.txt" 2>&1; then
    echo "the reference checker did not pass aged:" >&2
    tail -n 30 "$out/aged.fsck.txt" >&2
    exit 1
fi

# EXTENT ENTRIES (#52): the entry kinds beyond ptr, crc32, crc64 and
# reconcile, made where one device can make them, so each one's layout can
# be read off the image against the lister's printed fields.
#
# crc128. S1 (9.1.3.2): the write path takes the smallest crc entry that
# holds the extent, crc64 holds 512 sectors at most and crc128 8192, and
# encoded extents are at most encoded_extent_max (256k, 512 sectors, by
# default). So this set raises encoded_extent_max to 1M, and is made on an
# image large enough that the formatter picks buckets above 256k (it gave
# 32k on 64M and 128k on 256M), so an extent can outgrow crc64.
echo "== crc128 (--encoded_extent_max=1M --compression=lz4, 1G)"
truncate -s 1G "$out/crc128.img"
bcachefs-ref format -q --encoded_extent_max=1M --compression=lz4 --source="$src" \
    "$out/crc128.img" >"$out/crc128.format.txt" 2>&1
manifest "$src" "$out/crc128.json"
bcachefs-ref show-super "$out/crc128.img" >"$out/crc128.super.txt" 2>&1
for b in inodes dirents extents; do
    bcachefs-ref list -b "$b" "$out/crc128.img" >"$out/crc128.$b.txt" 2>&1
done
# The lister's raw view of each node, beside the image's own bytes.
bcachefs-ref list -b extents -m nodes-ondisk "$out/crc128.img" \
    >"$out/crc128.extents.ondisk.txt" 2>&1 || true
if ! bcachefs-ref fsck -n "$out/crc128.img" >"$out/crc128.fsck.txt" 2>&1; then
    echo "the reference checker did not pass crc128:" >&2
    tail -n 30 "$out/crc128.fsck.txt" >&2
    exit 1
fi
# A crc128 whose offset is not 0: every extent the formatter wrote starts at
# its data's start, so which of the crc128's two 13-bit fields after the
# sizes is the offset is not seen there. An extent partly overwritten keeps
# its entry and starts its live part further in (S1 9.1.3.2), so a file is
# written through the mount with the crc128 set's options, settled, and 4K
# of it overwritten. crc128-overwrite.txt records the steps; nothing here
# fails the build.
echo "== crc128-overwrite (a crc128 extent partly overwritten, through the mount)"
img=/var/tmp/age/crc128-overwrite.img
rm -f "$ROOT$img"
truncate -s 1G "$ROOT$img"
bcachefs-ref format -q --encoded_extent_max=1M "$img" >"$out/crc128-overwrite.format.txt" 2>&1
{
    if (fuse_mount "$img" rw,noatime "$work/fuse-crc128.log"); then
        python3 - "$ROOT$mnt" <<'PY' || echo "write: exit $?"
import os, random, sys
with open(os.path.join(sys.argv[1], "f"), "wb") as f:
    f.write(random.Random(128).randbytes(1 << 20))
PY
        sync
        sleep 2
        fuse_kill
        bcachefs-ref fsck -y "$img" >"$out/crc128-overwrite.written.settle.txt" 2>&1 ||
            echo "settle written: exit $?"
        bcachefs-ref list -b extents "$img" >"$out/crc128-overwrite.written.extents.txt" 2>&1 || true
        if (fuse_mount "$img" rw,noatime "$work/fuse-crc128.log"); then
            python3 - "$ROOT$mnt" <<'PY' || echo "overwrite: exit $?"
import os, sys
with open(os.path.join(sys.argv[1], "f"), "r+b") as f:
    f.seek(4096)
    f.write(b"overwritten\n" * 341 + b"end\n")
PY
            sync
            sleep 2
            manifest "$ROOT$mnt" "$out/crc128-overwrite.json" live
            fuse_kill
            bcachefs-ref fsck -y "$img" >"$out/crc128-overwrite.replay.txt" 2>&1 ||
                echo "settle overwritten: exit $?"
            for b in inodes dirents extents; do
                bcachefs-ref list -b "$b" "$img" >"$out/crc128-overwrite.$b.txt" 2>&1 || true
            done
            bcachefs-ref fsck -n "$img" >"$out/crc128-overwrite.fsck.txt" 2>&1 &&
                echo "fsck -n: clean" || echo "fsck -n: exit $?"
            grep -c 'crc128:' "$out/crc128-overwrite.extents.txt" | sed 's/$/ crc128 entries/'
            grep 'crc128:.* offset [1-9]' "$out/crc128-overwrite.extents.txt" || echo "no crc128 offset above 0"
            cp --sparse=always "$ROOT$img" "$out/crc128-overwrite.img"
        fi
    fi
} >"$out/crc128-overwrite.txt" 2>&1 || echo "the steps stopped: exit $?" >>"$out/crc128-overwrite.txt"
sed 's/^/crc128-overwrite: /' "$out/crc128-overwrite.txt"

# POISONED EXTENTS. S1 (5.5.5, 9.1.2.1): an extent whose data fails its
# checksum, with no good copy, is marked poisoned -- when it is read, or
# when it is moved (the move writes a fresh checksum over the bad data, and
# the flag is all that remembers it). Both are tried on one image: a file
# is written with reconcile off and the image settled, one sector of its
# first extent is corrupted, then it is read through the mount with
# reconcile still off, and then the image is mounted with reconcile on,
# which background compression gives work to. poison.txt records each
# step's outcome, poison.<step>.extents.txt the lister's view after it,
# and poison.img is the image after the last.
echo "== poison (a corrupted data sector, read and then moved by the reference)"
img=/var/tmp/age/poison.img
rm -f "$ROOT$img"
truncate -s 64M "$ROOT$img"
bcachefs-ref format -q --background_compression=lz4 "$img" >"$out/poison.format.txt" 2>&1
poison_settle() { # STEP
    sync
    sleep 2
    fuse_kill
    bcachefs-ref fsck -y "$img" >"$out/poison.$1.settle.txt" 2>&1 || echo "settle $1: exit $?"
    bcachefs-ref list -b extents "$img" >"$out/poison.$1.extents.txt" 2>&1 || true
    echo "after $1: $(grep -c 'flags: poisoned' "$out/poison.$1.extents.txt") extents poisoned"
}
{
    echo "## write (reconcile_enabled=0)"
    if (fuse_mount "$img" rw,noatime,reconcile_enabled=0 "$work/fuse-poison.log"); then
        python3 - "$ROOT$mnt" <<'PY' || echo "write: exit $?"
import os, sys
m = sys.argv[1]
with open(os.path.join(m, "victim"), "wb") as f:
    f.write(b"".join(b"%08d this line is read back or refused\n" % i for i in range(4000)))
with open(os.path.join(m, "intact"), "wb") as f:
    f.write(b"an intact file\n" * 100)
PY
        echo "victim inode $(stat -c %i "$ROOT$mnt/victim")" | tee "$work/poison.ino"
        poison_settle written
        echo "## corrupt the first sector of the victim's first extent"
        bcachefs-ref show-super "$img" >"$out/poison.super.txt" 2>&1
        python3 - "$ROOT$img" "$out/poison.written.extents.txt" "$out/poison.super.txt" \
            "$(awk '{print $3}' "$work/poison.ino")" <<'PY' || echo "corrupt: exit $?"
import re, sys
img, listing, sup, ino = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
size = re.search(r"Bucket size:\s*([0-9.]+)([kM]?)", open(sup).read())
bucket = float(size.group(1)) * {"": 1, "k": 1024, "M": 1 << 20}[size.group(2)] / 512
key = None
for line in open(listing):
    if line.startswith("u64s"):
        key = line if re.search(r" extent %s:" % ino, line) else None
    elif key and line.strip().startswith("ptr:"):
        dev, b, off = (int(x) for x in re.search(r" (\d+):(\d+):(\d+) gen", line).groups())
        sector = int(b * bucket + off)
        with open(img, "r+b") as f:
            f.seek(sector * 512 + 100)
            bad = bytes(x ^ 0xFF for x in f.read(64))
            f.seek(sector * 512 + 100)
            f.write(bad)
        print(f"corrupted 64 bytes of sector {sector} (bucket {b} + {off}), under {key.split(':')[0]}...")
        break
else:
    print("no pointer found for inode", ino)
PY
        echo "## read through the mount (reconcile_enabled=0)"
        if (fuse_mount "$img" rw,noatime,reconcile_enabled=0 "$work/fuse-poison.log"); then
            cat "$ROOT$mnt/victim" >/dev/null 2>&1 && echo "victim: read" || echo "victim: read failed ($?)"
            cmp -s "$ROOT$mnt/intact" <(yes 'an intact file' | head -n 100) && echo "intact: read" ||
                echo "intact: differs"
            poison_settle read
        fi
        echo "## mount with reconcile on, for 30 seconds"
        if (fuse_mount "$img" rw,noatime "$work/fuse-poison.log"); then
            sleep 30
            cat "$ROOT$mnt/victim" >/dev/null 2>&1 && echo "victim: read" || echo "victim: read failed ($?)"
            poison_settle moved
        fi
    fi
} >"$out/poison.txt" 2>&1 || echo "the experiment stopped: exit $?" >>"$out/poison.txt"
sed 's/^/poison: /' "$out/poison.txt"
cp "$out/poison.moved.extents.txt" "$out/poison.extents.txt" 2>/dev/null || true
bcachefs-ref list -b extents -m nodes-ondisk "$img" >"$out/poison.extents.ondisk.txt" 2>&1 || true
bcachefs-ref fsck -n "$img" >"$out/poison.fsck.txt" 2>&1 || true
cp --sparse=always "$ROOT$img" "$out/poison.img"

# Stripe pointers link an extent to an erasure-coding stripe (S1 9.1.3.3),
# which takes several devices; a reader of one device never meets one.
# Tried once on three devices so the lister shows whether, and how, the
# reference writes one. No test reads these images.
echo "== probe: erasure coding on three devices"
ec=/var/tmp/age/ec
{
    for i in 0 1 2; do
        rm -f "$ROOT$ec-$i.img"
        truncate -s 128M "$ROOT$ec-$i.img"
    done
    bcachefs-ref format -q --erasure_code --replicas=2 "$ec-0.img" "$ec-1.img" "$ec-2.img" ||
        echo "format: exit $?"
    if (fuse_mount "$ec-0.img:$ec-1.img:$ec-2.img" rw,noatime "$work/fuse-ec.log"); then
        python3 -c 'import random,sys; sys.stdout.buffer.write(random.Random(9).randbytes(8 << 20))' \
            >"$ROOT$mnt/data" || echo "write: exit $?"
        sync
        sleep 5
        fuse_kill
    else
        echo "the reference implementation did not mount the three devices"
    fi
    timeout 300 bcachefs-ref fsck -y "$ec-0.img" "$ec-1.img" "$ec-2.img" || echo "fsck: exit $?"
    for b in extents stripes; do
        echo "## list -b $b"
        timeout 300 bcachefs-ref list -b "$b" "$ec-0.img" "$ec-1.img" "$ec-2.img" \
            >"$work/ec.$b.txt" 2>&1 || echo "list $b: exit $?"
        head -n 400 "$work/ec.$b.txt"
    done
    # The stripe pointer's own bytes: each crc32 entry the lister prints
    # before a stripe_ptr line is built from its printed fields (the crc32
    # layout in src/extent.rs) and found in the images, at every place it
    # is (an older copy of the key, from before the stripe was made, holds
    # two pointers instead: CI run 37812305157); the words after the copy
    # that holds it are the stripe pointer and the pointer.
    echo "## the words after each copy of each crc32 entry that precedes a stripe_ptr"
    python3 - "$work/ec.extents.txt" "$ROOT$ec-0.img" "$ROOT$ec-1.img" "$ROOT$ec-2.img" <<'PY'
import mmap, re, struct, sys
lines = open(sys.argv[1]).read().splitlines()
files = [open(p, "rb") for p in sys.argv[2:]]
maps = [mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) for f in files]
shown = 0
for i, l in enumerate(lines[:-2]):
    m = re.match(r"\s+crc32: c_size (\d+) size (\d+) offset (\d+) .*csum crc32c \w+:(\w+)\s+compress none", l)
    if not m or "stripe_ptr:" not in lines[i + 1]:
        continue
    cs, size, off, csum = int(m[1]), int(m[2]), int(m[3]), int(m[4], 16)
    w = csum << 32 | 5 << 24 | off << 16 | (size - 1) << 9 | (cs - 1) << 2 | 2
    copies = set()
    for mm in maps:
        at = mm.find(struct.pack("<Q", w))
        while at >= 0:
            if at + 24 <= len(mm):
                copies.add(struct.unpack_from("<3Q", mm, at))
            at = mm.find(struct.pack("<Q", w), at + 1)
    print(f"{lines[i + 1].strip()} | {lines[i + 2].strip()} | {len(copies)} distinct copies:")
    for c in sorted(copies):
        print("   ", " ".join(f"{x:#018x}" for x in c))
    shown += 1
    if shown == 8:
        break
PY
} >"$out/probe-ec.txt" 2>&1 || echo "the probe stopped: exit $?" >>"$out/probe-ec.txt"
rm -f "$ROOT$ec"-*.img

# THE REFUSED SETS: what this reader must recognise and refuse with a clear
# error rather than misread. An encrypted filesystem (its master key stored
# unencrypted, so no passphrase is involved), and both members of a
# two-device filesystem. Only the superblock printer's view is recorded.
step "encrypted (--encrypted --no_passphrase)"
truncate -s 64M "$out/encrypted.img"
bcachefs-ref format -q --encrypted --no_passphrase --source="$src" "$out/encrypted.img" \
    > "$out/encrypted.format.txt" 2>&1
bcachefs-ref show-super "$out/encrypted.img" > "$out/encrypted.super.txt" 2>&1
step "multi (two devices)"
truncate -s 64M "$out/multi-0.img" "$out/multi-1.img"
bcachefs-ref format -q "$out/multi-0.img" "$out/multi-1.img" > "$out/multi.format.txt" 2>&1
for i in 0 1; do
    bcachefs-ref show-super "$out/multi-$i.img" > "$out/multi-$i.super.txt" 2>&1
done

# CASEFOLDED DIRECTORIES (#54). S1 (2.7, 7.2): casefold is a per-directory
# option that can also be given at format time, and a casefolded
# directory's entries store both the name as given and its folded form,
# looked up by the folded form. The reference mount refuses to set it on a
# directory (probe.txt), so this set asks the formatter for it
# filesystem-wide and populates the image from a tree of mixed-case and
# non-ASCII names. A second image is formatted the same way and populated
# through the reference mount, which records whether a name is found in
# another case. casefold.txt holds every step's outcome; nothing here
# fails the build, and tests/oracle_casefold.rs fails if the set does not
# hold a casefolded directory.
echo "== casefold (--casefold)"
cf_src="$work/casefold-src"
python3 - "$cf_src" <<'PY'
import os, sys
root = sys.argv[1]
def write(path, data):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "wb") as f:
        f.write(data)
write("Hello.TXT", b"hello\n")
write("lower.txt", b"lower\n")
write("MiXeD/Inner.md", b"# inner\n")
write("MiXeD/UPPER", b"upper\n")
write("Straße.txt", "straße\n".encode())
write("ÉCOLE/Fichier", b"fichier\n")
for i in range(40):
    write("Many/File-%02d.Txt" % i, b"file %d\n" % i)
os.symlink("Hello.TXT", os.path.join(root, "Link"))
PY
{
    cf_opt=
    for opt in --casefold --casefold=1; do
        echo "## format $opt --source"
        truncate -s 64M "$out/casefold.img"
        if bcachefs-ref format -q "$opt" --source="$cf_src" "$out/casefold.img" 2>&1; then
            cf_opt="$opt"
            break
        fi
        echo "exit $?"
    done
    if [ -n "$cf_opt" ]; then
        manifest "$cf_src" "$out/casefold.json"
        bcachefs-ref show-super "$out/casefold.img" >"$out/casefold.super.txt" 2>&1
        for b in inodes dirents extents; do
            bcachefs-ref list -b "$b" "$out/casefold.img" >"$out/casefold.$b.txt" 2>&1
        done
        bcachefs-ref list -b dirents -m nodes-ondisk "$out/casefold.img" \
            >"$out/casefold.dirents.ondisk.txt" 2>&1 || echo "nodes-ondisk: exit $?"
        echo "## fsck -n"
        bcachefs-ref fsck -n "$out/casefold.img" >"$out/casefold.fsck.txt" 2>&1 &&
            echo "fsck: clean" || echo "fsck: exit $?"

        echo "## through the mount ($cf_opt)"
        cf_img=/var/tmp/age/casefold-mount.img
        rm -f "$ROOT$cf_img"
        truncate -s 64M "$ROOT$cf_img"
        bcachefs-ref format -q "$cf_opt" "$cf_img"
        if (fuse_mount "$cf_img" rw,noatime "$work/fuse-casefold.log"); then
            m="$ROOT$mnt"
            printf 'mixed\n' >"$m/Hello.TXT"
            mkdir "$m/MiXeD" && printf 'inner\n' >"$m/MiXeD/Inner.md"
            for name in Hello.TXT HELLO.txt hello.txt MIXED/inner.MD; do
                cat "$m/$name" >/dev/null 2>&1 && echo "lookup $name: found" ||
                    echo "lookup $name: not found"
            done
            printf 'second\n' >"$m/HELLO.TXT" 2>&1 || echo "create HELLO.TXT: exit $?"
            ls -la "$m" "$m/MiXeD"
            sync
            sleep 2
            fuse_kill
            bcachefs-ref fsck -y "$cf_img" >"$out/casefold-mount.replay.txt" 2>&1 ||
                echo "replay: exit $?"
            for b in inodes dirents; do
                bcachefs-ref list -b "$b" "$cf_img" >"$out/casefold-mount.$b.txt" 2>&1 || true
            done
            cp --sparse=always "$ROOT$cf_img" "$out/casefold-mount.img"
        else
            echo "the reference implementation did not mount the casefold image"
        fi
    else
        rm -f "$out/casefold.img"
        echo "the formatter took neither --casefold nor --casefold=1"
    fi
} >"$out/casefold.txt" 2>&1 || echo "the casefold steps stopped: exit $?" >>"$out/casefold.txt"
sed 's/^/casefold: /' "$out/casefold.txt"
# The write study's before/after pairs (#20): its own script, its own
# directory under fixtures/.
bash /repo/scripts/guest-write-study.sh

# PROBES: open questions this build cannot settle on purpose, but can
# record. Snapshots and subvolumes (#12), casefolded directories (#54) and
# reflink (#7) each need a fixture that only a running filesystem can make;
# the reference implementation's FUSE mount may or may not honour the
# ioctls behind them. Each is tried once, on a scratch image, and whatever
# happens -- success, "not supported", an error -- goes to probe.txt, with
# the printer's and lister's view of the image afterwards. If one of them
# works, probe.img is the first fixture of its kind and the lister's output
# beside it is where the layout is first observed. Nothing here fails the
# build: a probe that finds nothing is a recorded answer, not a skipped
# step, and the image is not read by any test until a reader for it exists.
step "probes (subvolume, snapshot, casefold, reflink through the reference mount)"
probe_img=/var/tmp/age/probe.img
rm -f "$ROOT$probe_img"
truncate -s 64M "$ROOT$probe_img"
bcachefs-ref format -q "$probe_img" > "$out/probe.format.txt" 2>&1
{
    echo "## mount"
    if (fuse_mount "$probe_img" rw,noatime "$work/fuse-probe.log"); then
        m="$ROOT$mnt"
        mkdir -p "$m/sub" && echo "probe" > "$m/sub/file"
        echo "## subvolume create (bcachefs subvolume create $mnt/subvol)"
        bcachefs-ref subvolume create "$mnt/subvol" 2>&1 || echo "exit $?"
        echo "## subvolume snapshot (bcachefs subvolume snapshot $mnt $mnt/snap)"
        bcachefs-ref subvolume snapshot "$mnt" "$mnt/snap" 2>&1 || echo "exit $?"
        echo "## casefold via set-file-option (--casefold=1 on an empty directory)"
        mkdir -p "$m/casefold"
        bcachefs-ref set-file-option --casefold=1 "$mnt/casefold" 2>&1 || echo "exit $?"
        echo "## casefold via chattr +F"
        chattr +F "$m/casefold" 2>&1 || echo "exit $?"
        echo "Mixed" > "$m/casefold/Name" 2>&1 || echo "exit $?"
        echo "## reflink via cp --reflink=always"
        cp --reflink=always "$m/sub/file" "$m/sub/clone" 2>&1 || echo "exit $?"
        # Every other route to a clone (#7): the clone and dedupe ioctls,
        # whole-file and ranged, and copy_file_range, the one call FUSE
        # passes on to the daemon. Each on the inline file and on one too
        # big to be inline (S1 9.1.7 says inline data reflinks as its own
        # key type). A failure prints `exit 1`, as the shell's do.
        python3 - "$m/sub" <<'PY' 2>&1 || echo "exit $?"
import fcntl, os, struct, sys
d = sys.argv[1]
big = bytes((i * 7 + 3) & 0xFF for i in range(65536))
for name in ("big", "big-twin"):
    with open(os.path.join(d, name), "wb") as f:
        f.write(big)
os.sync()
# Linux's generic clone and dedupe ioctls (ioctl_ficlone(2),
# ioctl_fideduperange(2)).
FICLONE, FICLONERANGE, FIDEDUPERANGE = 0x40049409, 0x4020940D, 0xC0189436
def probe(title, src, dst, how):
    print("## " + title, flush=True)
    s = os.open(os.path.join(d, src), os.O_RDONLY)
    t = os.open(os.path.join(d, dst), os.O_RDWR | os.O_CREAT, 0o644)
    try:
        r = how(s, t, os.fstat(s).st_size)
        print("ok" if isinstance(r, bytes) else f"ok {r}")
    except OSError as e:
        print(e)
        print("exit 1")
    finally:
        os.close(s)
        os.close(t)
def dedupe(s, t, n):
    arg = bytearray(struct.pack("=QQHHI", 0, n, 1, 0, 0) + struct.pack("=qQQiI", t, 0, 0, 0, 0))
    fcntl.ioctl(s, FIDEDUPERANGE, arg)
    done, status = struct.unpack_from("=Qi", arg, 24 + 16)
    if status != 0:
        raise OSError(-status if status < 0 else 0, f"dedupe status {status}, {done} bytes")
    return f"{done} bytes deduplicated"
for src in ("file", "big"):
    probe(f"clone via FICLONE ({src})", src, src + "-ficlone",
          lambda s, t, n: fcntl.ioctl(t, FICLONE, s))
    probe(f"clone via FICLONERANGE ({src})", src, src + "-range",
          lambda s, t, n: fcntl.ioctl(t, FICLONERANGE, struct.pack("=qQQQ", s, 0, 0, 0)))
    probe(f"clone via copy_file_range ({src})", src, src + "-copied",
          lambda s, t, n: os.copy_file_range(s, t, n))
probe("clone via FIDEDUPERANGE (big onto big-twin)", "big", "big-twin", dedupe)
PY
        echo "## listing"
        ls -laR "$m" 2>&1 || true
        # The daemon is killed, so what reaches the image is what the journal
        # flushed: the wait outlasts its flush delay (1 s by default), as the
        # write study's settle does. Without it the image held none of the
        # above (measured in CI: the lister found only lost+found).
        sync
        sleep 2
        fuse_kill
    else
        echo "the reference implementation did not mount the probe image"
    fi
} > "$out/probe.txt" 2>&1
bcachefs-ref fsck -y "$probe_img" > "$out/probe.replay.txt" 2>&1 || true
# SNAPSHOTS BY THE REFERENCE'S OWN EDITOR (#12). The mount cannot make a
# subvolume or a snapshot, but the reference tool's `kvdb` reads and writes
# btree keys by field name, through the normal transactional path, and by
# its own help anticipates fabricated snapshots. First its read-only view
# (the default open, which by its help never writes) of the subvolume,
# snapshot and snapshot-tree keys every image holds: the field names a
# fabricated snapshot is written in. Then one attempt at a second snapshot
# key, on a copy, and what the reference checker says of the result.
kvdb_img=/var/tmp/age/probe-kvdb.img
{
    echo "## kvdb read-only"
    timeout 120 bcachefs-ref kvdb "$probe_img" \
        -c "get subvolumes 0:1:0" -c "get snapshots 0:4294967295:0" \
        -c "get snapshot_trees 0:1:0" -c "list subvolume_children" \
        -c "snapshot 4294967295" -c "get inodes 0:4096" 2>&1 || echo "exit $?"
    echo "## kvdb set a second snapshot key (--rw, on a copy)"
    cp --sparse=always "$ROOT$probe_img" "$ROOT$kvdb_img"
    timeout 120 bcachefs-ref kvdb --rw "$kvdb_img" \
        -c "set snapshots 0:4294967294:0 snapshot parent=4294967295 subvol=1 tree=1 depth=1" \
        -c "get snapshots 0:4294967294:0" -c "list snapshots" 2>&1 || echo "exit $?"
    echo "## the reference checker on the copy"
    bcachefs-ref fsck -n "$kvdb_img" 2>&1 || echo "exit $?"
    echo "## the lister's snapshots on the copy"
    bcachefs-ref list -b snapshots "$kvdb_img" 2>&1 || echo "exit $?"
    # The first attempt's checker named what a snapshot needs: a parent
    # that names it as a child, tree and depth that agree, a state. So a
    # whole snapshot, in the shape S1 (9.4.2) describes: the root snapshot
    # becomes an interior node with two leaves, one per subvolume, and a
    # second subvolume holds the other leaf. One command per run of the
    # editor, so the first refusal does not hide the rest.
    cp --sparse=always "$ROOT$probe_img" "$ROOT$kvdb_img"
    for c in \
        "set snapshots 0:4294967294:0 snapshot parent=4294967295 subvol=1 tree=1 depth=1 state=live" \
        "set snapshots 0:4294967293:0 snapshot parent=4294967295 subvol=2 tree=1 depth=1 state=live" \
        "update snapshots 0:4294967295:0 children[0]=4294967294 children[1]=4294967293 subvol=0" \
        "update subvolumes 0:1:0 snapshot=4294967294" \
        "set subvolumes 0:2:0 subvolume root=4096 snapshot=4294967293 creation_parent=1 fs_parent=1"; do
        echo "## kvdb --rw: $c"
        timeout 120 bcachefs-ref kvdb --rw "$kvdb_img" -c "$c" 2>&1 | grep -v -E '^(Using|starting|  with|recovering|Journal keys|[a-z_]+\.\.\. done|going read-write|clean shutdown)' || true
    done
    echo "## kvdb read-only, after: the keys, and the root inode as each leaf sees it"
    timeout 120 bcachefs-ref kvdb "$kvdb_img" -c "list subvolumes" -c "list snapshots" \
        -c "list snapshot_trees" -c "snapshot 4294967293" -c "get -k inodes 0:4096" \
        -c "snapshot 4294967294" -c "get -k inodes 0:4096" 2>&1 || echo "exit $?"
    echo "## the reference checker on the whole snapshot"
    bcachefs-ref fsck -n "$kvdb_img" 2>&1 | tail -n 40 || true
} > "$out/probe.kvdb.txt" 2>&1
rm -f "$ROOT$kvdb_img"
bcachefs-ref show-super "$probe_img" > "$out/probe.super.txt" 2>&1 || true
for b in inodes dirents extents subvolumes snapshots reflink; do
    bcachefs-ref list -b "$b" "$probe_img" > "$out/probe.$b.txt" 2>&1 || true
done
cp --sparse=always "$ROOT$probe_img" "$out/probe.img"
# What the daemon said while the probes ran: an operation it does not
# implement may be named here and nowhere else.
grep -v '^\[<0>\]' "$work/fuse-probe.log" 2>/dev/null | tail -n 60 > "$out/probe.fuse-log.txt" || true
# What the reference tool offers, by its own account (#7, #12): its
# commands, and the options of every one that might make a reflink, a
# subvolume or a snapshot some other way than through the mount. The
# first run's list (CI, PR #100) named a btree read/write REPL (`kvdb`)
# and image commands; their help is asked for too.
for c in "" format fusemount mount subvolume "subvolume create" "subvolume snapshot" \
    reflink-option-propagate kvdb "image create" "image update" undump; do
    echo "## bcachefs $c --help"
    # shellcheck disable=SC2086 # $c is a command of one or two words
    bcachefs-ref $c --help 2>&1 || echo "exit $?"
done > "$out/probe.help.txt" 2>&1

bcachefs-ref version > "$out/reference-version.txt" 2>&1 || true
# The reference tool's own account of its commands and options, for what
# can be asked of it (format options, offline editing): printed output, the
# same standing as a listing.
{
    for cmd in "" format set-file-option kvdb; do
        echo "## $cmd --help"
        # shellcheck disable=SC2086 # $cmd is empty or one word
        bcachefs-ref $cmd --help
    done
} >"$out/reference-help.txt" 2>&1 || true
rm -rf "$work"
ls -l "$out"
