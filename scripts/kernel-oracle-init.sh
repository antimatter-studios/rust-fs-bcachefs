#!/bin/busybox sh
# shellcheck shell=dash
# kernel-oracle-init.sh -- /init of the reference kernel module's VM (built
# and booted by scripts/kernel-oracle.sh). Loads the module, mounts the
# image on /dev/vda, writes the tree from /tree, a hard link and a reflink,
# prints the manifest as the mount reports it, unmounts and powers off.
# Every line the host reads goes to ttyS1 and starts with `ORACLE `:
# `ORACLE key: value` for kernel.txt;
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
say "done: yes"
sync
poweroff -f
