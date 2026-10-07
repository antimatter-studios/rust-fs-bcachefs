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

out=/share/fixtures
work=/share/fixtures-work
rm -rf "$out" "$work"
mkdir -p "$out" "$work"

# A deterministic source tree: the same bytes on every run, so a fixture
# that changes means the formatter changed, not the input.
make_tree() {
    local root="$1"
    python3 - "$root" <<'PY'
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
PY
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
)

src="$work/src"
make_tree "$src"

for entry in "${sets[@]}"; do
    name="${entry%%|*}"
    opts="${entry#*|}"
    img="$out/$name.img"
    echo "== $name ($opts)"
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
echo "== large (30000 files in one directory)"
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

echo "== aged (mounted and aged by the reference implementation)"
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

# THE REFUSED SETS: what this reader must recognise and refuse with a clear
# error rather than misread. An encrypted filesystem (its master key stored
# unencrypted, so no passphrase is involved), and both members of a
# two-device filesystem. Only the superblock printer's view is recorded.
echo "== encrypted (--encrypted --no_passphrase)"
truncate -s 64M "$out/encrypted.img"
bcachefs-ref format -q --encrypted --no_passphrase --source="$src" "$out/encrypted.img" \
    > "$out/encrypted.format.txt" 2>&1
bcachefs-ref show-super "$out/encrypted.img" > "$out/encrypted.super.txt" 2>&1
echo "== multi (two devices)"
truncate -s 64M "$out/multi-0.img" "$out/multi-1.img"
bcachefs-ref format -q "$out/multi-0.img" "$out/multi-1.img" > "$out/multi.format.txt" 2>&1
for i in 0 1; do
    bcachefs-ref show-super "$out/multi-$i.img" > "$out/multi-$i.super.txt" 2>&1
done
# The write study's before/after pairs (#20): its own script, its own
# directory under fixtures/.
bash /repo/scripts/guest-write-study.sh

bcachefs-ref version > "$out/reference-version.txt" 2>&1 || true
rm -rf "$work"
ls -l "$out"
