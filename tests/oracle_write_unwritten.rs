//! A write keeps preallocated space reading as zeros (#207). The writer
//! rewrites a file whole from what it reads of it, and an unwritten
//! extent's sectors are not the file's data: the file reads as zeros there
//! (#158), whatever the device holds. Here the device under `/nocow`'s
//! unwritten extents is filled with a pattern, as a reused bucket would
//! hold a deleted file's bytes, and one byte is written at the start of the
//! file: everything else must read back as it did.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::key_type;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::extent::DataExtent;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use std::io::{Seek, SeekFrom, Write};

/// What the reference left in `/nocow` (tests/oracle_unwritten.rs): a MiB
/// of zeros with 4 KiB of `w` at 64 KiB.
fn expected() -> Vec<u8> {
    let mut want = vec![0u8; 1 << 20];
    want[65536..65536 + 4096].fill(b'w');
    want
}

#[test]
fn a_write_to_a_file_with_preallocated_space_keeps_that_space_zeros() {
    let dir = common::ScratchDir::new("write-unwritten");
    let img = dir.join("kernel-prealloc.img");
    std::fs::copy(fixture("kernel-prealloc.img"), &img).unwrap();

    let ino = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/nocow")
        .unwrap();
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let unwritten: Vec<DataExtent> = btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .iter()
        .filter(|k| k.pos.inode == ino && k.key_type == key_type::EXTENT)
        .filter_map(|k| DataExtent::from_key(k).ok())
        .filter(|e| e.ptr.is_unwritten())
        .collect();
    drop(dev);
    assert!(
        !unwritten.is_empty(),
        "/nocow has no unwritten extent, so this fixture does not hold what the test is for"
    );

    // The device under every unwritten extent holds a pattern, not zeros.
    let mut f = std::fs::OpenOptions::new().write(true).open(&img).unwrap();
    for e in &unwritten {
        f.seek(SeekFrom::Start(e.ptr.offset * 512)).unwrap();
        f.write_all(&vec![0xa5; (e.len * 512) as usize]).unwrap();
    }
    drop(f);
    let before = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .read(ino)
        .unwrap();
    assert!(
        before == expected(),
        "the reader does not read /nocow's preallocated space as zeros over the pattern"
    );

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.write_at(ino, 0, b"x").unwrap();
    drop(w);

    let mut want = expected();
    want[0] = b'x';
    let got = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .read(ino)
        .unwrap();
    assert_eq!(got.len(), want.len(), "/nocow's size after the write");
    if let Some(at) = got.iter().zip(&want).position(|(a, b)| a != b) {
        panic!(
            "/nocow differs at byte {at} after a one-byte write at 0: {:#04x}, where {:#04x} \
             was there before (the {} unwritten extents' device sectors hold 0xa5)",
            got[at],
            want[at],
            unwritten.len()
        );
    }
}
