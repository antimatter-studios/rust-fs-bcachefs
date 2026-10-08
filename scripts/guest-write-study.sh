#!/usr/bin/env bash
# guest-write-study.sh -- runs as root INSIDE the harness VM, from
# scripts/guest-build-fixtures.sh. The before/after pairs the write path is
# learned from (issue #20).
#
# One small formatted filesystem is settled into a base image; then, for
# each operation, a copy of the base is mounted by the reference
# implementation (its FUSE mount, scripts/vm-setup.sh), the one operation is
# done through the mount, and the image is settled again. Every btree of
# the base and of each result is dumped by the reference lister, so the
# difference between a pair is everything that operation changed on disk:
# the keys a writer has to produce, bookkeeping included.
#
# SETTLING. The pinned release's FUSE daemon aborts when it is unmounted,
# so a mount never ends in a clean shutdown. Each mount is therefore ended
# the same way -- a journal flush is waited for, the daemon is killed --
# and the reference checker replays the journal and shuts the filesystem
# down cleanly (`fsck -y`). It must report no error fixed: a settle that
# repaired something would put the repair, not the operation, in the diff.
#
# Writes into /share/fixtures/write-study:
#   base.img, base.<btree>.txt, base.super.txt
#   <op>.img, <op>.<btree>.txt, <op>.super.txt, <op>.fsck.txt
set -euo pipefail

ROOT=/srv/ref-trixie
out=/share/fixtures/write-study
work=/var/tmp/write-study
mnt=/mnt/study
rm -rf "$out" "$ROOT$work"
mkdir -p "$out" "$ROOT$work" "$ROOT$mnt"

# Every btree the reference lister knows by name (S1's list); one that does
# not exist on this version is reported by the lister and skipped.
BTREES="extents inodes dirents xattrs alloc quotas stripes reflink subvolumes
snapshots lru freespace need_discard backpointers bucket_gens snapshot_trees
deleted_inodes logged_ops reconcile_work subvolume_children accounting"

mount_rw() { # IMAGE
    (bcachefs-ref fusemount -f -o rw,noatime "$1" "$mnt" >"$ROOT$work/fuse.log" 2>&1 &)
    for _ in $(seq 1 60); do
        mountpoint -q "$ROOT$mnt" && return 0
        sleep 1
    done
    echo "write-study: the reference implementation did not mount $1" >&2
    grep -v '^\[<0>\]' "$ROOT$work/fuse.log" | tail -n 20 >&2
    exit 1
}

settle() { # IMAGE NAME
    sync
    sleep 2
    pkill -KILL -f "bcachefs fusemount" || true
    for _ in $(seq 1 30); do
        pgrep -f "bcachefs fusemount" >/dev/null || break
        sleep 1
    done
    fusermount3 -uz "$ROOT$mnt" 2>/dev/null || umount -l "$ROOT$mnt" 2>/dev/null || true
    if ! bcachefs-ref fsck -y "$1" >"$out/$2.settle.txt" 2>&1 ||
        grep -q 'errors fixed\|errors this recovery' "$out/$2.settle.txt"; then
        echo "write-study: settling $2 was not a clean replay:" >&2
        tail -n 20 "$out/$2.settle.txt" >&2
        exit 1
    fi
}

dump() { # IMAGE NAME
    bcachefs-ref show-super "$1" >"$out/$2.super.txt" 2>&1
    for b in $BTREES; do
        bcachefs-ref list -b "$b" "$1" >"$out/$2.$b.txt" 2>&1 || true
    done
    # The packed key format of every node (#82: tests/oracle_formats.rs).
    for b in inodes dirents extents; do
        bcachefs-ref list -b "$b" -m formats "$1" >"$out/$2.$b.formats.txt" 2>&1 || true
    done
    if ! bcachefs-ref fsck -n "$1" >"$out/$2.fsck.txt" 2>&1; then
        echo "write-study: the reference checker did not pass $2" >&2
        tail -n 20 "$out/$2.fsck.txt" >&2
        exit 1
    fi
}

# The base: one directory holding one file, everything else as formatted.
base="$work/base.img"
truncate -s 64M "$ROOT$base"
bcachefs-ref format -q "$base" >/dev/null
mount_rw "$base"
mkdir "$ROOT$mnt/d"
printf 'an existing file\n' >"$ROOT$mnt/d/existing"
settle "$base" base
dump "$base" base
cp --sparse=always "$ROOT$base" "$out/base.img"
# The same base on 4096-byte blocks, for writes on larger blocks (#87):
# small, so a write has room in every leaf.
base4k="$work/base-bs4k.img"
truncate -s 64M "$ROOT$base4k"
bcachefs-ref format -q --block_size=4096 "$base4k" >/dev/null
mount_rw "$base4k"
mkdir "$ROOT$mnt/d"
printf 'an existing file\n' >"$ROOT$mnt/d/existing"
settle "$base4k" base-bs4k
dump "$base4k" base-bs4k
cp --sparse=always "$ROOT$base4k" "$out/base-bs4k.img"

# name | what is done through the mount, with $M the mount point
ops=(
    'create-small|printf "hello\n" > "$M/d/new.txt"'
    'create-empty|: > "$M/d/empty"'
    'create-large|head -c 300000 /dev/zero | tr "\0" "x" > "$M/d/big.bin"'
    'mkdir|mkdir "$M/d/sub"'
    'unlink|rm "$M/d/existing"'
    'rename|mv "$M/d/existing" "$M/d/renamed"'
    'truncate|truncate -s 0 "$M/d/existing"'
    'overwrite|printf "changed\n" > "$M/d/existing"'
)
for entry in "${ops[@]}"; do
    name="${entry%%|*}"
    action="${entry#*|}"
    echo "== write-study: $name"
    img="$work/$name.img"
    cp --sparse=always "$ROOT$base" "$ROOT$img"
    mount_rw "$img"
    M="$ROOT$mnt" bash -euc "$action"
    settle "$img" "$name"
    dump "$img" "$name"
    cp --sparse=always "$ROOT$img" "$out/$name.img"
done

# THE INLINE LIMIT (#79): where the reference stops storing a file inline.
# One file of every size from 1 to 2100 bytes, and from just under to just
# over one and two 4096-byte blocks (4090..4200, 8190..8300; #87), is
# written through the mount, once on a default image and once with
# 4096-byte blocks (the block4k fixture's option), and the lister shows
# which became inline_data and how the extents are cut. /d/grow
# is written at 100 bytes, flushed, then grown to 3000, to show what growing
# past the limit does. sizes.txt is the mount's own `inode size name` for
# each file, so a test can join the lister's keys to sizes without this
# crate's reader.
for variant in 'inline-default|' 'inline-bs4k|--block_size=4096'; do
    name="${variant%%|*}"
    opts="${variant#*|}"
    echo "== write-study: $name"
    img="$work/$name.img"
    truncate -s 128M "$ROOT$img"
    # shellcheck disable=SC2086 # $opts is empty or one option
    bcachefs-ref format -q $opts "$img" >"$out/$name.format.txt" 2>&1
    mount_rw "$img"
    M="$ROOT$mnt"
    mkdir "$M/d"
    head -c 100 /dev/zero | tr '\0' g >"$M/d/grow"
    for n in $(seq 1 2100) $(seq 4090 4200) $(seq 8190 8300); do
        head -c "$n" /dev/zero | tr '\0' x >"$M/d/s$n"
    done
    sync
    sleep 2
    head -c 2900 /dev/zero | tr '\0' g >>"$M/d/grow"
    (cd "$M/d" && stat -c '%i %s %n' grow s*) >"$out/$name.sizes.txt"
    settle "$img" "$name"
    dump "$img" "$name"
    cp --sparse=always "$ROOT$img" "$out/$name.img"
done

# DIRENT COLLISIONS (#78): where an entry goes when its name's hash slot is
# taken. SipHash slots are 63 bits wide, so no collision can be had by
# chance; with --str_hash=crc32c four names of one length collide by
# construction: CRC is linear, so equal-length names whose XOR difference
# has a CRC of 0 collide whatever the seed, init or final XOR
# (tests/oracle_collisions.rs checks the four, and why). They are created in
# order through the mount; then the second is removed, then created again,
# each step settled and dumped, so the lister shows where each went.
COLLIDE="CAAAAAAAAAAAAAAA CBBF@MBKAAAAAAAA COA@GJOCFAAAAAAA CLBGFFLIFAAAAAAA"
echo "== write-study: collide"
img="$work/collide.img"
truncate -s 64M "$ROOT$img"
bcachefs-ref format -q --str_hash=crc32c "$img" >"$out/collide.format.txt" 2>&1
mount_rw "$img"
mkdir "$ROOT$mnt/d"
for n in $COLLIDE plain; do printf '%s\n' "$n" >"$ROOT$mnt/d/$n"; done
settle "$img" collide
dump "$img" collide
cp --sparse=always "$ROOT$img" "$out/collide.img"
for step in 'collide-unlink|rm "$M/d/CBBF@MBKAAAAAAAA"' \
    'collide-recreate|printf "%s\n" CBBF@MBKAAAAAAAA >"$M/d/CBBF@MBKAAAAAAAA"'; do
    name="${step%%|*}"
    echo "== write-study: $name"
    mount_rw "$img"
    M="$ROOT$mnt" bash -euc "${step#*|}"
    settle "$img" "$name"
    dump "$img" "$name"
    cp --sparse=always "$ROOT$img" "$out/$name.img"
done
rm -rf "${ROOT:?}$work"
