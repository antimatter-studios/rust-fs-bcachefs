#!/usr/bin/env bash
#
# vm-setup.sh -- the fs-linux-test-harness [setup] script. Runs as root
# INSIDE the VM, re-applied by the harness whenever this file changes.
#
# THE ORACLE IS THE REFERENCE USERSPACE TOOL, AT ARM'S LENGTH. It is
# GPL-licensed. It is built here, inside the disposable guest, from the
# upstream release tarball, and it never leaves the guest: nothing in this
# repository links it, copies it or reads its source (docs/clean-room.md).
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
# WHY NO KERNEL MODULE. The formatter populates an image from a directory
# tree itself (`format --source`), and every reading of an image is done by
# the tool's userspace copy of the filesystem. The AGED fixtures need a
# running filesystem to write them, and that is the same userspace copy
# mounted through FUSE (the tool's documented, experimental `fusemount`,
# built with BCACHEFS_FUSE=1): no guest kernel this harness boots carries
# bcachefs, which is maintained out of mainline. Every aged image must
# then pass the reference checker, so a fault of the FUSE path cannot
# reach a fixture unnoticed.
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
apt-get install -y -qq debootstrap jq xxd coreutils fuse3 build-essential >/dev/null

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
