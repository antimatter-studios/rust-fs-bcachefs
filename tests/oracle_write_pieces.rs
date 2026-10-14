//! A write into a file whose data the reference split (#169). On the aged
//! image the reference overwrote the middle of /over/middle.bin, leaving
//! the file held by pieces of checksummed regions: extents whose crc entry
//! covers more than they do. The reference counts such a piece's live
//! sectors in its bucket, not its region's (bucket 105 holds pieces of 111
//! and 120 sectors of one 256-sector region and counts 231), and keeps its
//! backpointer at the region's sector << 16 plus the piece's offset in it.
//! Rewriting the file frees every piece: each bucket loses the pieces' live
//! sectors, a bucket left with none is emptied, their backpointers go, and
//! this checker stays clean.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::extent::DataExtent;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use std::collections::BTreeMap;

/// The alloc and backpointers btrees, their key types, and where an
/// alloc_v4 key keeps its data type and dirty sectors (docs/clean-room.md,
/// "Allocating space").
const ALLOC: u8 = 4;
const BACKPOINTERS: u8 = 13;
const ALLOC_V4: u8 = 27;
const BACKPOINTER: u8 = 28;
const DATA_TYPE: usize = 14;
const DATA_NEED_DISCARD: u8 = 9;

fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

/// Each bucket's data type and dirty sectors.
fn buckets(img: &std::path::Path) -> BTreeMap<u64, (u8, u64)> {
    keys(img, ALLOC)
        .into_iter()
        .filter(|k| k.key_type == ALLOC_V4 && k.pos.inode == 0)
        .map(|k| {
            let dirty = u32::from_le_bytes(k.value[16..20].try_into().unwrap());
            (k.pos.offset, (k.value[DATA_TYPE], u64::from(dirty)))
        })
        .collect()
}

#[test]
fn a_file_held_by_pieces_of_checksummed_regions_is_rewritten() {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-write-pieces-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("aged.img");
    std::fs::copy(fixture("aged.img"), &img).unwrap();

    let (ino, mut want) = {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        let ino = fs.lookup("/over/middle.bin").unwrap();
        (ino, fs.read(ino).unwrap())
    };
    let bucket_size = {
        let dev = FileDevice::open(&img).unwrap();
        u64::from(Superblock::read(&dev).unwrap().members().unwrap()[0].bucket_size)
    };
    // The file's extents: by bucket, the live sectors they hold, and where
    // their backpointers are.
    let extents: Vec<DataExtent> = keys(&img, btree_id::EXTENTS)
        .iter()
        .filter(|k| k.pos.inode == ino && k.key_type == key_type::EXTENT)
        .map(|k| DataExtent::from_key(k).unwrap())
        .collect();
    let pieces = extents
        .iter()
        .filter(|e| {
            e.crc
                .is_some_and(|c| c.offset != 0 || u64::from(c.uncompressed_size) != e.len)
        })
        .count();
    assert!(
        pieces > 0,
        "/over/middle.bin is held by no piece of a checksummed region, so this fixture does not \
         hold what the test is for"
    );
    let mut held: BTreeMap<u64, u64> = BTreeMap::new();
    for e in &extents {
        *held.entry(e.ptr.offset / bucket_size).or_default() += e.len;
    }
    let backpointers: Vec<u64> = extents
        .iter()
        .map(|e| e.ptr.offset << 16 | u64::from(e.crc.map_or(0, |c| c.offset)))
        .collect();
    let before = buckets(&img);
    for (b, n) in &held {
        assert!(
            before.get(b).is_some_and(|&(_, dirty)| dirty >= *n),
            "bucket {b} counts {:?} where the file's extents hold {n} live sectors in it",
            before.get(b)
        );
    }
    for bp in &backpointers {
        assert!(
            keys(&img, BACKPOINTERS)
                .iter()
                .any(|k| k.key_type == BACKPOINTER && k.pos.inode == 0 && k.pos.offset == *bp),
            "the reference left no backpointer at 0:{bp}"
        );
    }

    let at = 125_207;
    want[at..at + 16].copy_from_slice(b"sixteen bytes in");
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.write_at(ino, at as u64, b"sixteen bytes in")
        .unwrap_or_else(|e| panic!("writing into /over/middle.bin: {e}"));
    drop(w);

    let got = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .read(ino)
        .unwrap();
    assert!(got == want, "/over/middle.bin does not read as written");
    let report = fs_bcachefs::check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(report.clean(), "{:?}", report.problems);
    let after = buckets(&img);
    for (b, n) in &held {
        let (_, dirty) = before[b];
        if dirty == *n {
            assert_eq!(
                after.get(b).map(|&(t, d)| (t, d)),
                Some((DATA_NEED_DISCARD, 0)),
                "bucket {b} held only the file's {n} sectors and was not emptied"
            );
        } else {
            assert_eq!(
                after.get(b).map(|&(_, d)| d),
                Some(dirty - n),
                "bucket {b} counted {dirty} sectors, {n} of them the file's"
            );
        }
    }
    let left = keys(&img, BACKPOINTERS);
    for bp in &backpointers {
        assert!(
            !left
                .iter()
                .any(|k| k.key_type == BACKPOINTER && k.pos.inode == 0 && k.pos.offset == *bp),
            "the backpointer at 0:{bp} outlived its extent"
        );
    }
}
