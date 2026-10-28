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
# scripts/vm-setup.sh installs. The aged sets use its FUSE mount; the fresh
# sets are populated by the formatter from a directory.
set -euo pipefail
# One deadline per step, five minutes unless the step says otherwise
# (scripts/guest-watchdog.sh).
owned_watchdog=
[ -n "${WATCHDOG_PID:-}" ] || owned_watchdog=yes
# shellcheck source=scripts/guest-watchdog.sh
source "${BCACHEFS_FIXTURE_WATCHDOG:-/repo/scripts/guest-watchdog.sh}"
[ -z "$owned_watchdog" ] || owned_watchdog="${WATCHDOG_PID:-}"

ROOT="${BCACHEFS_REF_ROOT:-/srv/ref-trixie}"
out="${BCACHEFS_FIXTURE_OUT:-/share/fixtures}"
scratch='' owner='' fuse_pid='' fuse_id='' fuse_attempt=0 tool_pid=''

# Read the kernel table without stat/canonicalization: a dead FUSE endpoint
# returns ENOTCONN to mountpoint/stat while its mount still occupies the path.
fuse_mount_identity() {
    findmnt --kernel --noheadings --raw --nocanonicalize \
        --mountpoint "$ROOT$mnt" --output ID
}

# Keep the first identity for this attempt: later lookups must never adopt a
# replacement. The private mountpoint was checked empty before launch.
fuse_record_identity() {
    [ -n "$fuse_id" ] || fuse_id="$(fuse_mount_identity)" || fuse_id=
}

# Only the child sessions and the private mount started here are stopped.
fuse_kill() {
    local current
    if [ -n "$fuse_pid" ]; then
        kill -KILL -- "-$fuse_pid" "$fuse_pid" 2>/dev/null || true
        for _ in {1..50}; do
            kill -0 "$fuse_pid" 2>/dev/null || break
            sleep 0.1
        done
        if kill -0 "$fuse_pid" 2>/dev/null; then
            echo "fixtures: owned FUSE process $fuse_pid did not stop" >&2
            return 1
        fi
        wait "$fuse_pid" 2>/dev/null || true
        # An owned child can attach after the last readiness lookup, including
        # when a signal interrupts startup. Recheck after it can no longer attach.
        fuse_record_identity
        fuse_pid=
    fi
    current="$(fuse_mount_identity)" || current=
    if [ -n "$current" ]; then
        if [ -z "$fuse_id" ] || [ "$current" != "$fuse_id" ]; then
            echo "fixtures: mount identity changed; refusing to detach $ROOT$mnt" >&2
            return 1
        fi
        timeout --kill-after=1s 5s fusermount3 -uz "$ROOT$mnt" ||
            timeout --kill-after=1s 5s umount -l "$ROOT$mnt" || return 1
        if fuse_mount_identity >/dev/null; then
            echo "fixtures: owned mount $fuse_id was not detached" >&2
            return 1
        fi
    fi
    fuse_id=
}

cleanup() {
    local status=$? unmounted=yes
    trap - EXIT
    trap '' HUP INT TERM
    if [ -n "$tool_pid" ]; then
        # The nested write study needs its TERM trap to release its own root.
        kill -TERM -- "-$tool_pid" "$tool_pid" 2>/dev/null || true
        for _ in {1..50}; do
            kill -0 -- "-$tool_pid" 2>/dev/null || break
            sleep 0.1
        done
        kill -KILL -- "-$tool_pid" "$tool_pid" 2>/dev/null || true
        wait "$tool_pid" 2>/dev/null || true
    fi
    if [ -n "${mnt:-}" ] && ! fuse_kill; then
        echo "fixtures: owned mount is still busy; retained $scratch" >&2
        unmounted=no
        status=1
    fi
    if [ "$unmounted" = yes ] && [ -n "$scratch" ] && [ -f "$scratch/.owner" ] &&
        [ "$(cat "$scratch/.owner")" = "$owner" ]; then
        rm -rf -- "$scratch" || status=1
    fi
    if [ -n "$owned_watchdog" ]; then
        kill "$owned_watchdog" 2>/dev/null || true
        wait "$owned_watchdog" 2>/dev/null || true
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

run_owned() {
    local status=0
    # Explicit stdin preserves heredocs even though this is an async child.
    setsid -- "$@" <&0 &
    tool_pid=$!
    wait "$tool_pid" || status=$?
    tool_pid=
    return "$status"
}
bcachefs-ref() { run_owned bcachefs-ref "$@"; }
cp() { run_owned cp "$@"; }
python3() { run_owned python3 "$@"; }
timeout() { run_owned timeout "$@"; }

# Both host and chroot see this invocation's files, without binding or
# sweeping shared paths. Published fixtures are deliberately outside it.
mkdir -p "$out" "$ROOT/var/tmp"
scratch="$(mktemp -d "$ROOT/var/tmp/fixture-build.XXXXXXXX")"
owner="fixture-build:$$:$scratch"
printf '%s\n' "$owner" >"$scratch/.owner"
work="$scratch/work"
age_dir="${scratch#"$ROOT"}/age"
mnt="${scratch#"$ROOT"}/mnt"
mkdir "$work" "$ROOT$age_dir" "$ROOT$mnt"

# A deterministic source tree: the same bytes on every run, so a fixture
# that changes means the formatter changed, not the input.
make_tree() {
    python3 /repo/scripts/fixture-tree.py "$1"
}

# manifest ROOT DEST [live]: every path under ROOT with its type, mode, size,
# symlink target and SHA-256. `live` is for a mounted filesystem: it also
# records each inode number, link count, owner, 512-byte block count and the
# access, modification and change times (seconds plus nanoseconds) the
# mount reports, and leaves out lost+found, which the filesystem made and
# nobody put there.
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
            e["uid"] = st.st_uid
            e["gid"] = st.st_gid
            e["blocks"] = st.st_blocks
            for t in ("atime", "mtime", "ctime"):
                e[t], e[t + "_nsec"] = divmod(getattr(st, "st_" + t + "_ns"), 10**9)
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
    "labeled|--fs_label=PROPERTIES"
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
    bcachefs-ref format -q $opts --source="${src#"$ROOT"}" "$img" > "$out/$name.format.txt" 2>&1
    manifest "$src" "$out/$name.json"
    bcachefs-ref show-super "$img" > "$out/$name.super.txt" 2>&1
    for b in inodes dirents extents freespace; do
        bcachefs-ref list -b "$b" "$img" > "$out/$name.$b.txt" 2>&1
    done
    if ! bcachefs-ref fsck -n "$img" > "$out/$name.fsck.txt" 2>&1; then
        echo "the reference checker did not pass $name:" >&2
        tail -n 30 "$out/$name.fsck.txt" >&2
        exit 1
    fi
done

# THE DEEP SET (#342): a tree 33 directories deep under /tree, for the
# shared recursive export (tests/cli/test-export-tree.sh). Each level holds
# one directory and one file; /tree/big is 3 MiB and one byte of seeded
# bytes, unique on the image, so a test can damage it by its content; three
# symlinks: to a file, dangling, and one leaving the tree upwards. Its own
# source, so no other set's tree changes.
step "deep (a tree 33 directories deep)"
deep_src="$work/deep-src"
python3 - "$deep_src" <<'PY'
import os, random, sys
root = os.path.join(sys.argv[1], "tree")
os.makedirs(root)
p = root
for i in range(1, 34):
    p = os.path.join(p, f"d{i}")
    os.mkdir(p)
    with open(os.path.join(p, f"f{i}"), "wb") as f:
        f.write(b"file %d\n" % i)
rng = random.Random(342)
with open(os.path.join(root, "big"), "wb") as f:
    f.write(bytes(rng.getrandbits(8) for _ in range(3 * 1024 * 1024 + 1)))
open(os.path.join(root, "empty"), "wb").close()
os.symlink("big", os.path.join(root, "link"))
os.symlink("absent", os.path.join(root, "dangling"))
os.symlink("../..", os.path.join(root, "d1", "up"))
PY
truncate -s 64M "$out/deep.img"
bcachefs-ref format -q --source="${deep_src#"$ROOT"}" "$out/deep.img" > "$out/deep.format.txt" 2>&1
manifest "$deep_src" "$out/deep.json"
if ! bcachefs-ref fsck -n "$out/deep.img" > "$out/deep.fsck.txt" 2>&1; then
    echo "the reference checker did not pass deep:" >&2
    tail -n 30 "$out/deep.fsck.txt" >&2
    exit 1
fi
rm -rf "$deep_src"

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
bcachefs-ref format -q --source="${large_src#"$ROOT"}" "$out/large.img" > "$out/large.format.txt" 2>&1
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
fuse_mount() { # IMAGE OPTIONS LOG
    local status=0 deadline=$((SECONDS + 60))
    fuse_kill || return 1
    fuse_attempt=$((fuse_attempt + 1))
    setsid -- bcachefs-ref fusemount -f -o "$2" "$1" "$mnt" >"$3" 2>&1 &
    fuse_pid=$!
    while [ "$SECONDS" -lt "$deadline" ]; do
        fuse_record_identity
        if ! kill -0 "$fuse_pid" 2>/dev/null; then
            wait "$fuse_pid" || status=$?
            # The empty lookup above may precede both attachment and exit.
            # Finish recording this attempt before discarding its child handle.
            fuse_record_identity
            fuse_pid=
            echo "the reference mount exited: status=$status image=$1" >&2
            break
        fi
        if [ -n "$fuse_id" ] && timeout --kill-after=1s 2s mountpoint -q "$ROOT$mnt"; then
            return 0
        fi
        sleep 0.1
    done
    echo "the reference implementation did not mount $1:" >&2
    # Keep the complete native stderr even after private scratch is removed.
    cp -- "$3" "$out/${scratch##*/}.fuse-$fuse_attempt.stderr.txt" || return 1
    grep -v '^\[<0>\]' "$3" | tail -n 30 >&2
    fuse_kill || return 1
    return 1
}
fuse_unmount() {
    local current
    sync
    current="$(fuse_mount_identity)" || current=
    if [ -z "$fuse_id" ] || [ "$current" != "$fuse_id" ]; then
        echo "fixtures: mount identity changed before graceful unmount" >&2
        return 1
    fi
    timeout --kill-after=1s 5s fusermount3 -u "$ROOT$mnt"
    for _ in $(seq 1 60); do
        if ! kill -0 "$fuse_pid" 2>/dev/null; then
            wait "$fuse_pid" 2>/dev/null || true
            fuse_pid=
            ! fuse_mount_identity >/dev/null || return 1
            fuse_id=
            return 0
        fi
        sleep 1
    done
    echo "the FUSE daemon did not exit" >&2
    exit 1
}

step "aged (mounted and aged by the reference implementation)"
img="$age_dir/aged.img"
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
# The owner, blocks and times are left out of that comparison: they are the
# replayed mount's report, for stat (tests/cli/test-stat-tree.sh), not part
# of what the tree is.
python3 - "$work/aged.mounted.json" "$work/aged.replayed.json" <<'PY'
import json, sys
STAT = {"uid", "gid", "blocks", "atime", "atime_nsec", "mtime", "mtime_nsec", "ctime", "ctime_nsec"}
def tree(es):
    return [{k: v for k, v in e.items() if k not in STAT} for e in es]
before = tree(json.load(open(sys.argv[1]))["entries"])
after = tree([e for e in json.load(open(sys.argv[2]))["entries"]
              if e["path"] != "/late" and not e["path"].startswith("/late/")])
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
for b in inodes dirents extents freespace; do
    bcachefs-ref list -b "$b" "$img" > "$out/aged.$b.txt" 2>&1
done
# The packed key formats of the btrees tests/oracle_formats.rs reads (#82).
for b in inodes dirents extents; do
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
bcachefs-ref format -q --encoded_extent_max=1M --compression=lz4 --source="${src#"$ROOT"}" \
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
img="$age_dir/crc128-overwrite.img"
rm -f "$ROOT$img"
truncate -s 1G "$ROOT$img"
bcachefs-ref format -q --encoded_extent_max=1M "$img" >"$out/crc128-overwrite.format.txt" 2>&1
{
    if fuse_mount "$img" rw,noatime "$work/fuse-crc128.log"; then
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
        if fuse_mount "$img" rw,noatime "$work/fuse-crc128.log"; then
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
img="$age_dir/poison.img"
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
    if fuse_mount "$img" rw,noatime,reconcile_enabled=0 "$work/fuse-poison.log"; then
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
        if fuse_mount "$img" rw,noatime,reconcile_enabled=0 "$work/fuse-poison.log"; then
            cat "$ROOT$mnt/victim" >/dev/null 2>&1 && echo "victim: read" || echo "victim: read failed ($?)"
            cmp -s "$ROOT$mnt/intact" <(yes 'an intact file' | head -n 100) && echo "intact: read" ||
                echo "intact: differs"
            poison_settle read
        fi
        echo "## mount with reconcile on, for 30 seconds"
        if fuse_mount "$img" rw,noatime "$work/fuse-poison.log"; then
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

# EXTENTS WAITING ON BACKGROUND WORK (#420). An extent with a reconcile
# entry has a set key at its own position in reconcile_work and one at its
# backpointer's position in reconcile_work_phys; the reconcile_work checker
# cases (tests/check_oracle.rs) damage a copy of an image that holds them.
# kernel.img held them only when the module had not finished that work
# before it unmounted, which varied from build to build. Here background
# compression gives every extent work, and reconcile is off both while the
# files are written through the mount and while the checker replays the
# journal, so the work is never done. A build whose image still holds no key
# in either btree fails here, by name.
step "reconcile (background compression, written with reconcile off)"
img="$age_dir/reconcile.img"
rm -f "$ROOT$img"
truncate -s 64M "$ROOT$img"
bcachefs-ref format -q --background_compression=lz4 "$img" >"$out/reconcile.format.txt" 2>&1
fuse_mount "$img" rw,noatime,reconcile_enabled=0 "$work/fuse-reconcile.log"
python3 - "$ROOT$mnt" <<'PY'
import os, sys
m = sys.argv[1]
for i in range(4):
    with open(os.path.join(m, "file%d" % i), "wb") as f:
        f.write(b"".join(b"%08d a line that waits to be compressed\n" % j for j in range(4000)))
PY
sync
fuse_kill
bcachefs-ref fsck -y -o reconcile_enabled=0 "$img" >"$out/reconcile.replay.txt" 2>&1 || {
    echo "the reference checker could not replay reconcile:" >&2
    tail -n 30 "$out/reconcile.replay.txt" >&2
    exit 1
}
for b in extents reconcile_work reconcile_work_phys; do
    bcachefs-ref list -b "$b" "$img" >"$out/reconcile.$b.txt" 2>&1
done
for b in reconcile_work reconcile_work_phys; do
    grep -q '^u64s [0-9]* type set ' "$out/reconcile.$b.txt" || {
        echo "reconcile: the reference lister shows no $b key, so no extent is waiting on background work:" >&2
        tail -n 30 "$out/reconcile.$b.txt" >&2
        exit 1
    }
done
if ! bcachefs-ref fsck -n "$img" >"$out/reconcile.fsck.txt" 2>&1; then
    echo "the reference checker did not pass reconcile:" >&2
    tail -n 30 "$out/reconcile.fsck.txt" >&2
    exit 1
fi
cp --sparse=always "$ROOT$img" "$out/reconcile.img"

# Stripe pointers link an extent to an erasure-coding stripe (S1 9.1.3.3),
# which takes several devices; a reader of one device never meets one.
# Tried once on three devices so the lister shows whether, and how, the
# reference writes one. No test reads these images.
echo "== probe: erasure coding on three devices"
ec="$age_dir/ec"
{
    for i in 0 1 2; do
        rm -f "$ROOT$ec-$i.img"
        truncate -s 128M "$ROOT$ec-$i.img"
    done
    bcachefs-ref format -q --erasure_code --replicas=2 "$ec-0.img" "$ec-1.img" "$ec-2.img" ||
        echo "format: exit $?"
    if fuse_mount "$ec-0.img:$ec-1.img:$ec-2.img" rw,noatime "$work/fuse-ec.log"; then
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
for i in 0 1 2; do rm -f "$ROOT$ec-$i.img"; done

# THE REFUSED SETS: what this reader must recognise and refuse with a clear
# error rather than misread. An encrypted filesystem (its master key stored
# unencrypted, so no passphrase is involved), and both members of a
# two-device filesystem. The superblock printer's view of each is recorded.
step "encrypted (--encrypted --no_passphrase)"
truncate -s 64M "$out/encrypted.img"
bcachefs-ref format -q --encrypted --no_passphrase --source="${src#"$ROOT"}" "$out/encrypted.img" \
    > "$out/encrypted.format.txt" 2>&1
bcachefs-ref show-super "$out/encrypted.img" > "$out/encrypted.super.txt" 2>&1
# Every btree of the unsealed image, as the reference lister reads it once
# it has decrypted the nodes (#271): tests/oracle_encrypted.rs reads the
# same keys. A lister that fails is recorded, not fatal here; the test fails
# printing what it said.
for b in extents inodes dirents xattrs alloc quotas stripes reflink subvolumes \
    snapshots lru freespace need_discard backpointers bucket_gens snapshot_trees \
    deleted_inodes logged_ops reconcile_work subvolume_children accounting; do
    bcachefs-ref list -b "$b" "$out/encrypted.img" > "$out/encrypted.$b.txt" 2>&1 ||
        echo "list: exit $?" >> "$out/encrypted.$b.txt"
done
# Its files (#272): the tree it was formatted from, which every file must
# read back as; the reference checker's verdict; and the tree as the
# reference implementation reads it, mounted over a scratch copy so the
# published image is never written. Nothing here fails the build:
# tests/oracle_encrypted.rs fails, printing the record, when the reference
# did not read the image.
step "encrypted: the reference reads its files"
manifest "$src" "$out/encrypted.json"
bcachefs-ref fsck -n "$out/encrypted.img" > "$out/encrypted.fsck.txt" 2>&1 ||
    echo "fsck: exit $?" >> "$out/encrypted.fsck.txt"
img="$age_dir/encrypted.img"
rm -f "$out/encrypted.mounted.json"
{
    rm -f "$ROOT$img"
    cp --sparse=always "$out/encrypted.img" "$ROOT$img"
    if fuse_mount "$img" rw,noatime "$work/fuse-encrypted.log"; then
        manifest "$ROOT$mnt" "$out/encrypted.mounted.json" live
        fuse_kill
    else
        echo "mount: failed"
    fi
    rm -f "$ROOT$img"
} > "$out/encrypted.mounted.txt" 2>&1 || echo "the steps stopped: exit $?" >> "$out/encrypted.mounted.txt"
# The nonce's remaining parts (#275): encrypted.img's extents are all
# uncompressed and start at their data's start, so an extent nonce's
# compression type, uncompressed size and crc offset (S1 9.1.4.1) are not
# seen there. An encrypted image with lz4 on is written through the mount
# (a compressible file and a random one), settled, and 4K of each file
# overwritten, which splits their extents and starts the later parts at an
# offset, as crc128-overwrite does. encrypted-compressed.txt records the
# steps; nothing here fails the build, and
# tests/oracle_encrypted_compressed.rs fails, printing that record, if the
# image does not hold both shapes.
step "encrypted-compressed (--encrypted --compression=lz4, partly overwritten)"
img="$age_dir/encrypted-compressed.img"
rm -f "$out/encrypted-compressed.img" "$out/encrypted-compressed.json"
{
    rm -f "$ROOT$img"
    truncate -s 64M "$ROOT$img"
    bcachefs-ref format -q --encrypted --no_passphrase --compression=lz4 "$img" ||
        echo "format: exit $?"
    if fuse_mount "$img" rw,noatime "$work/fuse-encrypted-compressed.log"; then
        python3 - "$ROOT$mnt" <<'PY' || echo "write: exit $?"
import os, random, sys
with open(os.path.join(sys.argv[1], "text"), "wb") as f:
    f.write(b"".join(b"line %06d of a compressible file\n" % i for i in range(32768)))
with open(os.path.join(sys.argv[1], "random"), "wb") as f:
    f.write(random.Random(275).randbytes(1 << 20))
PY
        sync
        sleep 2
        fuse_kill
        bcachefs-ref fsck -y "$img" >"$out/encrypted-compressed.written.settle.txt" 2>&1 ||
            echo "settle written: exit $?"
        bcachefs-ref list -b extents "$img" >"$out/encrypted-compressed.written.extents.txt" 2>&1 || true
        if fuse_mount "$img" rw,noatime "$work/fuse-encrypted-compressed.log"; then
            python3 - "$ROOT$mnt" <<'PY' || echo "overwrite: exit $?"
import os, sys
for name in ("text", "random"):
    with open(os.path.join(sys.argv[1], name), "r+b") as f:
        f.seek(65536)
        f.write(b"overwritten\n" * 341 + b"end\n")
PY
            sync
            sleep 2
            manifest "$ROOT$mnt" "$out/encrypted-compressed.json" live
            fuse_kill
            bcachefs-ref fsck -y "$img" >"$out/encrypted-compressed.replay.txt" 2>&1 ||
                echo "settle overwritten: exit $?"
            for b in inodes dirents extents; do
                bcachefs-ref list -b "$b" "$img" >"$out/encrypted-compressed.$b.txt" 2>&1 || true
            done
            bcachefs-ref fsck -n "$img" >"$out/encrypted-compressed.fsck.txt" 2>&1 ||
                echo "fsck: exit $?" >>"$out/encrypted-compressed.fsck.txt"
            grep -c 'csum chacha20_poly1305_80 ' "$out/encrypted-compressed.extents.txt" |
                sed 's/$/ encrypted crc entries/'
            grep 'crc[0-9]*:.* offset [1-9]' "$out/encrypted-compressed.extents.txt" ||
                echo "no crc offset above 0"
            cp --sparse=always "$ROOT$img" "$out/encrypted-compressed.img"
        fi
    fi
    rm -f "$ROOT$img"
} >"$out/encrypted-compressed.txt" 2>&1 || echo "the steps stopped: exit $?" >>"$out/encrypted-compressed.txt"
sed 's/^/encrypted-compressed: /' "$out/encrypted-compressed.txt"
# The 128-bit data MAC (#334, part of #276): S1 9.1.4.3 says data MACs
# keep all 128 bits under the wide_macs option instead of the default 80,
# and every other encrypted image carries only the 80-bit MAC. An encrypted
# image formatted --wide_macs from the fixture tree has a random file
# written through the mount, is settled, and is listed and checked.
# encrypted-wide-macs.txt records the steps; nothing here fails the build,
# and tests/oracle_encrypted_wide_macs.rs fails, printing that record, if
# its extents do not carry the 128-bit MAC.
step "encrypted-wide-macs (--encrypted --no_passphrase --wide_macs)"
img="$age_dir/encrypted-wide-macs.img"
rm -f "$out/encrypted-wide-macs.img" "$out/encrypted-wide-macs.json"
{
    rm -f "$ROOT$img"
    truncate -s 64M "$ROOT$img"
    bcachefs-ref format -q --encrypted --no_passphrase --wide_macs --source="${src#"$ROOT"}" "$img" ||
        echo "format: exit $?"
    bcachefs-ref show-super "$img" >"$out/encrypted-wide-macs.super.txt" 2>&1 || true
    if fuse_mount "$img" rw,noatime "$work/fuse-encrypted-wide-macs.log"; then
        python3 - "$ROOT$mnt" <<'PY' || echo "write: exit $?"
import os, random, sys
with open(os.path.join(sys.argv[1], "wide-macs-random"), "wb") as f:
    f.write(random.Random(276).randbytes(1 << 20))
PY
        sync
        sleep 2
        manifest "$ROOT$mnt" "$out/encrypted-wide-macs.json" live
        fuse_kill
        bcachefs-ref fsck -y "$img" >"$out/encrypted-wide-macs.settle.txt" 2>&1 ||
            echo "settle: exit $?"
        for b in inodes dirents extents; do
            bcachefs-ref list -b "$b" "$img" >"$out/encrypted-wide-macs.$b.txt" 2>&1 || true
        done
        bcachefs-ref fsck -n "$img" >"$out/encrypted-wide-macs.fsck.txt" 2>&1 ||
            echo "fsck: exit $?" >>"$out/encrypted-wide-macs.fsck.txt"
        grep -c 'csum chacha20_poly1305_128 ' "$out/encrypted-wide-macs.extents.txt" |
            sed 's/$/ crc entries with the 128-bit MAC/'
        grep -c 'csum chacha20_poly1305_80 ' "$out/encrypted-wide-macs.extents.txt" |
            sed 's/$/ crc entries with the 80-bit MAC/'
        cp --sparse=always "$ROOT$img" "$out/encrypted-wide-macs.img"
    else
        echo "mount: failed"
    fi
    rm -f "$ROOT$img"
} >"$out/encrypted-wide-macs.txt" 2>&1 || echo "the steps stopped: exit $?" >>"$out/encrypted-wide-macs.txt"
sed 's/^/encrypted-wide-macs: /' "$out/encrypted-wide-macs.txt"
# The unclean encrypted image (#337, part of #277): aged-unclean is the
# only image left without a clean shutdown, and its journal is plaintext.
# An encrypted image formatted from the fixture tree has files written
# through the mount with journal reclaim held off, and the daemon is
# killed as it is for aged-unclean, so the newest keys are in encrypted
# journal entries only. The image is copied out unreplayed with the
# journal lister's view, then the checker replays the scratch copy and a
# read-only mount of it records the manifest the replay is compared with.
# encrypted-unclean.txt records the steps; nothing here fails the build,
# and tests/oracle_encrypted_unclean.rs fails, printing that record, if
# the image is not unclean or the reference did not replay it.
step "encrypted-unclean (--encrypted --no_passphrase, left unclean by the reference mount)"
img="$age_dir/encrypted-unclean.img"
rm -f "$out/encrypted-unclean.img" "$out/encrypted-unclean.json" "$out/encrypted-unclean.mounted.json"
{
    rm -f "$ROOT$img"
    truncate -s 64M "$ROOT$img"
    bcachefs-ref format -q --encrypted --no_passphrase --source="${src#"$ROOT"}" "$img" ||
        echo "format: exit $?"
    if fuse_mount "$img" rw,noatime,journal_reclaim_delay=60000 "$work/fuse-encrypted-unclean.log"; then
        python3 - "$ROOT$mnt" <<'PY' || echo "write: exit $?"
import os, random, sys
d = os.path.join(sys.argv[1], "unclean")
os.mkdir(d)
for i in range(8):
    with open(os.path.join(d, f"file-{i}"), "wb") as f:
        f.write(random.Random(277 + i).randbytes(4096 * (i + 1)))
        f.flush()
        os.fsync(f.fileno())
PY
        sync
        sleep 2
        manifest "$ROOT$mnt" "$out/encrypted-unclean.mounted.json" live
        fuse_kill
        cp --sparse=always "$ROOT$img" "$out/encrypted-unclean.img"
        bcachefs-ref show-super "$img" >"$out/encrypted-unclean.super.txt" 2>&1 || true
        bcachefs-ref list_journal -a -H "$img" >"$out/encrypted-unclean.journal-headers.txt" 2>&1 ||
            echo "list_journal headers: exit $?"
        bcachefs-ref list_journal -d -V false "$img" >"$out/encrypted-unclean.journal-dirty.txt" 2>&1 ||
            echo "list_journal dirty keys: exit $?"
        bcachefs-ref fsck -y "$img" >"$out/encrypted-unclean.replay.txt" 2>&1 ||
            echo "replay: exit $?"
        if fuse_mount "$img" ro,noatime "$work/fuse-encrypted-unclean-ro.log"; then
            manifest "$ROOT$mnt" "$out/encrypted-unclean.json" live
            fuse_kill
        else
            echo "mount: failed (replayed, read-only)"
        fi
    else
        echo "mount: failed"
    fi
    rm -f "$ROOT$img"
} >"$out/encrypted-unclean.txt" 2>&1 || echo "the steps stopped: exit $?" >>"$out/encrypted-unclean.txt"
sed 's/^/encrypted-unclean: /' "$out/encrypted-unclean.txt"
# The same tree under a known passphrase (#270), so its master key is
# sealed: the reader tells the two kinds apart, and the passphrase unlock
# (#273) is learned from this image. The passphrase file holds exactly the
# passphrase's bytes, with no trailing newline. Formatted with
# --passphrase_file, the formatter exits 0 having written only its initial
# superblocks and none of the tree (CI run 39021762764; with
# --no_passphrase the same call reports the files it wrote), so the image
# is formatted as encrypted.img is, from the tree with the key unsealed,
# and the passphrase then seals the key: set-passphrase is given it on
# standard input, once and once more to confirm (#327). Every outcome
# goes to encrypted-passphrase.format.txt, with the passphrase commands'
# own help (how they take a passphrase) and the unlock command's report of
# each encrypted image's state; nothing here fails the build but the guard
# below, and tests/oracle_refused.rs fails, printing that record, if the
# formatter failed. The superblock printer will not read this image
# without its passphrase (S3): its report is that refusal, which the test
# takes as the reference's view that the key is sealed.
step "encrypted-passphrase (--encrypted --no_passphrase, then set-passphrase)"
printf '%s' 'correct horse battery staple' > "$out/encrypted-passphrase.passphrase"
rm -f "$out/encrypted-passphrase.img"
truncate -s 64M "$out/encrypted-passphrase.img"
{
    echo "-- format --encrypted --no_passphrase"
    bcachefs-ref format -q --encrypted --no_passphrase \
        --source="${src#"$ROOT"}" "$out/encrypted-passphrase.img" < /dev/null ||
        echo "format: exit $?"
    echo "-- set-passphrase --help"
    bcachefs-ref set-passphrase --help < /dev/null || echo "set-passphrase --help: exit $?"
    echo "-- set-passphrase"
    pass="$(cat "$out/encrypted-passphrase.passphrase")"
    printf '%s\n%s\n' "$pass" "$pass" |
        bcachefs-ref set-passphrase "$out/encrypted-passphrase.img" ||
        echo "set-passphrase: exit $?"
    echo "-- unlock --help"
    bcachefs-ref unlock --help < /dev/null || echo "unlock --help: exit $?"
    for e in encrypted encrypted-passphrase; do
        echo "-- unlock --check $e.img"
        bcachefs-ref unlock --check "$out/$e.img" < /dev/null || echo "unlock --check: exit $?"
    done
} > "$out/encrypted-passphrase.format.txt" 2>&1
sed 's/^/encrypted-passphrase: /' "$out/encrypted-passphrase.format.txt"
bcachefs-ref show-super "$out/encrypted-passphrase.img" < /dev/null \
    > "$out/encrypted-passphrase.super.txt" 2>&1 ||
    echo "show-super: exit $?" >> "$out/encrypted-passphrase.super.txt"
# The guard (#327): a formatter that writes only its initial superblocks
# (4 non-zero 4K blocks, docs/clean-room.md) still exits 0, and a sealed
# image with no tree in it cannot teach the passphrase unlock (#273)
# anything about reading files. The image holds the same tree as
# encrypted.img, so it must hold at least half as many non-zero blocks;
# fewer fails the build, naming both counts.
nonzero_blocks() {
    python3 - "$1" <<'PY'
import sys
zero = bytes(4096)
n = 0
with open(sys.argv[1], "rb") as f:
    while block := f.read(4096):
        n += block != zero[: len(block)]
print(n)
PY
}
sealed_blocks="$(nonzero_blocks "$out/encrypted-passphrase.img")"
unsealed_blocks="$(nonzero_blocks "$out/encrypted.img")"
if [ $((sealed_blocks * 2)) -lt "$unsealed_blocks" ]; then
    echo "fixtures: encrypted-passphrase.img holds $sealed_blocks non-zero 4K blocks" \
        "to encrypted.img's $unsealed_blocks: the formatter wrote its superblocks only" >&2
    exit 1
fi
# Its files, as the reference reads them given the recorded passphrase:
# the unlock command adds the key from the passphrase file to the user
# keyring and then the session keyring, so the mount finds it, and
# the reference mount over a scratch copy, so the published image is never
# written, takes the tree. Nothing here fails the build:
# tests/oracle_encrypted_passphrase.rs fails, printing the record, when the
# reference did not read the source tree from the image.
step "encrypted-passphrase: the reference reads its files"
img="$age_dir/encrypted-passphrase.img"
rm -f "$out/encrypted-passphrase.mounted.json"
{
    rm -f "$ROOT$img"
    cp --sparse=always "$out/encrypted-passphrase.img" "$ROOT$img"
    for k in user session; do
        echo "-- unlock -k $k -f encrypted-passphrase.passphrase"
        bcachefs-ref unlock -k "$k" -f "$out/encrypted-passphrase.passphrase" "$img" < /dev/null ||
            echo "unlock -k $k: exit $?"
    done
    if fuse_mount "$img" ro,noatime "$work/fuse-encrypted-passphrase.log"; then
        manifest "$ROOT$mnt" "$out/encrypted-passphrase.mounted.json" live
        fuse_kill
    else
        echo "mount: failed"
    fi
    rm -f "$ROOT$img"
} > "$out/encrypted-passphrase.mounted.txt" 2>&1 ||
    echo "the steps stopped: exit $?" >> "$out/encrypted-passphrase.mounted.txt"
sed 's/^/encrypted-passphrase: /' "$out/encrypted-passphrase.mounted.txt"
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
        if bcachefs-ref format -q "$opt" --source="${cf_src#"$ROOT"}" "$out/casefold.img" 2>&1; then
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
        cf_img="$age_dir/casefold-mount.img"
        rm -f "$ROOT$cf_img"
        truncate -s 64M "$ROOT$cf_img"
        bcachefs-ref format -q "$cf_opt" "$cf_img"
        if fuse_mount "$cf_img" rw,noatime "$work/fuse-casefold.log"; then
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
run_owned bash "${BCACHEFS_FIXTURE_STUDY:-/repo/scripts/guest-write-study.sh}"

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
probe_img="$age_dir/probe.img"
rm -f "$ROOT$probe_img"
truncate -s 64M "$ROOT$probe_img"
bcachefs-ref format -q "$probe_img" > "$out/probe.format.txt" 2>&1
{
    echo "## mount"
    if fuse_mount "$probe_img" rw,noatime "$work/fuse-probe.log"; then
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
# subvolume or a snapshot, but the reference tool's `kvdb` can show a btree
# key and change it one named field at a time, through the normal
# transactional path, and by
# its own help anticipates fabricated snapshots. First its read-only view
# (the default open, which by its help never writes) of the subvolume,
# snapshot and snapshot-tree keys every image holds: the field names a
# fabricated snapshot is written in. Then one attempt at a second snapshot
# key, on a copy, and what the reference checker says of the result.
kvdb_img="$age_dir/probe-kvdb.img"
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
        timeout 120 bcachefs-ref kvdb --rw "$kvdb_img" -c "$c" >"$work/kvdb-command.txt" 2>&1 || true
        grep -v -E '^(Using|starting|  with|recovering|Journal keys|[a-z_]+\.\.\. done|going read-write|clean shutdown)' "$work/kvdb-command.txt" || true
    done
    echo "## kvdb read-only, after: the keys, and the root inode as each leaf sees it"
    timeout 120 bcachefs-ref kvdb "$kvdb_img" -c "list subvolumes" -c "list snapshots" \
        -c "list snapshot_trees" -c "snapshot 4294967293" -c "get -k inodes 0:4096" \
        -c "snapshot 4294967294" -c "get -k inodes 0:4096" 2>&1 || echo "exit $?"
    echo "## the reference checker on the whole snapshot"
    bcachefs-ref fsck -n "$kvdb_img" >"$work/kvdb-fsck.txt" 2>&1 || true
    tail -n 40 "$work/kvdb-fsck.txt" || true
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
ls -l "$out"
