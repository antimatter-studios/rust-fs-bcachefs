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
# removes 16M files until buckets are reused (#94); on a third it snapshots
# the root subvolume itself (#138); a fourth, a gzip filesystem, it mounts
# and leaves for this crate's tools to write into, then boots again to read
# that back (#141); on a fifth it preallocates a MiB in a nocow file and in
# an ordinary one and writes 4 KiB into each (#158). Its console becomes
# kernel.txt and kernel.json; the reference checker judges both images. A
# stage that fails is recorded, not fatal here:
# tests/oracle_kernel.rs fails on it, naming what is missing. A hang is the
# exception (#500): it is recorded, the script runs to its end, and then it
# fails, naming what hung and, from the guest's console, whose hang it was
# (#514, scripts/kernel-oracle-hang.py).
set -euo pipefail

OUT="${1:?usage: kernel-oracle.sh OUT}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REF_VERSION=1.39.7
work="$here/tmp/kernel-oracle"
rm -rf "$work"
mkdir -p "$work/root" "$OUT"
: >"$OUT/kernel.txt"
record() { echo "$1: $2" >>"$OUT/kernel.txt"; }
# shellcheck source=scripts/kernel-oracle-boot.sh
source "$here/scripts/kernel-oracle-boot.sh"

# EVERY CALL AFTER THE BOOTS HAS A LIMIT (#500). Two fixtures jobs stopped a
# hung first boot at its limit, then printed nothing until the job's own limit
# cancelled them, with a docker process still running. Green run 41579678461
# took 9.5 s for the judge boot and all 15 reference tool calls together, so 60 s
# is far past a working call. Once a call is stopped the rest are not run, which
# is also what keeps the limits inside the job's: a daemon that has stopped
# answering would otherwise cost every call its limit. The worst case after a
# hung boot is 315 s for the boot, 75 s for the call and 45 s each to remove its
# container and the image: 8 minutes. The slowest job measured reaches the boots
# about 33 minutes in, of its 45 (ci.yml).
# The module build took about 100 s in green run 41579678461 (#502), mostly
# network and DKMS. A 300 s limit leaves three times that measurement.
BUILD_LIMIT="${KERNEL_ORACLE_BUILD_LIMIT:-300}"
TOOL_LIMIT="${KERNEL_ORACLE_TOOL_LIMIT:-60}"
CLEANUP_LIMIT="${KERNEL_ORACLE_CLEANUP_LIMIT:-30}"
KILL_AFTER="${KERNEL_ORACLE_KILL_AFTER:-15}"
TOOL_CONTAINER=rust-fs-bcachefs-ko-tool
NOT_RUN="not run: a call before it was stopped at its limit"
STOPPED="stopped at its $TOOL_LIMIT s limit"
stopped=

build_step() {
    local key="$1" limit="$2" output="$3" status=0
    shift 3
    case "$output" in
        -) timeout -k "$KILL_AFTER" "$limit" "$@" || status=$? ;;
        quiet) timeout -k "$KILL_AFTER" "$limit" "$@" >/dev/null 2>&1 || status=$? ;;
        *) timeout -k "$KILL_AFTER" "$limit" "$@" >"$output" || status=$? ;;
    esac
    case "$status" in
        124 | 137)
            record "$key" "stopped at its $limit s limit"
            echo "kernel-oracle: FAILED: the module build ($key) was stopped at its $limit s limit." >&2
            timeout -k "$KILL_AFTER" "$CLEANUP_LIMIT" docker rm -f rust-fs-bcachefs-ko \
                >/dev/null 2>&1 || true
            exit 1
            ;;
    esac
    return "$status"
}

# reference_tool KEY OUTPUT ARGS... runs the reference tool in its container and
# returns the tool's status. A call that gave no answer is recorded under KEY
# and returns 124. Stopping the docker client does not stop the container, whose
# first process ignores SIGTERM unless it installs a handler, so the container is
# removed by name.
reference_tool() {
    local key="$1" output="$2" status=0
    shift 2
    if [ -n "$stopped" ]; then
        record "$key" "$NOT_RUN"
        return 124
    fi
    timeout -k "$KILL_AFTER" "$TOOL_LIMIT" docker run --rm --name "$TOOL_CONTAINER" \
        -v "$work:/work" rust-fs-bcachefs-ko:tools \
        bcachefs "$@" >"$output" 2>&1 || status=$?
    case "$status" in
        124 | 137)
            stopped="the reference tool's $1 ($key)"
            record "$key" "$STOPPED"
            timeout -k "$KILL_AFTER" "$CLEANUP_LIMIT" docker rm -f "$TOOL_CONTAINER" \
                >/dev/null 2>&1 || true
            return 124
            ;;
    esac
    return "$status"
}

# judge_image KEY IMAGE: the reference checker's verdict on IMAGE, in KEY.
judge_image() {
    local status=0
    reference_tool "$1" "$OUT/$2.fsck.txt" fsck -n "/work/$2.img" || status=$?
    case "$status" in
        0) record "$1" clean ;;
        124) ;;
        *) record "$1" "errors ($2.fsck.txt)" ;;
    esac
}

# list_btree BTREE IMAGE: the reference lister's view of BTREE in IMAGE.
list_btree() {
    reference_tool "$2.$1" "$OUT/$2.$1.txt" list -b "$1" "/work/$2.img" || true
}

# write_file KEY PATH: this crate's writer puts written.txt at PATH. It runs on
# the image a hung guest left, ahead of the reference tools, so it has the same
# limit.
write_file() {
    local status=0
    if [ -n "$stopped" ]; then
        record "$1" "$NOT_RUN"
        return
    fi
    timeout -k "$KILL_AFTER" "$TOOL_LIMIT" "$work/fs.bcachefs" "$work/kernel-written.img" \
        put "$2" --from "$work/written.txt" >/dev/null 2>"$work/written.err" || status=$?
    case "$status" in
        0) record "$1" ok ;;
        124 | 137)
            stopped="this crate's writer ($1)"
            record "$1" "$STOPPED"
            ;;
        *) record "$1" "failed ($(head -c 200 "$work/written.err" | tr '\n' ' '))" ;;
    esac
}

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
cp "$here/scripts/prealloc.c" "$work/prealloc.c"
chmod 0755 "$work/root/init"

echo "kernel-oracle: building the module in a Debian 13 container"
build_step build-remove-stale "$CLEANUP_LIMIT" quiet \
    docker rm -f rust-fs-bcachefs-ko "$TOOL_CONTAINER" || true
build_step build-run "$BUILD_LIMIT" - \
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
    # The module is built from its source by DKMS, whose compiler output
    # can quote it: stderr stays in the container, and a failure reports
    # only its status (#153, docs/clean-room.md).
    apt-get install -y -qq --no-install-recommends \
        "bcachefs-kernel-dkms=1:$REF_VERSION" "bcachefs-tools=1:$REF_VERSION" \
        >/dev/null 2>/tmp/dkms-install.err || {
        echo "kernel-oracle: the reference module did not install (apt-get exited $?); its output stays in the container, unprinted" >&2
        exit 1
    }
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
    gcc -static -O2 -o /work/root/bin/prealloc /work/prealloc.c
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
    truncate -s 64M /work/kernel-rootsnap.img
    bcachefs format -q /work/kernel-rootsnap.img
    truncate -s 64M /work/kernel-prealloc.img
    bcachefs format -q /work/kernel-prealloc.img
    truncate -s 64M /work/kernel-written.img
    if bcachefs format -q --compression=gzip /work/kernel-written.img \
        >/work/written.format.txt 2>&1; then
        echo ok >/work/written.format.status
    else
        echo "failed ($(head -c 200 /work/written.format.txt | tr "\n" " "))" \
            >/work/written.format.status
    fi
    chmod -R a+rwX /work
'
build_step build-commit "$TOOL_LIMIT" /dev/null \
    docker commit rust-fs-bcachefs-ko rust-fs-bcachefs-ko:tools
build_step build-remove "$CLEANUP_LIMIT" /dev/null docker rm rust-fs-bcachefs-ko
kver="$(cat "$work/kver")"
echo "kernel-oracle: kernel $kver; modules: $(tr '\n' ' ' <"$work/root/modules.order")"

(cd "$work/root" && find . | cpio -o -H newc --quiet | gzip -1) >"$work/initrd.gz"

echo "kernel-oracle: booting it"
first=0
boot 300 -enable-kvm -machine q35,accel=kvm -cpu host -m 2G -smp 2 \
    -kernel "$work/vmlinuz" -initrd "$work/initrd.gz" \
    -append "console=ttyS0,115200 panic=-1 rdinit=/init" \
    -drive "file=$work/kernel.img,if=virtio,format=raw" \
    -drive "file=$work/kernel-reuse.img,if=virtio,format=raw" \
    -drive "file=$work/kernel-rootsnap.img,if=virtio,format=raw" \
    -drive "file=$work/kernel-written.img,if=virtio,format=raw" \
    -drive "file=$work/kernel-prealloc.img,if=virtio,format=raw" \
    -display none -serial "file:$work/console.log" -serial "file:$work/oracle.log" \
    -no-reboot || first=$?
[ "$first" -eq 0 ] || echo "kernel-oracle: qemu exited $first" >&2
# A guest stopped at its limit (124) or killed after it (137) hung (#194):
# recorded, and the judge boot below is not run over what it left. Its console
# says whose hang it was (#514); that changes what the failure says, not that
# the build fails.
first_failure=
case "$first" in
    124 | 137)
        hang="$(python3 "$here/scripts/kernel-oracle-hang.py" "$work/console.log" "$work/oracle.log")" ||
            hang="its console could not be read"
        first_failure="the first boot was stopped at its 300 s limit: $hang"
        record boot "$first_failure (kernel.console.txt)"
        ;;
esac

# THIS CRATE'S WRITES, JUDGED BY THE MODULE (#141). The fourth image is a
# gzip filesystem the module mounted once, leaving a seed file and the
# state a mounted filesystem has; this crate's tools (chore cli:install)
# now write a compressible file into it, and a second boot, which does
# nothing else, reads it back through the module.
record written-format "$(cat "$work/written.format.status" 2>/dev/null || echo missing)"
writer="${KERNEL_ORACLE_WRITER:-$here/tmp/cli/bin/rust-fs-bcachefs}"
if [ -x "$writer" ]; then
    ln -sf "$writer" "$work/fs.bcachefs"
    # 4096 numbered lines of text, compressible, made without `yes | head`,
    # which pipefail reads as a failure (yes dies of SIGPIPE).
    python3 -c 'print("\n".join(f"a line of text the writer stores compressed with gzip, {i:05d}" for i in range(4096)))' \
        >"$work/written.txt"
    record written-sha256 "$(sha256sum "$work/written.txt" | cut -d' ' -f1)"
    write_file written /written.txt
    # A file in the directory with options (#142): it takes them, as the
    # module's own file there did.
    write_file written-opt /optdir/ours.txt
else
    record written "no writer at $writer: build it with chore cli:install"
fi
case "$first" in
    124 | 137)
        record judge "not booted: the first boot hung, so the image it left is not judged"
        ;;
    *)
        echo "kernel-oracle: booting it again to read what this crate wrote"
        boot 120 -enable-kvm -machine q35,accel=kvm -cpu host -m 1G -smp 1 \
            -kernel "$work/vmlinuz" -initrd "$work/initrd.gz" \
            -append "console=ttyS0,115200 panic=-1 rdinit=/init oracle=judge" \
            -drive "file=$work/kernel-written.img,if=virtio,format=raw" \
            -display none -serial "file:$work/judge-console.log" -serial "file:$work/judge.log" \
            -no-reboot ||
            echo "kernel-oracle: qemu exited $? (judge)" >&2
        ;;
esac

# The init's lines carry `ORACLE `, on the second serial port (#134); a
# manifest line carries `ORACLE M`.
sed -n 's/\r$//; s/^.*ORACLE \([a-z][a-z0-9_.=-]*\): /\1: /p' "$work/oracle.log" >>"$OUT/kernel.txt"
sed -n 's/\r$//; s/^.*ORACLE \(judge\.[a-z0-9_.=-]*\): /\1: /p' "$work/judge.log" >>"$OUT/kernel.txt" 2>/dev/null || true
grep -q '^kernel: ' "$OUT/kernel.txt" || {
    record kernel "the VM printed nothing of the oracle's (kernel.console.txt)"
}
tail -n 200 "$work/console.log" >"$OUT/kernel.console.txt"
if [ -f "$work/judge-console.log" ]; then
    tail -n 200 "$work/judge-console.log" >"$OUT/kernel.judge-console.txt"
fi
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

judge_image fsck kernel
cp --sparse=always "$work/kernel.img" "$OUT/kernel.img"
# The reused bucket (#94): judged by the reference checker like the first.
judge_image reuse-fsck kernel-reuse
cp --sparse=always "$work/kernel-reuse.img" "$OUT/kernel-reuse.img"
# The snapshotted root (#138): judged by the reference checker like the
# others.
judge_image rootsnap-fsck kernel-rootsnap
cp --sparse=always "$work/kernel-rootsnap.img" "$OUT/kernel-rootsnap.img"
# What this crate wrote (#141): judged by the reference checker too.
judge_image written-fsck kernel-written
cp --sparse=always "$work/kernel-written.img" "$OUT/kernel-written.img"
# The preallocated files (#158): judged by the reference checker, and the
# lister's view of their extents, which says what kind of pointer the
# module made for space a file holds but never wrote.
judge_image prealloc-fsck kernel-prealloc
cp --sparse=always "$work/kernel-prealloc.img" "$OUT/kernel-prealloc.img"
for b in extents inodes; do
    list_btree "$b" kernel-prealloc
done
# The lister's view of its inodes (#142): this crate's file beside the
# module's, in a directory with options.
list_btree inodes kernel-written
# Per-file options (#81): what the reference tool read back from each file,
# and its help for the commands that set and read them.
sed -n 's/\r$//; s/^.*ORACLE O\t//p' "$work/oracle.log" >"$OUT/kernel.options.txt"
cp "$work/options-help.txt" "$OUT/kernel.options-help.txt" 2>/dev/null || true
# The reflink (#7) and the snapshot (#12): the reference lister's view of
# the reflink btree, the extents that point into it, and the btrees a
# snapshot touches, the record a reader's layout and visibility rules are
# checked against.
for b in reflink subvolumes snapshots snapshot_trees inodes dirents extents; do
    list_btree "$b" kernel
done
timeout -k "$KILL_AFTER" "$CLEANUP_LIMIT" docker rmi rust-fs-bcachefs-ko:tools >/dev/null 2>&1 || true
echo "kernel-oracle: $(tr '\n' ';' <"$OUT/kernel.txt")"

# A hang fails the build here, and not in tests/oracle_kernel.rs: the job after
# this one never gets fixtures from a build that did not finish (#500).
failure="$first_failure"
[ -z "$stopped" ] || failure="${failure:+$failure; }$stopped was stopped at its $TOOL_LIMIT s limit"
if [ -n "$failure" ]; then
    echo "kernel-oracle: FAILED: $failure. The console logs are tmp/kernel-oracle/*.log, in CI the kernel-oracle-console artifact." >&2
    exit 1
fi
