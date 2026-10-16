//! What the checker misses that the reference checker finds (#163): one
//! test per kind, each on a copy of `default` with the damage made through
//! the writer's raw `insert`, and each requiring the checker to name it.
//!
//! - an xattr whose inode is gone (the reference's `key_in_missing_inode`,
//!   on all 54 images a writer stress run left);
//! - an xattr away from its name's hash slot, with an empty slot between
//!   (`hash_table_key_wrong_offset`);
//! - an inode whose back-reference (`bi_dir`, `bi_dir_offset`) names no
//!   entry, or an entry naming another inode ("inode points to missing
//!   dirent", "inode points to dirent that does not point back");
//! - an inode whose has-options flag (bit 12) disagrees with its option
//!   fields ("inode has_inode_opts flag wrong").
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::inode::{InodeV3Raw, FIELD_NAMES};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn scratch(test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("check-namespace").into_image(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();
    img
}

fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

/// The inode key of `path`.
fn inode_key(img: &std::path::Path, path: &str) -> Bkey {
    let ino = Filesystem::open(FileDevice::open(img).unwrap())
        .unwrap()
        .lookup(path)
        .unwrap();
    keys(img, btree_id::INODES)
        .into_iter()
        .find(|k| k.pos.inode == 0 && k.pos.offset == ino && k.key_type == key_type::INODE_V3)
        .expect("the inode's key")
}

/// The checker's problem kinds on `img`, which must include `kind`.
#[track_caller]
fn assert_reported(img: &std::path::Path, kind: &str, what: &str) {
    let report = check::check(&FileDevice::open(img).unwrap()).expect("the checker runs");
    assert!(
        report.problems.iter().any(|p| p.kind == kind),
        "{what} was not reported as {kind}: {:?}",
        report.problems
    );
}

/// `raw` with varint field `name` set to `value`.
fn with_field(mut raw: InodeV3Raw, name: &str, value: u64) -> InodeV3Raw {
    let i = 8 + FIELD_NAMES.iter().position(|&n| n == name).unwrap();
    assert!(raw.varints.len() > i, "the inode stores no {name}");
    raw.varints[i] = value;
    raw
}

fn rekeyed(k: &Bkey, raw: &InodeV3Raw) -> Bkey {
    Bkey {
        value: raw.encode(),
        ..k.clone()
    }
}

/// An xattr set through the writer on /hello.txt, and its key.
fn hello_with_xattr(img: &std::path::Path) -> Bkey {
    let ino = Filesystem::open(FileDevice::open(img).unwrap())
        .unwrap()
        .lookup("/hello.txt")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    w.set_xattr(ino, b"user.colour", b"blue").unwrap();
    drop(w);
    let report = check::check(&FileDevice::open(img).unwrap()).unwrap();
    assert!(
        report.clean(),
        "the xattr alone is damage: {:?}",
        report.problems
    );
    keys(img, btree_id::XATTRS)
        .into_iter()
        .find(|k| k.pos.inode == ino && k.key_type == key_type::XATTR)
        .expect("the xattr's key")
}

#[test]
fn an_xattr_whose_inode_is_gone_is_key_in_missing_inode() {
    let img = scratch("xattr-missing-inode");
    let x = hello_with_xattr(&img);
    let orphan = Bkey {
        pos: Bpos {
            inode: 999_999,
            ..x.pos
        },
        ..x
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::XATTRS, vec![orphan]).unwrap();
    drop(w);
    assert_reported(&img, "key_in_missing_inode", "an xattr of inode 999999");
}

#[test]
fn an_xattr_away_from_its_slot_is_hash_table_key_wrong_offset() {
    let img = scratch("xattr-wrong-offset");
    let x = hello_with_xattr(&img);
    // Five slots on, with nothing between: a lookup from the name's slot
    // stops at the first empty one and never reaches it.
    let moved = Bkey {
        pos: Bpos {
            offset: x.pos.offset + 5,
            ..x.pos
        },
        ..x.clone()
    };
    let gone = Bkey {
        key_type: key_type::DELETED,
        value: Vec::new(),
        ..x
    };
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::XATTRS, vec![gone, moved]).unwrap();
    drop(w);
    assert_reported(
        &img,
        "hash_table_key_wrong_offset",
        "an xattr five slots past its hash",
    );
}

#[test]
fn an_inode_naming_no_entry_points_to_a_missing_dirent() {
    let img = scratch("backref-missing");
    let k = inode_key(&img, "/dir/notes.md");
    let raw = InodeV3Raw::parse(&k.value).unwrap();
    let off = raw.field("dir_offset").unwrap();
    assert_ne!(
        raw.field("dir").unwrap(),
        0,
        "the fixture's file has a back-reference"
    );
    let bad = rekeyed(&k, &with_field(raw, "dir_offset", off ^ 0x5a5a));
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::INODES, vec![bad]).unwrap();
    drop(w);
    assert_reported(
        &img,
        "inode_points_to_missing_dirent",
        "an inode whose bi_dir_offset names nothing",
    );
}

#[test]
fn an_inode_naming_another_inodes_entry_points_to_the_wrong_dirent() {
    let img = scratch("backref-wrong");
    let k = inode_key(&img, "/hello.txt");
    let other = InodeV3Raw::parse(&inode_key(&img, "/empty").value).unwrap();
    let raw = InodeV3Raw::parse(&k.value).unwrap();
    assert_eq!(raw.field("dir"), other.field("dir"), "both are in the root");
    let bad = rekeyed(
        &k,
        &with_field(raw, "dir_offset", other.field("dir_offset").unwrap()),
    );
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::INODES, vec![bad]).unwrap();
    drop(w);
    assert_reported(
        &img,
        "inode_points_to_wrong_dirent",
        "an inode whose back-reference names /empty's entry",
    );
}

#[test]
fn an_inode_flagged_with_options_it_has_not_got_is_reported() {
    let img = scratch("opts-flag");
    let k = inode_key(&img, "/hello.txt");
    let mut raw = InodeV3Raw::parse(&k.value).unwrap();
    assert_eq!(
        raw.flags & (1 << 12),
        0,
        "the fixture's file has no options"
    );
    raw.flags |= 1 << 12;
    let bad = rekeyed(&k, &raw);
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(btree_id::INODES, vec![bad]).unwrap();
    drop(w);
    assert_reported(
        &img,
        "inode_has_inode_opts_flag_wrong",
        "an inode flagged as having options, with none",
    );
}
