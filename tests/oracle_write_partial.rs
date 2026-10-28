//! A write touches only the extents it overlaps (#366). The writer used to
//! read a file whole, free every extent it had and write all of it back,
//! so a 10-byte write into a large file cost the whole file in memory, in
//! I/O and in fresh buckets. Each extent the write does not overlap now
//! keeps its key, and with it its pointer and its bucket.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

const MIB: u64 = 1 << 20;

fn scratch(test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("write-partial").into_image(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();
    img
}

/// File `ino`'s keys in the extents btree, whiteouts left out.
fn extent_keys(img: &std::path::Path, ino: u64) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.pos.inode == ino && k.size > 0)
        .filter(|k| !matches!(k.key_type, key_type::WHITEOUT | key_type::EXTENT_WHITEOUT))
        .collect()
}

fn clean(img: &std::path::Path) {
    let r = fs_bcachefs::check::check(&FileDevice::open(img).unwrap()).unwrap();
    assert!(r.clean(), "{:?}", r.problems);
}

/// The issue's red: a 10-byte write in the middle of a 1 MiB file on
/// `default`. Every extent that does not overlap the written block is
/// unchanged, key, pointer and checksum, and the file reads back as
/// written.
#[test]
fn a_write_into_a_large_file_leaves_the_extents_it_does_not_overlap_as_they_were() {
    let img = scratch("mid");
    let block = {
        let dev = FileDevice::open(&img).unwrap();
        (u64::from(Superblock::read(&dev).unwrap().block_size) * 512).max(512)
    };
    let mut model: Vec<u8> = (0..MIB as u32).map(|i| (i % 251) as u8 ^ 0x5a).collect();
    let root = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w.create_file(root, b"large", &model, 0o644).unwrap();
    drop(w);
    let before = extent_keys(&img, ino);
    assert!(
        before.len() > 2,
        "a 1 MiB file is {} extents; the test needs several",
        before.len()
    );

    let at = MIB / 2 + 3;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.write_at(ino, at, b"0123456789").unwrap();
    drop(w);
    model[at as usize..at as usize + 10].copy_from_slice(b"0123456789");

    let (from, to) = (at - at % block, (at + 10).next_multiple_of(block));
    let after = extent_keys(&img, ino);
    let untouched: Vec<&Bkey> = before
        .iter()
        .filter(|k| k.pos.offset * 512 <= from || k.start_offset() * 512 >= to)
        .collect();
    let rewritten: Vec<&&Bkey> = untouched.iter().filter(|k| !after.contains(k)).collect();
    assert!(
        rewritten.is_empty(),
        "a 10-byte write at byte {at} rewrote {} of the {} extents outside the written \
         block ({from}..{to}), the first at {}",
        rewritten.len(),
        untouched.len(),
        rewritten
            .first()
            .map(|k| k.pos.to_string())
            .unwrap_or_default()
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        fs.read(ino).unwrap() == model,
        "the file does not read back as written"
    );
    drop(fs);
    clean(&img);
}
