//! Preallocated space reads as zeros (#158). S1's metadata version history
//! lists `unwritten_extents`: "Preallocated extents that read as zeros".
//! Which pointer flag marks one, and what makes the reference write one,
//! is observed rather than guessed: the reference kernel module
//! preallocates a MiB with `fallocate` in a file the reference tool marked
//! `nocow` and in an ordinary one, and writes 4 KiB at 64 KiB in each
//! (scripts/kernel-oracle-init.sh). Both read back as that 4 KiB in a MiB
//! of zeros, and the nocow file's preallocated space is held by extents
//! whose pointers carry a flag.

mod common;

use common::{fixture, read_text};
use fs_bcachefs::bkey::key_type;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::extent::DataExtent;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

/// `key: value` lines of kernel.txt.
fn record() -> std::collections::BTreeMap<String, String> {
    read_text("kernel.txt")
        .lines()
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// A MiB of zeros with 4 KiB of `w` at 64 KiB: what scripts/prealloc.c
/// leaves in each file.
fn expected() -> Vec<u8> {
    let mut want = vec![0u8; 1 << 20];
    want[65536..65536 + 4096].fill(b'w');
    want
}

#[test]
fn preallocated_space_reads_as_zeros_around_what_was_written() {
    let r = record();
    let text = read_text("kernel.txt");
    for key in [
        "prealloc-option.nocow",
        "prealloc.nocow",
        "prealloc.cow",
        "prealloc",
    ] {
        assert_eq!(r.get(key).map(String::as_str), Some("ok"), "{key}:\n{text}");
    }
    assert_eq!(
        r.get("prealloc-fsck").map(String::as_str),
        Some("clean"),
        "{text}"
    );

    let img = fixture("kernel-prealloc.img");
    let listed = read_text("kernel-prealloc.extents.txt");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let keys = btree::walk(&dev, &sb, btree_id::EXTENTS).unwrap();
    let want = expected();
    let mut flagged = 0;
    for name in ["nocow", "cow"] {
        let ino = fs.lookup(&format!("/{name}")).unwrap();
        let got = fs
            .read(ino)
            .unwrap_or_else(|e| panic!("/{name}: {e}\nthe lister's view:\n{listed}"));
        assert_eq!(got.len(), want.len(), "/{name}'s size");
        if let Some(at) = got.iter().zip(&want).position(|(a, b)| a != b) {
            panic!(
                "/{name} differs at byte {at}: {:#04x}, where {:#04x} was written\n\
                 the lister's view:\n{listed}",
                got[at], want[at]
            );
        }
        if name == "nocow" {
            flagged += keys
                .iter()
                .filter(|k| k.pos.inode == ino && k.key_type == key_type::EXTENT)
                .filter_map(|k| DataExtent::from_key(k).ok())
                .filter(|e| e.ptr.flags != 0)
                .count();
        }
    }
    assert!(
        flagged > 0,
        "no extent of /nocow carries a flagged pointer, so this fixture does not hold the \
         unwritten extent it is for; the lister's view:\n{listed}"
    );
}
