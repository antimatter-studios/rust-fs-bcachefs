#!/bin/busybox sh
# shellcheck shell=dash
# kernel-oracle-init.sh -- /init of the reference kernel module's VM (built
# and booted by scripts/kernel-oracle.sh). Loads the module, mounts the
# image on /dev/vda, writes the tree from /tree, a hard link, a reflink, a
# subvolume and a snapshot of it, prints the manifest as the mount reports
# it, unmounts and powers off. Every line the host reads goes to ttyS1 and
# starts with `ORACLE `: `ORACLE key: value` for kernel.txt;
# `ORACLE M<TAB>path<TAB>type<TAB>mode<TAB>ino<TAB>nlink<TAB>size<TAB>extra`
# for the manifest (extra: a file's sha256, a symlink's target).
/bin/busybox --install -s /bin
mkdir -p /proc /sys /dev /mnt /tmp
mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
# The record goes to the second serial port, which nothing else writes:
# the kernel prints its messages on the console, and one once landed in
# the middle of a manifest line (#134).
exec 3>/dev/ttyS1
say() { echo "ORACLE $*" >&3; }

say "kernel: $(uname -r)"
failed=
for m in $(cat /modules.order); do
    insmod "/lib/modules/$m" 2>/dev/null || failed="$failed $m"
done
if grep -qw bcachefs /proc/filesystems; then
    say "module: loaded"
else
    say "module: not loaded (insmod failed:${failed:- none})"
fi
for _ in 1 2 3 4 5 6 7 8 9 10; do [ -b /dev/vda ] && break; sleep 1; done

if mount -t bcachefs -o noatime /dev/vda /mnt 2>/tmp/mount.err; then
    say "mount: ok"
    cp -a /tree/. /mnt/
    ln /mnt/hello.txt /mnt/dir/hello-again.txt
    # A reflink (#7): the clone shares big/random.bin's data through the
    # reflink btree, and both files then read through a reflink_p.
    if ficlone /mnt/big/random.bin /mnt/big/random-clone.bin 2>/tmp/ficlone.err; then
        say "reflink: ok"
    else
        say "reflink: failed ($(head -c 200 /tmp/ficlone.err))"
    fi
    # A subvolume and a snapshot of it (#12), by the reference tool: the
    # snapshot keeps the files as they were, the subvolume goes on to
    # change one, remove one and add one.
    if bcachefs subvolume create /mnt/sv 2>/tmp/snap.err; then
        echo "kept" >/mnt/sv/kept.txt
        echo "before the snapshot" >/mnt/sv/changed.txt
        echo "removed after the snapshot" >/mnt/sv/gone.txt
        mkdir /mnt/sv/inner
        echo "inner" >/mnt/sv/inner/file.txt
        sync
        if bcachefs subvolume snapshot /mnt/sv /mnt/snap 2>>/tmp/snap.err; then
            echo "after the snapshot, longer than before" >/mnt/sv/changed.txt
            rm /mnt/sv/gone.txt
            echo "new" >/mnt/sv/new.txt
            say "snapshot: ok"
        else
            say "snapshot: failed ($(head -c 200 /tmp/snap.err))"
        fi
    else
        say "snapshot: failed ($(head -c 200 /tmp/snap.err))"
    fi
    # Per-file options (#81), by the reference tool: each set on an empty
    # file, which is then written; and one set on a directory, whose new
    # file inherits it. Each file's options as the tool reads them back go
    # to the record as `ORACLE O<TAB>path<TAB>line`.
    mkdir /mnt/opts /mnt/opts/dir
    for spec in compression=lz4 compression=gzip compression=zstd \
        background_compression=zstd data_checksum=none data_checksum=crc64 \
        data_checksum=xxhash data_replicas=1; do
        f="/mnt/opts/${spec%%=*}-${spec#*=}"
        : >"$f"
        if bcachefs set-file-option "--$spec" "$f" 2>/tmp/opt.err; then
            say "option.$spec: ok"
        else
            say "option.$spec: failed ($(head -c 200 /tmp/opt.err | tr '\n' ' '))"
        fi
        yes "a line of text for $spec" | head -c 131072 >>"$f"
    done
    if bcachefs set-file-option --compression=zstd /mnt/opts/dir 2>/tmp/opt.err; then
        say "option.dir.compression=zstd: ok"
    else
        say "option.dir.compression=zstd: failed ($(head -c 200 /tmp/opt.err | tr '\n' ' '))"
    fi
    yes "inherited" | head -c 131072 >/mnt/opts/dir/inherited
    sync
    for f in /mnt/opts/* /mnt/opts/dir/inherited; do
        bcachefs get-file-option "$f" 2>&1 | while read -r line; do
            say "$(printf 'O\t%s\t%s' "${f#/mnt}" "$line")"
        done
    done
    sync
    cd /mnt || exit 1
    find . -mindepth 1 | sort | while read -r p; do
        case "$p" in ./lost+found | ./lost+found/*) continue ;; esac
        path="${p#.}"
        # shellcheck disable=SC2046 # five numbers, split on purpose
        set -- $(stat -c '%f %a %i %h %s' "$p")
        raw=$((0x$1))
        if [ $((raw & 0170000)) -eq $((0120000)) ]; then
            printf 'ORACLE M\t%s\tsymlink\t%s\t%s\t%s\t0\t%s\n' "$path" "$2" "$3" "$4" "$(readlink "$p")"
        elif [ -d "$p" ]; then
            printf 'ORACLE M\t%s\tdir\t%s\t%s\t%s\t0\t-\n' "$path" "$2" "$3" "$4"
        else
            sum="$(sha256sum "$p" | cut -d' ' -f1)"
            printf 'ORACLE M\t%s\tfile\t%s\t%s\t%s\t%s\t%s\n' "$path" "$2" "$3" "$4" "$5" "$sum"
        fi
    done >&3
    cd /
    if umount /mnt; then say "unmount: ok"; else say "unmount: failed"; fi
else
    say "mount: failed ($(head -c 200 /tmp/mount.err))"
fi
# A REUSED BUCKET (#94), on the second disk: a 16M file written, synced,
# removed, synced and given a few seconds for the module's background work
# to free its buckets, eight times over, so 128M pass through a 64M image
# and the later files can only land in buckets freed by the earlier ones.
# The last file stays.
mkdir -p /reuse
if [ -b /dev/vdb ] && mount -t bcachefs -o noatime /dev/vdb /reuse 2>/tmp/reuse.err; then
    for i in 1 2 3 4 5 6 7 8; do
        if head -c 16777216 /dev/zero | tr '\000' r >"/reuse/f$i" 2>/tmp/reuse.err; then
            sync
            [ "$i" -eq 8 ] || { rm "/reuse/f$i"; sync; sleep 3; }
        else
            say "reuse: write $i failed ($(head -c 200 /tmp/reuse.err))"
            break
        fi
    done
    if umount /reuse; then say "reuse: ok"; else say "reuse: unmount failed"; fi
else
    say "reuse: mount failed ($(head -c 200 /tmp/reuse.err))"
fi
# A SNAPSHOTTED ROOT (#138), on the third disk: a file written, the root
# subvolume snapshotted into /snap, then the file changed and another added,
# so the root and its snapshot each hold keys the other does not see.
mkdir -p /rootsnap
if [ -b /dev/vdc ] && mount -t bcachefs -o noatime /dev/vdc /rootsnap 2>/tmp/rootsnap.err; then
    echo "before the snapshot" >/rootsnap/kept.txt
    sync
    if bcachefs subvolume snapshot /rootsnap /rootsnap/snap 2>/tmp/rootsnap.err; then
        echo "after the snapshot" >/rootsnap/kept.txt
        echo "new" >/rootsnap/new.txt
        say "rootsnap: ok"
    else
        say "rootsnap: failed ($(head -c 200 /tmp/rootsnap.err))"
    fi
    umount /rootsnap || say "rootsnap: unmount failed"
else
    say "rootsnap: mount failed ($(head -c 200 /tmp/rootsnap.err))"
fi
say "done: yes"
sync
poweroff -f
