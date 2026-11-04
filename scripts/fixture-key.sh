#!/usr/bin/env bash
# fixture-key.sh [TREE]              print the key of TREE's fixtures (default HEAD)
# fixture-key.sh --manifest [TREE]   print what that key is a hash of
# fixture-key.sh --files             print the files the key reads, one a line
# fixture-key.sh --changed-from BASE print changed=true or changed=false, for HEAD
#
# THE KEY NAMES WHAT A FIXTURES BUILD IS MADE FROM. Two trees with one key make
# their fixtures the same way, so a build made for one is kept for the other
# (scripts/kept-fixtures.sh). A change to anything below gets a new key, so no
# kept build answers it and the run builds fresh:
#
# - the fixture scripts: every script `chore fixtures` runs;
# - the reference pin: the reference tools' version (scripts/vm-setup.sh,
#   scripts/kernel-oracle.sh), the harness's config and its pinned ref;
# - this crate's writer: `chore fixtures` builds the tools and the reference
#   kernel module judges a file they write (scripts/kernel-oracle.sh, #141), so
#   every source file the write verb reaches, and what it is compiled with;
# - the `fixtures` task and the ci.yml job that run all of it;
# - GENERATION, which asks for a new build when nothing else changed.
#
# A checker, reader-only or test change is not here: it reads the same images.
# tests/scripts/fixture-key-covers-its-inputs.sh fails if a script the fixture
# build reaches, or a source file the writer reaches, is missing below.
#
# The key is read from the tree, by blob, so it needs git and nothing else and
# does not depend on what is checked out.
set -euo pipefail

# Raise this to ask for a new build when no input changed: a fresh build differs
# from the kept one (.github/workflows/fixtures-fresh.yml) and that is accepted.
GENERATION=1

FILES=(
    # The fixture scripts.
    scripts/build-fixtures.sh
    scripts/guest-build-fixtures.sh
    scripts/guest-watchdog.sh
    scripts/guest-write-study.sh
    scripts/guest-age.py
    scripts/fixture-tree.py
    scripts/ficlone.c
    scripts/prealloc.c
    scripts/kernel-oracle.sh
    scripts/kernel-oracle-boot.sh
    scripts/kernel-oracle-init.sh
    scripts/kernel-oracle-hang.py
    scripts/cli-install.sh
    # The reference pin.
    scripts/vm-setup.sh
    fs-linux-test-harness.toml
    # This crate's writer, and what it is compiled with.
    Cargo.toml
    Cargo.lock
    rust-toolchain.toml
    src/lib.rs
    src/bkey.rs
    src/btree.rs
    src/compress.rs
    src/crypt.rs
    src/csum.rs
    src/error.rs
    src/extent.rs
    src/fs.rs
    src/inode.rs
    src/journal.rs
    src/siphash.rs
    src/superblock.rs
    src/util.rs
    src/write.rs
    src/write_alloc.rs
    src/write_journal.rs
    src/write_nodes.rs
    src/xattr.rs
    src/cli/main.rs
    src/cli/device.rs
    src/cli/fs.rs
    src/cli/fs_write.rs
    src/cli/import.rs
)

die() {
    echo "fixture-key: $*" >&2
    exit 1
}

# The lines of a two-space-indented block of a YAML file: its `  NAME:` line and
# what is indented under it, up to the next line indented two spaces.
block() {
    git show "$2:$1" | awk -v start="  $3:" '
        $0 == start { inside = 1; print; next }
        inside && /^  [^ ]/ { exit }
        inside && /^[^ ]/ { exit }
        inside { print }'
}

manifest() {
    local tree="$1" file listed text
    git rev-parse --verify --quiet "$tree^{tree}" >/dev/null || die "$tree is not a tree"
    # The tree's own generation, so a pull request's raise reads as a change.
    git show "$tree:scripts/fixture-key.sh" | awk -F= '$1 == "GENERATION" { print "generation " $2; exit }'
    listed="$(git ls-tree "$tree" -- "${FILES[@]}")"
    for file in "${FILES[@]}"; do
        printf '%s\n' "$listed" | awk -F '\t' -v f="$file" '$2 == f { found = 1 } END { exit !found }' ||
            die "$file is not in $tree: the key would miss it"
    done
    printf '%s\n' "$listed"
    text="$(git show "$tree:chores.yml" | awk '$1 == "FS_CORE_REF:" || $1 == "LINUX_HARNESS_REF:"')"
    [ "$(printf '%s\n' "$text" | grep -c .)" = 2 ] || die "chores.yml does not name both sibling pins"
    printf '%s\n' "$text"
    text="$(block chores.yml "$tree" fixtures)"
    [ -n "$text" ] || die "chores.yml has no fixtures task"
    printf '%s\n' "$text"
    text="$(block .github/workflows/ci.yml "$tree" fixtures)"
    [ -n "$text" ] || die "ci.yml has no fixtures job"
    printf '%s\n' "$text"
}

sha256() {
    if command -v sha256sum >/dev/null; then sha256sum; else shasum -a 256; fi | cut -d' ' -f1
}

cd "$(dirname "${BASH_SOURCE[0]}")/.."
case "${1:-}" in
    --files) printf '%s\n' "${FILES[@]}" ;;
    --manifest) manifest "${2:-HEAD}" ;;
    --changed-from)
        # A base that lacks an input cannot have this tree's key: the input is new.
        head="$(manifest HEAD)"
        if base="$(manifest "${2:?--changed-from needs a base}" 2>/dev/null)" && [ "$base" = "$head" ]; then
            echo "changed=false"
        else
            echo "changed=true"
        fi
        ;;
    *)
        # Through a file: a failed manifest fails here, which a pipe would hide.
        out="$(manifest "${1:-HEAD}")"
        printf '%s\n' "$out" | sha256
        ;;
esac
