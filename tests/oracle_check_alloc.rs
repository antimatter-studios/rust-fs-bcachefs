//! What the checker misses in the allocation btrees that the reference
//! checker finds (#163): each test damages a copy of `default` through the
//! writer's raw `insert` and requires the checker to name the damage.
//!
//! - a bucket whose generation in `bucket_gens` is not its alloc key's
//!   (the reference's "incorrect gen in bucket_gens");
//! - a data extent with no backpointer at its device sector ("missing
//!   backpointer").
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

/// The alloc and bucket_gens btrees, and their key types (S8).
const ALLOC: u8 = 4;
const BACKPOINTERS: u8 = 13;
const BUCKET_GENS: u8 = 14;
const ALLOC_V4: u8 = 27;
const BACKPOINTER: u8 = 28;
const BUCKET_GENS_KEY: u8 = 30;

fn scratch(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-check-alloc-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();
    img
}

fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

#[test]
fn a_bucket_whose_generation_bucket_gens_and_its_alloc_key_disagree_on_is_reported() {
    let img = scratch("bucket-gens");
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.clean(),
        "the fixture is damaged: {:?}",
        report.problems
    );

    // A bucket with an alloc key, and its generation there (alloc_v4 byte 12).
    let alloc = keys(&img, ALLOC)
        .into_iter()
        .find(|k| k.key_type == ALLOC_V4 && k.value.len() > 12)
        .expect("the fixture has an alloc key");
    let bucket = alloc.pos.offset;
    let gen = alloc.value[12];
    // The bucket_gens key holding it: 256 buckets a key, one byte each.
    let gens = keys(&img, BUCKET_GENS)
        .into_iter()
        .find(|k| {
            k.key_type == BUCKET_GENS_KEY
                && k.pos.inode == alloc.pos.inode
                && k.pos.offset == bucket >> 8
        })
        .expect("the fixture has a bucket_gens key for the bucket");
    let at = (bucket & 0xff) as usize;
    assert_eq!(gens.value[at], gen, "the fixture's bucket_gens agrees");
    let mut bad = gens.clone();
    bad.value[at] = gen.wrapping_add(1);

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(BUCKET_GENS, vec![bad]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "incorrect_gen_in_bucket_gens"
                && p.detail.contains(&format!("bucket {bucket}"))),
        "bucket {bucket}'s generation, {gen} in its alloc key and {} in bucket_gens, was not \
         reported as incorrect_gen_in_bucket_gens: {:?}",
        gen.wrapping_add(1),
        report.problems
    );
}

#[test]
fn a_data_extent_whose_backpointer_is_gone_is_reported() {
    let img = scratch("backpointer");
    let root = fs_bcachefs::Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w
        .create_file(root, b"data.bin", &vec![0x5a; 64 << 10], 0o644)
        .unwrap();
    drop(w);
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.clean(),
        "the file alone is damage: {:?}",
        report.problems
    );

    // The file's data extent, and the backpointer at its device sector << 16.
    let extent = keys(&img, btree_id::EXTENTS)
        .into_iter()
        .find(|k| k.pos.inode == ino && k.key_type == key_type::EXTENT)
        .expect("the file's data extent");
    let e = fs_bcachefs::extent::DataExtent::from_key(&extent).unwrap();
    let at = Bpos {
        inode: u64::from(e.ptr.dev),
        offset: e.ptr.offset << 16,
        snapshot: 0,
    };
    assert!(
        keys(&img, BACKPOINTERS)
            .iter()
            .any(|k| k.key_type == BACKPOINTER && k.pos == at),
        "the writer wrote no backpointer at {at}"
    );

    let gone = Bkey {
        key_type: key_type::DELETED,
        size: 0,
        version_hi: 0,
        version_lo: 0,
        pos: at,
        value: Vec::new(),
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(BACKPOINTERS, vec![gone]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "missing_backpointer" && p.detail.contains(&extent.pos.to_string())),
        "extent {} with no backpointer at {at} was not reported as missing_backpointer: {:?}",
        extent.pos,
        report.problems
    );
}
