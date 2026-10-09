#!/usr/bin/env bash
#
# vm-setup.sh -- the fs-linux-test-harness [setup] script. Runs as root
# INSIDE the VM, re-applied by the harness whenever this file changes.
#
# THE ORACLE IS THE REFERENCE USERSPACE TOOL, AT ARM'S LENGTH. It is
# GPL-licensed. It is built here, inside the disposable guest, from the
# upstream release tarball, and it never leaves the guest: nothing in this
# repository links it, copies it or reads its source (docs/clean-room.md).
# Its source tree is deleted as soon as the binary is installed.
# Fixture scripts reach it only through the `bcachefs-ref` wrapper this
# script installs, by role: "the reference formatter", "the reference
# lister".
#
# WHY A CHROOT. The guest is Debian 12, whose userspace-RCU library is too
# old for current releases of the tool (the tool's own install notes say
# so), and the distribution has no current package of it. A Debian 13
# chroot made with debootstrap has every build dependency at a version
# that works, and the Rust compiler it ships meets the tool's minimum.
#
# WHY FUSE, AND NOW A KERNEL MODULE TOO. The formatter populates an image
# from a directory tree itself (`format --source`), and every reading of an
# image is done by the tool's userspace copy of the filesystem. The AGED
# fixtures need a running filesystem to write them, and that is the same
# userspace copy mounted through FUSE (the tool's documented, experimental
# `fusemount`, built with BCACHEFS_FUSE=1). Every aged image must then pass
# the reference checker, so a fault of the FUSE path cannot reach a fixture
# unnoticed. The FUSE mount cannot make reflinks, snapshots, casefolded
# directories or per-inode options, and stalls on a removal followed by a
# sync (#94), so the reference kernel module runs here too (#110): see THE
# KERNEL ORACLE below.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

# The pinned release of the reference tools. Bump deliberately: the
# fixtures record which version made them.
REF_VERSION=1.39.7
# What the build marker records: the version and the build options.
REF_BUILD="$REF_VERSION fuse"
ROOT=/srv/ref-trixie

apt-get update -qq
# build-essential: the in-guest suite (`chore test:vm`, scripts/guest-suite.sh)
# compiles this crate in the guest itself, and its build scripts need a C
# linker; the reference tools are built in the chroot and need none of this.
apt-get install -y -qq debootstrap jq xxd coreutils fuse3 build-essential curl ca-certificates >/dev/null

if [ ! -f "$ROOT/.bootstrapped" ]; then
    rm -rf "$ROOT"
    debootstrap --variant=minbase trixie "$ROOT" http://deb.debian.org/debian >/dev/null
    touch "$ROOT/.bootstrapped"
fi

mount_into_chroot() {
    for d in proc sys dev; do
        mountpoint -q "$ROOT/$d" || mount --rbind "/$d" "$ROOT/$d"
    done
}
mount_into_chroot
cp /etc/resolv.conf "$ROOT/etc/resolv.conf"

if [ "$(cat "$ROOT/.ref-version" 2>/dev/null || true)" != "$REF_BUILD" ]; then
    chroot "$ROOT" apt-get update -qq
    chroot "$ROOT" apt-get install -y -qq --no-install-recommends \
        build-essential pkg-config ca-certificates curl git \
        libaio-dev libblkid-dev libkeyutils-dev liblz4-dev libsodium-dev \
        libunwind-dev liburcu-dev libzstd-dev uuid-dev zlib1g-dev \
        libudev-dev udev systemd-dev libclang-dev clang valgrind rustc cargo bindgen \
        python3 libfuse3-dev fuse3 attr >/dev/null
    chroot "$ROOT" bash -euc "
        rm -rf /build && mkdir -p /build && cd /build
        curl -fsSL -o tools.tar.gz https://github.com/koverstreet/bcachefs-tools/archive/refs/tags/v$REF_VERSION.tar.gz
        tar -xzf tools.tar.gz
        cd bcachefs-tools-$REF_VERSION
        BCACHEFS_FUSE=1 make -j\$(nproc) bcachefs >/build/make.log 2>&1 || { tail -n 60 /build/make.log; exit 1; }
        install -m 0755 bcachefs /usr/local/sbin/bcachefs
        # The binary is all the oracle needs. The unpacked source tree goes
        # at once, so nobody working in this guest (where this crate is also
        # built and tested, scripts/guest-suite.sh) can open it by accident:
        # the clean-room rule is never to read it (docs/clean-room.md).
        cd / && rm -rf /build
    "
    echo "$REF_BUILD" > "$ROOT/.ref-version"
fi

# The one way anything in the guest reaches the reference tool. The share
# is bind-mounted at the same path inside the chroot, so an image path
# means the same thing on both sides.
cat > /usr/local/bin/bcachefs-ref <<WRAP
#!/usr/bin/env bash
set -euo pipefail
ROOT=$ROOT
for d in proc sys dev; do
    mountpoint -q "\$ROOT/\$d" || mount --rbind "/\$d" "\$ROOT/\$d"
done
mkdir -p "\$ROOT/share"
mountpoint -q "\$ROOT/share" || mount --bind /share "\$ROOT/share"
exec chroot "\$ROOT" /usr/local/sbin/bcachefs "\$@"
WRAP
chmod 0755 /usr/local/bin/bcachefs-ref
bcachefs-ref version

# THE KERNEL ORACLE (#110). The reference kernel module is packaged for
# DKMS (apt.bcachefs.org, S10 in docs/clean-room.md) and needs kernel
# headers 6.16 or newer. The harness's box is Debian 12, whose newest kernel
# is 6.12, so the guest is upgraded to Debian 13, whose backports carry a
# newer kernel, and the module is built for that kernel by DKMS, pinned to
# the tools' version. Setup runs on a provisioning boot that the harness
# stops before any run, so every run boots the newest kernel installed,
# this one. The module is built from GPL source inside the guest, and that
# source is deleted as soon as the module is installed, as the tools'
# source tree is above: nobody working in this guest can open it.
KERNEL_BUILD="$REF_VERSION trixie-backports"
if [ "$(cat /etc/ref-kernel-version 2>/dev/null || true)" != "$KERNEL_BUILD" ]; then
    apt_opts=(-y -qq -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold)
    if ! grep -q '^13' /etc/debian_version; then
        for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
            if [ -f "$f" ]; then sed -i 's/\bbookworm\b/trixie/g' "$f"; fi
        done
        apt-get update -qq
        apt-get "${apt_opts[@]}" full-upgrade >/dev/null
    fi
    arch="$(dpkg --print-architecture)"
    echo "deb http://deb.debian.org/debian trixie-backports main" \
        >/etc/apt/sources.list.d/trixie-backports.list
    install -d -m 0755 /etc/apt/keyrings
    curl -fsSL -o /etc/apt/keyrings/apt.bcachefs.org.asc https://apt.bcachefs.org/apt.bcachefs.org.asc
    echo "deb [signed-by=/etc/apt/keyrings/apt.bcachefs.org.asc] https://apt.bcachefs.org/trixie bcachefs-tools-release main" \
        >/etc/apt/sources.list.d/apt.bcachefs.org.list
    apt-get update -qq
    apt-get "${apt_opts[@]}" install -t trixie-backports \
        "linux-image-$arch" "linux-headers-$arch" >/dev/null
    apt-get "${apt_opts[@]}" install "bcachefs-kernel-dkms=1:$REF_VERSION" >/dev/null
    kver="$(find /lib/modules -mindepth 1 -maxdepth 1 -printf '%f\n' | sort -V | tail -n 1)"
    if ! find "/lib/modules/$kver" -name 'bcachefs.ko*' | grep -q .; then
        echo "vm-setup: DKMS built no bcachefs module for $kver" >&2
        dkms status >&2 || true
        exit 1
    fi
    # No rebuild could find its source, and none should be attempted.
    apt-mark hold bcachefs-kernel-dkms "linux-image-$arch" "linux-headers-$arch" >/dev/null
    rm -rf /usr/src/bcachefs-* /var/lib/dkms/bcachefs/*/build /var/lib/dkms/bcachefs/*/source
    # THE NETWORK ACROSS THE REBOOT. The first run of this (CI run
    # 37896786204) built the module, and then the guest never answered SSH
    # on the new kernel: three ten-minute boot timeouts. The likeliest cause
    # is the network interface's name changing with the kernel and systemd,
    # so the old configuration names a device that is gone. The kernel's
    # own name, eth0, is fixed here and configured by DHCP whatever the box
    # used; what the box had is printed first, for the record.
    echo "vm-setup: the network before the reboot:"
    ip -br link || true
    cat /etc/network/interfaces 2>/dev/null || true
    ls /etc/network/interfaces.d /etc/systemd/network /etc/netplan 2>/dev/null || true
    for u in networking systemd-networkd NetworkManager; do
        echo "$u: $(systemctl is-enabled "$u" 2>&1 || true)"
    done
    sed -i -E 's/^(GRUB_CMDLINE_LINUX=")/\1net.ifnames=0 biosdevname=0 /' /etc/default/grub
    update-grub >/dev/null 2>&1
    install -d /etc/systemd/network
    printf '[Match]\nName=eth0 en*\n\n[Network]\nDHCP=yes\n' >/etc/systemd/network/10-guest.network
    systemctl enable systemd-networkd >/dev/null 2>&1
    # And what the new kernel boots with: its initramfs's virtio drivers (no
    # disk or network driver means no boot at all) and the boot menu's
    # default entry.
    echo "vm-setup: virtio modules in the initramfs of $kver:"
    lsinitramfs "/boot/initrd.img-$kver" 2>&1 | grep -oE 'virtio[a-z_]*\.ko' | sort -u | tr '\n' ' ' || true
    echo
    grep -m 3 -E "menuentry |linux\s" /boot/grub/grub.cfg 2>&1 | cut -c1-160 || true
    echo "vm-setup: the reference module is installed for $kver"
    echo "$KERNEL_BUILD" >/etc/ref-kernel-version
fi

# EVERYTHING ON DISK BEFORE THE PROVISIONING BOOT IS STOPPED. Harness
# v0.4.0 stopped that boot without syncing it first if the graceful halt did
# not finish (its #58 fixed that, in v0.4.1), and a whole distribution
# upgrade sits in the page cache when this script ends:
# CI runs 37896786204 and 37901329719 installed the kernel and module, then
# the next boot never answered SSH.
sync
