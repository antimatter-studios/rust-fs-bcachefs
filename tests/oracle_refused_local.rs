//! What this reader must refuse rather than misread, shown on fixture
//! copies this crate has altered with its own writer: a second snapshot
//! of the root inode, and extents of one file that overlap. Neither can be
//! made by the reference tools in the guest yet (no kernel with bcachefs
//! for snapshots; the reference never writes an overlap), so the writer's
//! raw `insert` stands in. The checker must name both too.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::inode::ROOT_INO;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::{Error, Filesystem};
use fs_core::FileDevice;

fn scratch(name: &str, test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-refused-local-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(name), &img).unwrap();
    img
}

/// Every leaf key of one btree of the image.
fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

fn insert(img: &std::path::Path, id: u8, key: Bkey) {
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    w.insert(id, vec![key]).unwrap();
}

#[test]
fn a_root_inode_at_a_second_snapshot_is_refused_and_named() {
    let img = scratch("write-study/base.img", "second-snapshot");
    Filesystem::open(FileDevice::open(&img).unwrap()).expect("the fixture opens as it is");
    let root = keys(&img, btree_id::INODES)
        .into_iter()
        .find(|k| k.pos.inode == 0 && k.pos.offset == ROOT_INO && k.key_type == key_type::INODE_V3)
        .expect("the root inode key");
    // The same inode, as a snapshot would leave it: at another snapshot id.
    let mut copy = root.clone();
    copy.pos.snapshot = 1;
    insert(&img, btree_id::INODES, copy);

    match Filesystem::open(FileDevice::open(&img).unwrap()) {
        Err(Error::Unsupported(m)) if m.contains("snapshot") => {}
        other => panic!(
            "a filesystem with two snapshots of its root was not refused: {:?}",
            other.map(|_| ())
        ),
    }
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.problems.iter().any(|p| p.kind == "snapshots"),
        "the checker did not name the second snapshot: {:?}",
        report.problems
    );
}

#[test]
fn overlapping_extents_are_refused_by_the_reader_and_named_by_the_checker() {
    let img = scratch("write-study/base.img", "overlap");
    let first = keys(&img, btree_id::EXTENTS)
        .into_iter()
        .find(|k| k.key_type == key_type::INLINE_DATA)
        .expect("an inline extent in the fixture");
    let ino = first.pos.inode;
    {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.read(ino).expect("the file reads as it is");
    }
    // A second extent one sector further on and two sectors long: it
    // covers the sector the first one holds.
    let mut second = first.clone();
    second.pos.offset = first.pos.offset + 1;
    second.size = 2;
    second.value = vec![b'x'; 8];
    insert(&img, btree_id::EXTENTS, second);

    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    match fs.read(ino) {
        Err(Error::Corrupt(m)) if m.contains("overlap") => {}
        other => panic!(
            "overlapping extents were not refused: {:?}",
            other.map(|b| b.len())
        ),
    }
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.problems.iter().any(|p| p.kind == "extent_overlap"),
        "the checker did not name the overlap: {:?}",
        report.problems
    );
}

#[test]
fn an_older_inode_encoding_is_unsupported_not_missing() {
    let img = scratch("write-study/base.img", "inode-v2");
    let ino = 777_777u64;
    // A v2 inode where there was none: the filesystem now holds an inode
    // this reader does not decode, which is not "no such inode".
    insert(
        &img,
        btree_id::INODES,
        Bkey {
            key_type: key_type::INODE_V2,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: fs_bcachefs::bkey::Bpos {
                inode: 0,
                offset: ino,
                snapshot: u32::MAX,
            },
            value: vec![0u8; 48],
        },
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    match fs.inode(ino) {
        Err(Error::Unsupported(m)) if m.contains("inode_v2") => {}
        other => panic!("an inode_v2 key was not refused by name: {other:?}"),
    }
}

#[test]
fn an_error_extent_reads_as_lost_data_and_is_named_by_the_checker() {
    let img = scratch("write-study/base.img", "error-extent");
    let first = keys(&img, btree_id::EXTENTS)
        .into_iter()
        .find(|k| k.key_type == key_type::INLINE_DATA)
        .expect("an inline extent in the fixture");
    let ino = first.pos.inode;
    // The same range, marked as permanently lost (a zero-byte value since
    // 1.34, S1 11.6).
    let mut lost = first.clone();
    lost.key_type = key_type::ERROR;
    lost.value = Vec::new();
    insert(&img, btree_id::EXTENTS, lost);

    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    match fs.read(ino) {
        Err(Error::Io(m)) if m.contains("lost") => {}
        other => panic!(
            "an error extent did not read as lost data: {:?}",
            other.map(|b| b.len())
        ),
    }
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.problems.iter().any(|p| p.kind == "data_lost"),
        "the checker did not name the lost range: {:?}",
        report.problems
    );
}

/// `key`'s first pointer word with its device and generation replaced.
fn repoint(key: &Bkey, dev: u8, gen: u8) -> Bkey {
    let mut k = key.clone();
    let mut p = 0;
    while p + 8 <= k.value.len() {
        let w = u64::from_le_bytes(k.value[p..p + 8].try_into().unwrap());
        if w == 0 {
            break;
        }
        match w.trailing_zeros() {
            0 => {
                let w = (w & 0x0000_ffff_ffff_ffff) | u64::from(dev) << 48 | u64::from(gen) << 56;
                k.value[p..p + 8].copy_from_slice(&w.to_le_bytes());
                return k;
            }
            1 => p += 8,
            2 => p += 16,
            _ => break,
        }
    }
    panic!("no pointer in {key:?}");
}

/// The first allocated (non-inline) extent of the fixture and its pointer.
/// The base image's one file is stored inline, so these tests use the
/// write study's `create-large` image, whose 300000-byte file is not.
fn an_extent(img: &std::path::Path) -> (Bkey, fs_bcachefs::extent::Ptr) {
    let k = keys(img, btree_id::EXTENTS)
        .into_iter()
        .find(|k| k.key_type == key_type::EXTENT)
        .expect("an allocated extent in the fixture");
    let ptr = fs_bcachefs::extent::DataExtent::from_key(&k).unwrap().ptr;
    (k, ptr)
}

#[test]
fn a_pointer_to_another_device_is_refused_and_named() {
    let img = scratch("write-study/create-large.img", "pointer-device");
    let (k, ptr) = an_extent(&img);
    insert(&img, btree_id::EXTENTS, repoint(&k, 1, ptr.gen));
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    match fs.read(k.pos.inode) {
        Err(Error::Corrupt(m)) if m.contains("device 1") => {}
        other => panic!(
            "a pointer to device 1 was read: {:?}",
            other.map(|b| b.len())
        ),
    }
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "extent_pointer" && p.detail.contains("device")),
        "{:?}",
        report.problems
    );
}

#[test]
fn a_stale_pointer_is_refused_and_named() {
    let img = scratch("write-study/create-large.img", "pointer-stale");
    let (k, ptr) = an_extent(&img);
    {
        let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
        fs.read(k.pos.inode)
            .expect("the file reads with its own generation");
    }
    insert(
        &img,
        btree_id::EXTENTS,
        repoint(&k, ptr.dev, ptr.gen.wrapping_add(1)),
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    match fs.read(k.pos.inode) {
        Err(Error::Corrupt(m)) if m.contains("stale") => {}
        other => panic!("a stale pointer was read: {:?}", other.map(|b| b.len())),
    }
    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "extent_pointer" && p.detail.contains("stale")),
        "{:?}",
        report.problems
    );
}
