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
const SUBVOLUME_CHILDREN: u8 = 19;
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
    Inode(u64),
    KeyType(u8),
}

/// Change one field of the first key of type `kt` in btree `id` of a copy
/// of `set` (the first that `pick` also accepts), commit it with the
/// writer's raw `insert`, and require the checker to report `kind` for it.
fn refused(set: &str, id: u8, kt: u8, pick: impl Fn(&Bkey) -> bool, change: Change, kind: &str) {
    let test = format!("{set}-{id}-{kt}-{kind}");
    // Removed when the test ends however it ends.
    let img = common::ScratchDir::new("check-key-fields").into_image(format!("{test}.img"));
    std::fs::copy(fixture(&format!("{set}.img")), &img).unwrap();

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
        Change::Inode(inode) => {
            assert_ne!(
                k.pos.inode, inode,
                "{test}: the reference-made key is there"
            );
            k.pos.inode = inode;
        }
        Change::KeyType(key_type) => k.key_type = key_type,
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

/// Nightly differential run 39756419740 (#376) moved the cursor to a
/// snapshot (seed 1659898), to inode U64_MAX (seed 1659851) and to inode 0
/// (seed 1659879); the reference deleted it as "nonzero snapshot" and as
/// "k.p.inode bad".
#[test]
fn an_inode_alloc_cursor_at_a_snapshot_is_refused() {
    let kind = "bkey_snapshot_nonzero";
    let snap = Change::Snapshot(u32::MAX - 1);
    refused("default", LOGGED_OPS, INODE_ALLOC_CURSOR, any, snap, kind);
}

#[test]
fn an_inode_alloc_cursor_at_inode_u64_max_is_refused() {
    let kind = "inode_alloc_cursor_inode_bad";
    let inode = Change::Inode(u64::MAX);
    refused("aged", LOGGED_OPS, INODE_ALLOC_CURSOR, any, inode, kind);
}

#[test]
fn an_inode_alloc_cursor_at_inode_0_is_refused() {
    let kind = "inode_alloc_cursor_inode_bad";
    let inode = Change::Inode(0);
    refused(
        "kernel-rootsnap",
        LOGGED_OPS,
        INODE_ALLOC_CURSOR,
        any,
        inode,
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

/// Differential run 38753768243 changed a dirent of kernel-rootsnap.img
/// (seed 10) into an xattr; the reference deleted it as "invalid key type
/// for btree dirents (xattr)".
#[test]
fn an_xattr_in_dirents_is_refused() {
    let kind = "bkey_invalid_type_for_btree";
    let xattr = Change::KeyType(key_type::XATTR);
    refused(
        "kernel-rootsnap",
        btree_id::DIRENTS,
        key_type::DIRENT,
        any,
        xattr,
        kind,
    );
}

/// Nightly differential run 38878947701 moved an alloc key from device 0 to
/// device 1 (seed 1659438, on aged.img and kernel-written.img) and a
/// bucket_gens key the same way (seed 1659387, on kernel.img). Each fixture
/// has one device, and the reference stopped on each key with "pointer to
/// nonexistent device 1" (#163).
#[test]
fn an_alloc_key_on_a_device_the_filesystem_lacks_is_refused() {
    let kind = "pointer_to_nonexistent_device";
    for set in ["aged", "kernel-written"] {
        let moved = Change::Inode(1);
        refused(set, btree_id::ALLOC, key_type::ALLOC_V4, any, moved, kind);
    }
}

#[test]
fn a_bucket_gens_key_on_a_device_the_filesystem_lacks_is_refused() {
    let kind = "pointer_to_nonexistent_device";
    let moved = Change::Inode(1);
    refused("kernel", BUCKET_GENS, BUCKET_GENS_KEY, any, moved, kind);
}

/// Nightly differential run 38878947701 (#219) gave the root snapshot a size
/// (aged seed 1659430, kernel seed 1659425), gave a subvolume_children key a
/// size (kernel-rootsnap seeds 1659417 and 1659425) and moved a
/// snapshot_tree to a snapshot (kernel seed 1659439). The reference deleted
/// each one, as "size != 0" and as "nonzero snapshot".
#[test]
fn a_sized_snapshot_key_is_refused() {
    let kind = "bkey_size_nonzero";
    for set in ["aged", "kernel"] {
        refused(set, SNAPSHOTS, SNAPSHOT, any, Change::Size(1), kind);
    }
}

#[test]
fn a_sized_subvolume_children_key_is_refused() {
    let kind = "bkey_size_nonzero";
    let size = Change::Size(1);
    refused("kernel-rootsnap", SUBVOLUME_CHILDREN, SET, any, size, kind);
}

#[test]
fn a_snapshot_tree_at_a_snapshot_is_refused() {
    let kind = "bkey_snapshot_nonzero";
    let snap = Change::Snapshot(u32::MAX - 1);
    refused("kernel", SNAPSHOT_TREES, SNAPSHOT_TREE, any, snap, kind);
}

/// Nightly-seed run 39018152705 changed kernel-rootsnap.img's
/// subvolume_children key at 1:2:0 into a logged_op_truncate (seed 1659445),
/// and nightly run 38878947701 into a logged_op_finsert (seed 1659394); the
/// reference deleted each as "invalid key type for btree subvolume_children"
/// (#266).
#[test]
fn a_logged_operation_in_subvolume_children_is_refused() {
    const LOGGED_OP_TRUNCATE: u8 = 32;
    const LOGGED_OP_FINSERT: u8 = 33;
    let kind = "bkey_invalid_type_for_btree";
    for kt in [LOGGED_OP_TRUNCATE, LOGGED_OP_FINSERT] {
        let to = Change::KeyType(kt);
        refused("kernel-rootsnap", SUBVOLUME_CHILDREN, SET, any, to, kind);
    }
}

/// Nightly-seed run 39018152705 changed a backpointer of kernel-rootsnap.img
/// into an inode_v3 (seed 1659466, at 0:381681664:0, its value kept); the
/// reference deleted it as "invalid key type for btree backpointers
/// (inode_v3)" (#268).
#[test]
fn an_inode_in_backpointers_is_refused() {
    let kind = "bkey_invalid_type_for_btree";
    let to = Change::KeyType(key_type::INODE_V3);
    refused("kernel-rootsnap", BACKPOINTERS, BACKPOINTER, any, to, kind);
}

/// Nightly differential run 39756419740, re-run on the top of stack #365 as
/// run 39764355709, changed a backpointer of kernel-rootsnap.img into a
/// snapshot (seed 1659883, at 0:390070272:0, its value kept); the reference
/// refused it as "invalid bkey in btree_node btree=backpointers level=0:
/// u64s 9 type snapshot" (#378).
#[test]
fn a_snapshot_in_backpointers_is_refused() {
    let kind = "bkey_invalid_type_for_btree";
    let to = Change::KeyType(SNAPSHOT);
    refused("kernel-rootsnap", BACKPOINTERS, BACKPOINTER, any, to, kind);
}
