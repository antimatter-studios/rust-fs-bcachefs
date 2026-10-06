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

manifest() {
    local root="$1" dest="$2"
    python3 - "$root" "$dest" <<'PY'
import hashlib, json, os, stat, sys
root, dest = sys.argv[1], sys.argv[2]
entries = []
for dirpath, dirnames, filenames in os.walk(root):
    dirnames.sort()
    for name in sorted(dirnames) + sorted(filenames):
        full = os.path.join(dirpath, name)
        rel = "/" + os.path.relpath(full, root)
        st = os.lstat(full)
        e = {"path": rel, "mode": st.st_mode & 0o7777}
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

bcachefs-ref version > "$out/reference-version.txt" 2>&1 || true
rm -rf "$work"
ls -l "$out"
