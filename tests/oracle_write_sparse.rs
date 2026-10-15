//! Truncating a file up, or writing past its end, leaves the range between
//! a hole (#156). The writer
//! rewrites a file whole, and it used to write the range a truncate added
//! as allocated zeros: an empty file truncated to 1 MiB took a MiB of
//! buckets, and a file truncated past the free space was refused. Only the
//! file's last block is rewritten now, its bytes past the old end zeros,
//! and the size set; the rest reads as zeros because nothing is there.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::key_type;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

const MIB: u64 = 1 << 20;

fn scratch(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-write-sparse-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();
    img
}

/// The sectors file `ino`'s extents cover, by the extents btree.
fn extent_sectors(img: &std::path::Path, ino: u64) -> u64 {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .iter()
        .filter(|k| k.pos.inode == ino)
        .filter(|k| matches!(k.key_type, key_type::EXTENT | key_type::INLINE_DATA))
        .map(|k| u64::from(k.size))
        .sum()
}

fn clean(img: &std::path::Path) {
    let r = fs_bcachefs::check::check(&FileDevice::open(img).unwrap()).unwrap();
    assert!(r.clean(), "{:?}", r.problems);
}

#[test]
fn an_empty_file_truncated_up_remains_sparse() {
    let img = scratch("empty");
    let root = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w.create_file(root, b"sparse", b"", 0o644).unwrap();
    drop(w);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.truncate(ino, MIB).unwrap();
    drop(w);

    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let i = fs.inode(ino).unwrap();
    assert_eq!(i.size, MIB, "the size after a truncate to 1 MiB");
    assert_eq!(i.sectors, 0, "an empty file truncated up counts sectors");
    assert!(
        fs.read(ino).unwrap() == vec![0u8; MIB as usize],
        "the file does not read as a MiB of zeros"
    );
    drop(fs);
    assert_eq!(
        extent_sectors(&img, ino),
        0,
        "an empty file truncated up has extents"
    );
    clean(&img);
}

#[test]
fn a_file_truncated_up_keeps_its_data_and_allocates_no_more_than_its_last_block() {
    let img = scratch("data");
    let block = {
        let dev = FileDevice::open(&img).unwrap();
        (u64::from(Superblock::read(&dev).unwrap().block_size) * 512).max(512)
    };
    let data: Vec<u8> = (0..70_001u32).map(|i| (i % 251) as u8 ^ 0x5a).collect();
    let root = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w.create_file(root, b"grown", &data, 0o644).unwrap();
    drop(w);

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.truncate(ino, MIB).unwrap();
    drop(w);

    let mut want = data.clone();
    want.resize(MIB as usize, 0);
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(
        fs.inode(ino).unwrap().size,
        MIB,
        "the size after the truncate"
    );
    assert!(
        fs.read(ino).unwrap() == want,
        "the file does not read as its data and then zeros"
    );
    drop(fs);
    let most = (data.len() as u64).div_ceil(block) * block / 512;
    let got = extent_sectors(&img, ino);
    assert!(
        got <= most,
        "the file's extents cover {got} sectors after a truncate up; its data needs {most}"
    );
    clean(&img);
}

/// A write that starts a block or more past a file's end leaves the gap a
/// hole (#156): the old data's last block and the written range are all
/// that is allocated, and the gap reads as zeros.
#[test]
fn a_write_past_the_end_leaves_the_gap_a_hole() {
    let img = scratch("past-end");
    let block = {
        let dev = FileDevice::open(&img).unwrap();
        (u64::from(Superblock::read(&dev).unwrap().block_size) * 512).max(512)
    };
    let data: Vec<u8> = (0..70_001u32).map(|i| (i % 251) as u8 ^ 0x3c).collect();
    let tail: Vec<u8> = (0..5_000u32).map(|i| (i % 241) as u8 ^ 0xa5).collect();
    let root = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let empty = w.create_file(root, b"empty-past", b"", 0o644).unwrap();
    let grown = w.create_file(root, b"grown-past", &data, 0o644).unwrap();
    drop(w);

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.write_at(empty, MIB, &tail).unwrap();
    w.write_at(grown, MIB + 100, &tail).unwrap();
    drop(w);

    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let mut want_empty = vec![0u8; MIB as usize];
    want_empty.extend_from_slice(&tail);
    let mut want_grown = data.clone();
    want_grown.resize((MIB + 100) as usize, 0);
    want_grown.extend_from_slice(&tail);
    for (ino, want) in [(empty, &want_empty), (grown, &want_grown)] {
        assert_eq!(fs.inode(ino).unwrap().size, want.len() as u64);
        assert!(
            fs.read(ino).unwrap() == *want,
            "inode {ino} does not read as its data, zeros, and the write"
        );
    }
    drop(fs);
    let blocks = |bytes: u64| bytes.div_ceil(block) * block / 512;
    for (ino, most) in [
        (empty, blocks(tail.len() as u64)),
        (
            grown,
            blocks(data.len() as u64) + blocks(100 + tail.len() as u64),
        ),
    ] {
        let got = extent_sectors(&img, ino);
        assert!(
            got <= most,
            "inode {ino}'s extents cover {got} sectors after a write past its end; its \
             data and the write need {most}"
        );
    }
    clean(&img);
}
