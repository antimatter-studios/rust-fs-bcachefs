#!/usr/bin/env bash
# kernel-oracle.sh OUT -- the reference kernel module as an oracle (#110), on
# the Linux host that runs the fixture build (a CI runner with KVM). Writes
# kernel.img, kernel.json, kernel.txt and kernel.fsck.txt into OUT.
#
# WHY NOT THE HARNESS GUEST. The module needs a kernel of 6.16 or newer; the
# harness box is Debian 12. Upgraded to Debian 13 and its backports kernel,
# the guest reset at once after GRUB's "Booting", every time, while the same
# kernel and initramfs booted under KVM loaded straight by QEMU (#110, the
# handoff comment). So the kernel is booted that way, in a VM of its own.
#
# HOW. A Debian 13 container installs the backports kernel, builds the
# reference module for it with DKMS (apt.bcachefs.org, pinned to the tools'
# version; its source is built in the container and never leaves it, S10 in
# docs/clean-room.md), and formats the image with the packaged reference
# formatter. The kernel, the module and what it needs, a static busybox, the
# deterministic tree (scripts/fixture-tree.py) and kernel-oracle-init.sh go
# into an initramfs; QEMU boots it under KVM with the image as a virtio disk.
# The init loads the module, mounts the image, writes the tree, a hard
# link, a reflink (scripts/ficlone.c) and a subvolume with a snapshot of it
# (the reference tool, run in the VM), prints the manifest as the mount
# reports it, unmounts and powers off; on a second image it writes and
# removes 16M files until buckets are reused (#94). Its console becomes
# kernel.txt and kernel.json; the reference checker judges both images. A
# stage that fails is recorded, not fatal here:
# tests/oracle_kernel.rs fails on it, naming what is missing.
set -euo pipefail

OUT="${1:?usage: kernel-oracle.sh OUT}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REF_VERSION=1.39.7
work="$here/tmp/kernel-oracle"
rm -rf "$work"
mkdir -p "$work/root" "$OUT"
: >"$OUT/kernel.txt"
record() { echo "$1: $2" >>"$OUT/kernel.txt"; }

missing=
[ -w /dev/kvm ] || missing="$missing /dev/kvm"
command -v docker >/dev/null || missing="$missing docker"
command -v qemu-system-x86_64 >/dev/null || missing="$missing qemu-system-x86_64"
if [ -n "$missing" ]; then
    record kernel "unavailable: this host lacks$missing (the fixtures job's Linux runner has them)"
    echo "kernel-oracle: skipped on this host, lacking$missing; tests/oracle_kernel.rs will fail" >&2
    exit 0
fi

python3 "$here/scripts/fixture-tree.py" "$work/root/tree"
cp "$here/scripts/kernel-oracle-init.sh" "$work/root/init"
cp "$here/scripts/ficlone.c" "$work/ficlone.c"
chmod 0755 "$work/root/init"

echo "kernel-oracle: building the module in a Debian 13 container"
docker rm -f rust-fs-bcachefs-ko >/dev/null 2>&1 || true
docker run --name rust-fs-bcachefs-ko -v "$work:/work" -e REF_VERSION="$REF_VERSION" \
    debian:trixie bash -euc '
    export DEBIAN_FRONTEND=noninteractive
    echo "deb http://deb.debian.org/debian trixie-backports main" \
        >/etc/apt/sources.list.d/trixie-backports.list
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends ca-certificates curl busybox-static \
        kmod xz-utils zstd cpio gcc libc6-dev >/dev/null
    install -d -m 0755 /etc/apt/keyrings
    curl -fsSL -o /etc/apt/keyrings/apt.bcachefs.org.asc https://apt.bcachefs.org/apt.bcachefs.org.asc
    echo "deb [signed-by=/etc/apt/keyrings/apt.bcachefs.org.asc] https://apt.bcachefs.org/trixie bcachefs-tools-release main" \
        >/etc/apt/sources.list.d/apt.bcachefs.org.list
    apt-get update -qq
    arch="$(dpkg --print-architecture)"
    apt-get install -y -qq --no-install-recommends -t trixie-backports \
        "linux-image-$arch" "linux-headers-$arch" >/dev/null
    apt-get install -y -qq --no-install-recommends \
        "bcachefs-kernel-dkms=1:$REF_VERSION" "bcachefs-tools=1:$REF_VERSION" >/dev/null
    kver="$(ls /lib/modules | sort -V | tail -n 1)"
    echo "$kver" >/work/kver
    cp "/boot/vmlinuz-$kver" /work/vmlinuz
    # The module and every module it, the disk and its bus need, in load
    # order, decompressed (busybox insmod reads plain .ko).
    mkdir -p /work/root/lib/modules /work/root/bin
    : >/work/root/modules.order
    for m in virtio_pci virtio_blk bcachefs; do
        modprobe -S "$kver" --show-depends "$m" | awk "/^insmod/ {print \$2}"
    done | awk "!seen[\$0]++" | while read -r ko; do
        name="$(basename "$ko")"
        case "$name" in
            *.ko.xz) xz -dc "$ko" >"/work/root/lib/modules/${name%.xz}"; name="${name%.xz}" ;;
            *.ko.zst) zstd -qdc "$ko" >"/work/root/lib/modules/${name%.zst}"; name="${name%.zst}" ;;
            *) cp "$ko" "/work/root/lib/modules/$name" ;;
        esac
        echo "$name" >>/work/root/modules.order
    done
    cp /bin/busybox /work/root/bin/busybox
    gcc -static -O2 -o /work/root/bin/ficlone /work/ficlone.c
    # The reference tool, with the libraries it loads, for the subvolume
    # and the snapshot (#12): only it knows their ioctls.
    tool="$(command -v bcachefs)"
    cp "$tool" /work/root/bin/bcachefs
    ldd "$tool" | awk "/=> \// {print \$3} /^\t\/lib/ {print \$1}" | while read -r lib; do
        mkdir -p "/work/root$(dirname "$lib")"
        cp -L "$lib" "/work/root$lib"
    done
    # The reference module source goes with the container; it is never read.
    rm -rf /usr/src/bcachefs-* /var/lib/dkms/bcachefs
    # The option commands as the reference tool describes them (#81).
    { bcachefs set-file-option --help; bcachefs get-file-option --help; } \
        >/work/options-help.txt 2>&1 || true
    truncate -s 64M /work/kernel.img
    bcachefs format -q /work/kernel.img
    truncate -s 64M /work/kernel-reuse.img
    bcachefs format -q /work/kernel-reuse.img
    chmod -R a+rwX /work
'
docker commit rust-fs-bcachefs-ko rust-fs-bcachefs-ko:tools >/dev/null
docker rm rust-fs-bcachefs-ko >/dev/null
kver="$(cat "$work/kver")"
echo "kernel-oracle: kernel $kver; modules: $(tr '\n' ' ' <"$work/root/modules.order")"

(cd "$work/root" && find . | cpio -o -H newc --quiet | gzip -1) >"$work/initrd.gz"

echo "kernel-oracle: booting it"
timeout 300 qemu-system-x86_64 -enable-kvm -machine q35,accel=kvm -cpu host -m 2G -smp 2 \
    -kernel "$work/vmlinuz" -initrd "$work/initrd.gz" \
    -append "console=ttyS0,115200 panic=-1 rdinit=/init" \
    -drive "file=$work/kernel.img,if=virtio,format=raw" \
    -drive "file=$work/kernel-reuse.img,if=virtio,format=raw" \
    -display none -serial "file:$work/console.log" -serial "file:$work/oracle.log" \
    -no-reboot ||
    echo "kernel-oracle: qemu exited $?" >&2

# The init's lines carry `ORACLE `, on the second serial port (#134); a
# manifest line carries `ORACLE M`.
sed -n 's/\r$//; s/^.*ORACLE \([a-z][a-z0-9_.=-]*\): /\1: /p' "$work/oracle.log" >>"$OUT/kernel.txt"
grep -q '^kernel: ' "$OUT/kernel.txt" || {
    record kernel "the VM printed nothing of the oracle's (kernel.console.txt)"
}
tail -n 200 "$work/console.log" >"$OUT/kernel.console.txt"
sed -n 's/\r$//; s/^.*ORACLE M\t//p' "$work/oracle.log" | python3 -c '
import json, sys
entries = []
for line in sys.stdin:
    path, kind, mode, ino, nlink, size, extra = line.rstrip("\n").split("\t")
    e = {"path": path, "mode": int(mode, 8), "ino": int(ino), "nlink": int(nlink)}
    if kind == "symlink":
        e.update(type="symlink", target=extra, size=len(extra))
    elif kind == "dir":
        e["type"] = "dir"
    else:
        e.update(type="file", size=int(size), sha256=extra)
    entries.append(e)
entries.sort(key=lambda e: e["path"])
json.dump({"entries": entries}, sys.stdout, indent=1)
' >"$OUT/kernel.json"

if docker run --rm -v "$work:/work" rust-fs-bcachefs-ko:tools \
    bcachefs fsck -n /work/kernel.img >"$OUT/kernel.fsck.txt" 2>&1; then
    record fsck clean
else
    record fsck "errors (kernel.fsck.txt)"
fi
cp --sparse=always "$work/kernel.img" "$OUT/kernel.img"
# The reused bucket (#94): judged by the reference checker like the first.
if docker run --rm -v "$work:/work" rust-fs-bcachefs-ko:tools \
    bcachefs fsck -n /work/kernel-reuse.img >"$OUT/kernel-reuse.fsck.txt" 2>&1; then
    record reuse-fsck clean
else
    record reuse-fsck "errors (kernel-reuse.fsck.txt)"
fi
cp --sparse=always "$work/kernel-reuse.img" "$OUT/kernel-reuse.img"
# Per-file options (#81): what the reference tool read back from each file,
# and its help for the commands that set and read them.
sed -n 's/\r$//; s/^.*ORACLE O\t//p' "$work/oracle.log" >"$OUT/kernel.options.txt"
cp "$work/options-help.txt" "$OUT/kernel.options-help.txt" 2>/dev/null || true
# The reflink (#7) and the snapshot (#12): the reference lister's view of
# the reflink btree, the extents that point into it, and the btrees a
# snapshot touches, the record a reader's layout and visibility rules are
# checked against.
for b in reflink subvolumes snapshots snapshot_trees inodes dirents extents; do
    docker run --rm -v "$work:/work" rust-fs-bcachefs-ko:tools \
        bcachefs list -b "$b" /work/kernel.img >"$OUT/kernel.$b.txt" 2>&1 || true
done
docker rmi rust-fs-bcachefs-ko:tools >/dev/null 2>&1 || true
echo "kernel-oracle: $(tr '\n' ';' <"$OUT/kernel.txt")"
