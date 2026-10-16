//! What the checker misses in the allocation btrees that the reference
//! checker finds (#163): each test damages a copy of `default` through the
//! writer's raw `insert` and requires the checker to name the damage.
//!
//! - a bucket whose generation in `bucket_gens` is not its alloc key's
//!   (the reference's "incorrect gen in bucket_gens");
//! - a data extent with no backpointer at its device sector ("missing
//!   backpointer"), a piece of a checksummed region included;
//! - a bucket holding a data extent whose alloc key names another data
//!   type than user;
//! - a partly filled bucket with no fragmentation lru entry, and an lru
//!   entry no bucket's fill matches (the reference's "missing fragmentation
//!   lru entry" and "incorrect lru entry");
//! - a bucket waiting for discard with no need_discard key, and a
//!   need_discard key no such bucket gives;
//! - the device's accounting of a data type, out of step with what its
//!   alloc keys add up to (the reference's "accounting mismatch").
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
const LRU: u8 = 10;
const NEED_DISCARD: u8 = 12;
const ACCOUNTING: u8 = 20;
const ACCOUNTING_KEY: u8 = 34;
const BACKPOINTERS: u8 = 13;
const BUCKET_GENS: u8 = 14;
const SET: u8 = 25;
const ALLOC_V4: u8 = 27;
const BACKPOINTER: u8 = 28;
const BUCKET_GENS_KEY: u8 = 30;
/// An alloc_v4 key's data type: byte 6 of its second word; 3 btree, 4 user
/// (docs/clean-room.md, "Allocating space").
const ALLOC_DATA_TYPE: usize = 14;
const DATA_BTREE: u8 = 3;
const DATA_USER: u8 = 4;
const DATA_NEED_DISCARD: u8 = 9;

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

#[test]
fn a_data_bucket_whose_alloc_key_names_another_data_type_is_reported() {
    let img = scratch("data-type");
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

    // The file's data extent, its bucket, and that bucket's alloc key.
    let extent = keys(&img, btree_id::EXTENTS)
        .into_iter()
        .find(|k| k.pos.inode == ino && k.key_type == key_type::EXTENT)
        .expect("the file's data extent");
    let e = fs_bcachefs::extent::DataExtent::from_key(&extent).unwrap();
    let dev = FileDevice::open(&img).unwrap();
    let bucket_size = u64::from(
        Superblock::read(&dev).unwrap().members().unwrap()[usize::from(e.ptr.dev)].bucket_size,
    );
    drop(dev);
    let bucket = e.ptr.offset / bucket_size;
    let alloc = keys(&img, ALLOC)
        .into_iter()
        .find(|k| {
            k.key_type == ALLOC_V4 && k.pos.inode == u64::from(e.ptr.dev) && k.pos.offset == bucket
        })
        .expect("the file's bucket has an alloc key");
    assert_eq!(
        alloc.value[ALLOC_DATA_TYPE], DATA_USER,
        "the writer gave the file's bucket another data type than user"
    );
    let mut bad = alloc.clone();
    bad.value[ALLOC_DATA_TYPE] = DATA_BTREE;

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(ALLOC, vec![bad]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "alloc_key_data_type_wrong"
                && p.detail.contains(&format!("bucket {bucket}"))),
        "bucket {bucket}, holding extent {}, with data type btree in its alloc key was not \
         reported as alloc_key_data_type_wrong: {:?}",
        extent.pos,
        report.problems
    );
}

/// A file half a bucket long on a copy of `default`, checked clean: its
/// bucket, and the fragmentation lru entry the writer gave the bucket, at
/// `(1 << 61 | dirty * 2^31 / bucket_size) : bucket` (docs/clean-room.md).
fn half_bucket_file(test: &str) -> (std::path::PathBuf, u64, Bpos) {
    let img = scratch(test);
    let dev = FileDevice::open(&img).unwrap();
    let bucket_size = u64::from(Superblock::read(&dev).unwrap().members().unwrap()[0].bucket_size);
    drop(dev);
    let root = fs_bcachefs::Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w
        .create_file(
            root,
            b"half.bin",
            &vec![0x5a; (bucket_size * 512 / 2) as usize],
            0o644,
        )
        .unwrap();
    drop(w);
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.clean(),
        "the file alone is damage: {:?}",
        report.problems
    );

    let extent = keys(&img, btree_id::EXTENTS)
        .into_iter()
        .find(|k| k.pos.inode == ino && k.key_type == key_type::EXTENT)
        .expect("the file's data extent");
    let e = fs_bcachefs::extent::DataExtent::from_key(&extent).unwrap();
    let bucket = e.ptr.offset / bucket_size;
    let alloc = keys(&img, ALLOC)
        .into_iter()
        .find(|k| k.key_type == ALLOC_V4 && k.pos.inode == 0 && k.pos.offset == bucket)
        .expect("the file's bucket has an alloc key");
    let dirty = u64::from(u32::from_le_bytes(alloc.value[16..20].try_into().unwrap()));
    assert!(
        dirty > 0 && dirty < bucket_size,
        "bucket {bucket} holds {dirty} of {bucket_size} sectors: not partly filled"
    );
    let at = Bpos {
        inode: (1 << 61) | ((dirty << 31) / bucket_size),
        offset: bucket,
        snapshot: 0,
    };
    assert!(
        keys(&img, LRU)
            .iter()
            .any(|k| k.key_type == SET && k.pos == at),
        "the writer wrote no fragmentation lru entry at {at}"
    );
    (img, bucket, at)
}

#[test]
fn a_partly_filled_bucket_with_no_fragmentation_lru_entry_is_reported() {
    let (img, bucket, at) = half_bucket_file("lru-missing");
    let gone = Bkey {
        key_type: key_type::DELETED,
        size: 0,
        version_hi: 0,
        version_lo: 0,
        pos: at,
        value: Vec::new(),
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(LRU, vec![gone]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "missing_fragmentation_lru_entry"
                && p.detail.contains(&format!("bucket {bucket}"))),
        "partly filled bucket {bucket} with its lru entry at {at} gone was not reported as \
         missing_fragmentation_lru_entry: {:?}",
        report.problems
    );
}

#[test]
fn an_lru_entry_no_bucket_fill_matches_is_reported() {
    let (img, bucket, at) = half_bucket_file("lru-incorrect");
    let stray = Bkey {
        key_type: SET,
        size: 0,
        version_hi: 0,
        version_lo: 0,
        pos: Bpos {
            inode: at.inode + 1,
            ..at
        },
        value: Vec::new(),
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(LRU, vec![stray]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "incorrect_lru_entry"
                && p.detail.contains(&format!("bucket {bucket}"))),
        "an lru entry for bucket {bucket} one step past its fill, beside the right one at \
         {at}, was not reported as incorrect_lru_entry: {:?}",
        report.problems
    );
}

#[test]
fn a_piece_of_a_checksummed_region_whose_backpointer_is_gone_is_reported() {
    // On the aged image the reference overwrote the middle of
    // /over/middle.bin, leaving pieces of checksummed regions; a piece's
    // backpointer sits at its region's sector << 16 plus its offset in the
    // region (the aged image: the piece at offset 136 of the region at
    // sector 26880 has its backpointer at 26880 << 16 | 136).
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-check-alloc-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("piece-backpointer.img");
    std::fs::copy(fixture("aged.img"), &img).unwrap();
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.clean(),
        "the fixture is damaged: {:?}",
        report.problems
    );
    let ino = fs_bcachefs::Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/over/middle.bin")
        .unwrap();
    let (extent, e) = keys(&img, btree_id::EXTENTS)
        .into_iter()
        .filter(|k| k.pos.inode == ino && k.key_type == key_type::EXTENT)
        .map(|k| {
            let e = fs_bcachefs::extent::DataExtent::from_key(&k).unwrap();
            (k, e)
        })
        .find(|(_, e)| e.crc.is_some_and(|c| c.offset != 0))
        .expect("/over/middle.bin has a piece at an offset into its region");
    let at = Bpos {
        inode: u64::from(e.ptr.dev),
        offset: e.ptr.offset << 16 | u64::from(e.crc.unwrap().offset),
        snapshot: 0,
    };
    assert!(
        keys(&img, BACKPOINTERS)
            .iter()
            .any(|k| k.key_type == BACKPOINTER && k.pos == at),
        "the reference left no backpointer at {at}"
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
        "piece {} with no backpointer at {at} was not reported as missing_backpointer: {:?}",
        extent.pos,
        report.problems
    );
}

/// A bucket of `default` waiting for discard, and its need_discard key, at
/// `journal_seq_empty : bucket` with the sequence number word 6 of its alloc
/// key (docs/clean-room.md, "Allocating space"; every need_discard bucket of
/// the reference's fixtures has one there).
fn waiting_bucket(test: &str) -> (std::path::PathBuf, u64, Bpos) {
    let img = scratch(test);
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.clean(),
        "the fixture is damaged: {:?}",
        report.problems
    );
    let alloc = keys(&img, ALLOC)
        .into_iter()
        .find(|k| k.key_type == ALLOC_V4 && k.value[ALLOC_DATA_TYPE] == DATA_NEED_DISCARD)
        .expect("default has a bucket waiting for discard");
    let seq = u64::from_le_bytes(alloc.value[48..56].try_into().unwrap());
    let at = Bpos {
        inode: seq,
        offset: alloc.pos.offset,
        snapshot: 0,
    };
    assert!(
        keys(&img, NEED_DISCARD)
            .iter()
            .any(|k| k.key_type == SET && k.pos == at),
        "the reference left no need_discard key at {at}"
    );
    (img, alloc.pos.offset, at)
}

#[test]
fn a_bucket_waiting_for_discard_with_no_need_discard_key_is_reported() {
    let (img, bucket, at) = waiting_bucket("need-discard-missing");
    let gone = Bkey {
        key_type: key_type::DELETED,
        size: 0,
        version_hi: 0,
        version_lo: 0,
        pos: at,
        value: Vec::new(),
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(NEED_DISCARD, vec![gone]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "missing_need_discard_entry"
                && p.detail.contains(&format!("bucket {bucket}"))),
        "bucket {bucket}, waiting for discard, with its need_discard key at {at} gone was not \
         reported as missing_need_discard_entry: {:?}",
        report.problems
    );
}

#[test]
fn a_need_discard_key_no_waiting_bucket_gives_is_reported() {
    let (img, bucket, at) = waiting_bucket("need-discard-stray");
    let stray = Bkey {
        key_type: SET,
        size: 0,
        version_hi: 0,
        version_lo: 0,
        pos: Bpos {
            inode: at.inode + 1,
            ..at
        },
        value: Vec::new(),
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(NEED_DISCARD, vec![stray]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "incorrect_need_discard_entry"
                && p.detail.contains(&format!("bucket {bucket}"))),
        "a need_discard key for bucket {bucket} one sequence number past its own, beside the \
         right one at {at}, was not reported as incorrect_need_discard_entry: {:?}",
        report.problems
    );
}

#[test]
fn a_device_accounting_out_of_step_with_the_alloc_keys_is_reported() {
    let img = scratch("accounting");
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.clean(),
        "the fixture is damaged: {:?}",
        report.problems
    );
    // The device's accounting of user data: kind 3, then the device and
    // the data type, byte by byte (docs/clean-room.md, "Allocating space");
    // its value is buckets, sectors, fragmented.
    let at = Bpos {
        inode: 0x0300_0000_0000_0000 | u64::from(DATA_USER) << 40,
        offset: 0,
        snapshot: 0,
    };
    let user = keys(&img, ACCOUNTING)
        .into_iter()
        .find(|k| k.key_type == ACCOUNTING_KEY && k.pos == at)
        .expect("default accounts for its user data");
    let mut bad = user.clone();
    let buckets = u64::from_le_bytes(bad.value[0..8].try_into().unwrap());
    bad.value[0..8].copy_from_slice(&(buckets + 1).to_le_bytes());

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(ACCOUNTING, vec![bad]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "accounting_mismatch"
                && p.detail.contains(&format!("data type {DATA_USER}"))),
        "the device's user accounting, {} buckets where its alloc keys add up to {buckets}, was \
         not reported as accounting_mismatch: {:?}",
        buckets + 1,
        report.problems
    );
}

#[test]
fn an_unclean_filesystem_whose_bucket_gens_runs_ahead_of_its_alloc_keys_is_not_judged() {
    // After an unclean shutdown allocation information may lag the rest:
    // the reference's own crash images (`aged-unclean`, CI runs 38505641616
    // and 38507148919) each hold a bucket whose generation in bucket_gens
    // is one ahead of its alloc key, and the reference rebuilds such derived
    // data in its recovery (S1 1.3.1). Here one bucket's bucket_gens is put
    // a generation ahead on a copy of `default`, and a journalled session
    // then leaves the filesystem unclean.
    let img = scratch("unclean-bucket-gens");
    let alloc = keys(&img, ALLOC)
        .into_iter()
        .find(|k| k.key_type == ALLOC_V4 && k.value.len() > 12)
        .expect("the fixture has an alloc key");
    let bucket = alloc.pos.offset;
    let gen = alloc.value[12];
    let gens = keys(&img, BUCKET_GENS)
        .into_iter()
        .find(|k| {
            k.key_type == BUCKET_GENS_KEY
                && k.pos.inode == alloc.pos.inode
                && k.pos.offset == bucket >> 8
        })
        .expect("the fixture has a bucket_gens key for the bucket");
    let mut ahead = gens.clone();
    ahead.value[(bucket & 0xff) as usize] = gen.wrapping_add(1);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(BUCKET_GENS, vec![ahead]).unwrap();
    drop(w);
    let root = fs_bcachefs::Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open_journalled(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.create_file(root, b"after", b"a file in the journal\n", 0o644)
        .unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.replayed,
        "the journalled session left the filesystem clean, so this tests nothing"
    );
    assert!(
        report.clean(),
        "an unclean filesystem's allocation information was judged: {:?}",
        report.problems
    );
}

/// Differential run 38814891184 (kernel fixture): the reference refuses a
/// bucket_gens key with a nonzero size ("size != 0", `bkey_size_nonzero`).
#[test]
fn a_sized_bucket_gens_key_is_reported() {
    let img = scratch("bucket-gens-size");
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        !report
            .problems
            .iter()
            .any(|p| p.kind == "bkey_size_nonzero"),
        "the unchanged fixture already reports a sized key: {:?}",
        report.problems
    );

    let gens = keys(&img, BUCKET_GENS)
        .into_iter()
        .find(|k| k.key_type == BUCKET_GENS_KEY)
        .expect("the fixture has a bucket_gens key");
    assert_eq!(gens.size, 0, "the fixture's bucket_gens key has a size");
    let mut bad = gens.clone();
    bad.size = 1;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(BUCKET_GENS, vec![bad]).unwrap();
    drop(w);

    let stored = keys(&img, BUCKET_GENS)
        .into_iter()
        .find(|k| k.key_type == BUCKET_GENS_KEY && k.pos == gens.pos)
        .expect("the changed bucket_gens key reads back");
    assert_eq!(stored.size, 1, "the stored key keeps its size");
    assert_eq!(stored.value, gens.value, "the generations are preserved");

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "bkey_size_nonzero" && p.detail.contains(&gens.pos.to_string())),
        "the sized bucket_gens key at {} was not reported: {:?}",
        gens.pos,
        report.problems
    );
    std::fs::remove_file(&img).ok();
}

#[test]
fn an_xattr_key_in_the_dirents_btree_is_reported() {
    let img = scratch("dirents-xattr");
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        !report
            .problems
            .iter()
            .any(|p| p.kind == "bkey_invalid_type_for_btree"),
        "the unchanged fixture already reports an invalid key: {:?}",
        report.problems
    );

    let dirent = keys(&img, btree_id::DIRENTS)
        .into_iter()
        .find(|k| k.key_type == key_type::DIRENT)
        .expect("the fixture has a dirent");
    let mut bad = dirent.clone();
    bad.key_type = key_type::XATTR;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::DIRENTS, vec![bad]).unwrap();
    drop(w);

    let stored = keys(&img, btree_id::DIRENTS)
        .into_iter()
        .find(|k| k.pos == dirent.pos)
        .expect("the changed dirents key reads back");
    assert_eq!(
        stored.key_type,
        key_type::XATTR,
        "the stored key keeps its type"
    );
    assert_eq!(stored.value, dirent.value, "the value is preserved");

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "bkey_invalid_type_for_btree"
                && p.detail.contains(&dirent.pos.to_string())),
        "the xattr key in dirents at {} was not reported: {:?}",
        dirent.pos,
        report.problems
    );
    std::fs::remove_file(&img).ok();
}

/// One offender is one finding (#219): the xattr key in dirents was once
/// reported twice, as `invalid_bkey` by the walk and as
/// `bkey_invalid_type_for_btree` by the key-field check. A second, distinct
/// offender in the same btree keeps its own finding, and the unchanged
/// fixture reports neither.
#[test]
fn an_xattr_in_dirents_is_one_finding_beside_a_distinct_offender() {
    const TYPE_KINDS: &[&str] = &["invalid_bkey", "bkey_invalid_type_for_btree"];
    let img = scratch("dirents-xattr-once");
    let dirents: Vec<Bkey> = keys(&img, btree_id::DIRENTS)
        .into_iter()
        .filter(|k| k.key_type == key_type::DIRENT)
        .collect();
    assert!(dirents.len() >= 2, "the fixture has two dirents");
    let (xattr_at, sized_at) = (dirents[0].pos, dirents[1].pos);

    let control = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        !control
            .problems
            .iter()
            .any(|p| TYPE_KINDS.contains(&p.kind) || p.kind == "bkey_size_nonzero"),
        "the unchanged fixture already reports a dirents key: {:?}",
        control.problems
    );

    let mut xattr = dirents[0].clone();
    xattr.key_type = key_type::XATTR;
    let mut sized = dirents[1].clone();
    sized.size = 1;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::DIRENTS, vec![xattr, sized]).unwrap();
    drop(w);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    let at = |pos: Bpos| {
        report
            .problems
            .iter()
            .filter(|p| p.detail.contains(&pos.to_string()))
            .collect::<Vec<_>>()
    };
    let xattr_findings = at(xattr_at);
    assert_eq!(
        xattr_findings.len(),
        1,
        "the xattr key in dirents at {xattr_at} is one finding: {:?}",
        report.problems
    );
    assert!(
        TYPE_KINDS.contains(&xattr_findings[0].kind),
        "the xattr key in dirents is reported as a wrong key type: {:?}",
        xattr_findings
    );
    let sized_findings = at(sized_at);
    assert_eq!(
        sized_findings.len(),
        1,
        "the sized dirent at {sized_at} is its own finding: {:?}",
        report.problems
    );
    assert_eq!(sized_findings[0].kind, "bkey_size_nonzero");
    std::fs::remove_file(&img).ok();
}
