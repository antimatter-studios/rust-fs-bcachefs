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
# WHY NO KERNEL. Nothing here mounts. The formatter populates an image
# from a directory tree itself (`format --source`), and every reading of
# an image is done by the tool's userspace copy of the filesystem. The
# guest kernel does not need bcachefs at all.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

# The pinned release of the reference tools. Bump deliberately: the
# fixtures record which version made them.
REF_VERSION=1.39.7
ROOT=/srv/ref-trixie

apt-get update -qq
apt-get install -y -qq debootstrap jq xxd coreutils >/dev/null

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

if [ "$(cat "$ROOT/.ref-version" 2>/dev/null || true)" != "$REF_VERSION" ]; then
    chroot "$ROOT" apt-get update -qq
    chroot "$ROOT" apt-get install -y -qq --no-install-recommends \
        build-essential pkg-config ca-certificates curl git \
        libaio-dev libblkid-dev libkeyutils-dev liblz4-dev libsodium-dev \
        libunwind-dev liburcu-dev libzstd-dev uuid-dev zlib1g-dev \
        libudev-dev libclang-dev clang valgrind rustc cargo bindgen \
        python3 >/dev/null
    chroot "$ROOT" bash -euc "
        rm -rf /build && mkdir -p /build && cd /build
        curl -fsSL -o tools.tar.gz https://github.com/koverstreet/bcachefs-tools/archive/refs/tags/v$REF_VERSION.tar.gz
        tar -xzf tools.tar.gz
        cd bcachefs-tools-$REF_VERSION
        make -j\$(nproc) bcachefs >/build/make.log 2>&1 || { tail -n 60 /build/make.log; exit 1; }
        install -m 0755 bcachefs /usr/local/sbin/bcachefs
    "
    echo "$REF_VERSION" > "$ROOT/.ref-version"
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
