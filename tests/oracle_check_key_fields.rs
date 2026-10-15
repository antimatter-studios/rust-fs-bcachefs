//! A key's own size and snapshot, judged as the reference checker judges
//! them (#219). Differential run 38579070302 (reference checker 1.39.7)
//! changed one field of a real key and committed it through the writer's
//! raw `insert`, and the reference refused the key where this checker said
//! nothing:
//!
//! - a size on a key of these btrees and types: "size != 0, deleting"
//!   (`bkey_size_nonzero`);
//! - a snapshot on a key of these btrees and types: "nonzero snapshot,
//!   deleting" (`bkey_snapshot_nonzero`).
//!
//! Each test repeats one disagreement class of that run on the fixture it
//! was seen on, with the value the run wrote. Only those pairs of btree and
//! key type are judged; the rule is not carried to a pair nobody observed.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

/// Btrees and key types by number, in the Principles of Operation's list
/// order (S1 11.3, 11.5).
const SUBVOLUMES: u8 = 8;
const SNAPSHOTS: u8 = 9;
const LRU: u8 = 10;
const FREESPACE: u8 = 11;
const NEED_DISCARD: u8 = 12;
const BACKPOINTERS: u8 = 13;
const BUCKET_GENS: u8 = 14;
const SNAPSHOT_TREES: u8 = 15;
const LOGGED_OPS: u8 = 17;
const ACCOUNTING: u8 = 20;
const SUBVOLUME: u8 = 21;
const SNAPSHOT: u8 = 22;
const SET: u8 = 25;
const BACKPOINTER: u8 = 28;
const BUCKET_GENS_KEY: u8 = 30;
const SNAPSHOT_TREE: u8 = 31;
const ACCOUNTING_KEY: u8 = 34;
const INODE_ALLOC_CURSOR: u8 = 35;
/// An accounting key's kind, the top byte of its position's inode: the
/// lister prints 6 as `btree` and 8 as `inum`.
const ACCT_BTREE: u64 = 6;
const ACCT_INUM: u64 = 8;

/// Which field the run changed, and to what.
enum Change {
    Size(u32),
    Snapshot(u32),
}

/// Change one field of the first key of type `kt` in btree `id` of a copy
/// of `set` (the first that `pick` also accepts), commit it with the
/// writer's raw `insert`, and require the checker to report `kind` for it.
fn refused(set: &str, id: u8, kt: u8, pick: impl Fn(&Bkey) -> bool, change: Change, kind: &str) {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-check-key-fields-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let test = format!("{set}-{id}-{kt}-{kind}");
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(&format!("{set}.img")), &img).unwrap();
    let _gone = Scratch(img.clone());

    let before = check::check(&FileDevice::open(&img).unwrap()).expect("the checker runs");
    assert!(
        !before.problems.iter().any(|p| p.kind == kind),
        "{test}: the unchanged fixture already reports {kind}: {:?}",
        before.problems
    );
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut k = btree::walk(&dev, &sb, id)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == kt && pick(k))
        .unwrap_or_else(|| panic!("{test}: the fixture has no key of type {kt} in btree {id}"));
    drop(dev);
    match change {
        Change::Size(size) => {
            assert_eq!(k.size, 0, "{test}: the reference-made key has a size");
            k.size = size;
        }
        Change::Snapshot(snapshot) => {
            assert_eq!(
                k.pos.snapshot, 0,
                "{test}: the reference-made key has a snapshot"
            );
            k.pos.snapshot = snapshot;
        }
    }
    let pos = k.pos;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.insert(id, vec![k]).expect("the writer commits the key");
    drop(w);

    let after = check::check(&FileDevice::open(&img).unwrap()).expect("the checker runs");
    assert!(
        after
            .problems
            .iter()
            .any(|p| p.kind == kind && p.detail.contains(&pos.to_string())),
        "{test}: the key at {pos} was not reported as {kind}: {:?}",
        after.problems
    );
}

/// A copy of a fixture, removed when the test ends however it ends: the
/// guest's temporary directory holds a few of them, not sixteen.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

fn any(_: &Bkey) -> bool {
    true
}

fn acct(kind: u64) -> impl Fn(&Bkey) -> bool {
    move |k: &Bkey| k.pos.inode >> 56 == kind
}

#[test]
fn a_sized_inode_alloc_cursor_in_logged_ops_is_refused() {
    let kind = "bkey_size_nonzero";
    refused(
        "default",
        LOGGED_OPS,
        INODE_ALLOC_CURSOR,
        any,
        Change::Size(1),
        kind,
    );
}

#[test]
fn a_sized_subvolume_is_refused() {
    let kind = "bkey_size_nonzero";
    refused("default", SUBVOLUMES, SUBVOLUME, any, Change::Size(1), kind);
}

#[test]
fn a_sized_inode_is_refused() {
    let kind = "bkey_size_nonzero";
    let inode = key_type::INODE_V3;
    refused(
        "default",
        btree_id::INODES,
        inode,
        any,
        Change::Size(1),
        kind,
    );
}

#[test]
fn a_sized_backpointer_is_refused() {
    let kind = "bkey_size_nonzero";
    let size = Change::Size(u32::MAX);
    refused("default", BACKPOINTERS, BACKPOINTER, any, size, kind);
}

#[test]
fn a_sized_backpointer_is_refused_with_4k_blocks() {
    let kind = "bkey_size_nonzero";
    let size = Change::Size(u32::MAX);
    refused("block4k", BACKPOINTERS, BACKPOINTER, any, size, kind);
}

#[test]
fn a_sized_lru_entry_is_refused() {
    let kind = "bkey_size_nonzero";
    refused("aged", LRU, SET, any, Change::Size(u32::MAX), kind);
}

#[test]
fn a_sized_bucket_gens_key_is_refused() {
    let kind = "bkey_size_nonzero";
    refused(
        "kernel",
        BUCKET_GENS,
        BUCKET_GENS_KEY,
        any,
        Change::Size(1),
        kind,
    );
}

#[test]
fn a_sized_inum_accounting_key_is_refused() {
    let kind = "bkey_size_nonzero";
    let inum = acct(ACCT_INUM);
    refused(
        "kernel",
        ACCOUNTING,
        ACCOUNTING_KEY,
        inum,
        Change::Size(1),
        kind,
    );
}

#[test]
fn a_sized_btree_accounting_key_is_refused() {
    let kind = "bkey_size_nonzero";
    let btree = acct(ACCT_BTREE);
    refused(
        "kernel-rootsnap",
        ACCOUNTING,
        ACCOUNTING_KEY,
        btree,
        Change::Size(1),
        kind,
    );
}

#[test]
fn a_sized_dirent_is_refused() {
    let kind = "bkey_size_nonzero";
    let size = Change::Size(u32::MAX);
    refused(
        "kernel",
        btree_id::DIRENTS,
        key_type::DIRENT,
        any,
        size,
        kind,
    );
}

#[test]
fn a_sized_snapshot_tree_is_refused() {
    let kind = "bkey_size_nonzero";
    refused(
        "kernel-rootsnap",
        SNAPSHOT_TREES,
        SNAPSHOT_TREE,
        any,
        Change::Size(1),
        kind,
    );
}

#[test]
fn a_subvolume_at_a_snapshot_is_refused() {
    let kind = "bkey_snapshot_nonzero";
    refused(
        "default",
        SUBVOLUMES,
        SUBVOLUME,
        any,
        Change::Snapshot(1),
        kind,
    );
}

#[test]
fn a_snapshot_key_at_a_snapshot_is_refused() {
    let kind = "bkey_snapshot_nonzero";
    let snap = Change::Snapshot(u32::MAX - 1);
    refused("default", SNAPSHOTS, SNAPSHOT, any, snap, kind);
}

#[test]
fn an_lru_entry_at_a_snapshot_is_refused() {
    let kind = "bkey_snapshot_nonzero";
    refused("aged", LRU, SET, any, Change::Snapshot(u32::MAX - 1), kind);
}

#[test]
fn a_need_discard_key_at_a_snapshot_is_refused() {
    let kind = "bkey_snapshot_nonzero";
    let snap = Change::Snapshot(u32::MAX - 1);
    refused("kernel", NEED_DISCARD, SET, any, snap, kind);
}

#[test]
fn a_freespace_key_at_a_snapshot_is_refused() {
    let kind = "bkey_snapshot_nonzero";
    let snap = Change::Snapshot(u32::MAX - 1);
    refused("kernel-rootsnap", FREESPACE, SET, any, snap, kind);
}
