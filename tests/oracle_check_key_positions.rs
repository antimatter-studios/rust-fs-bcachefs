//! A key's position and its value's own lengths, judged as the reference
//! checker judges them (#219). Differential run 38579070302 (reference
//! checker 1.39.7) changed one field of a real key and committed it through
//! the writer's raw `insert`, or changed a node, and the reference refused
//! the key where this checker said nothing:
//!
//! - a subvolume at offset 0, or at inode U64_MAX: "invalid pos, deleting"
//!   (`subvol_pos_bad`);
//! - a snapshot_tree at offset 0, or (nightly run 38878947701) at offset
//!   U64_MAX: "bad pos, deleting" (`snapshot_tree_pos_bad`);
//! - an accounting key of the `btree` kind at a snapshot, or with bytes
//!   set below its btree id: "junk at end of accounting key, deleting"
//!   (`accounting_key_junk_at_end`);
//! - an xattr whose name and value lengths run past its value: "value too
//!   small, deleting" (`xattr_val_size_too_small`).
//!
//! Each test repeats one disagreement class of that run on the fixture it
//! was seen on, with the value the run wrote.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

/// Btrees and key types by number, in the Principles of Operation's list
/// order (S1 11.3, 11.5).
const SUBVOLUMES: u8 = 8;
const SNAPSHOTS: u8 = 9;
const SNAPSHOT_TREES: u8 = 15;
const ACCOUNTING: u8 = 20;
const SUBVOLUME: u8 = 21;
const SNAPSHOT: u8 = 22;
const SNAPSHOT_TREE: u8 = 31;
const ACCOUNTING_KEY: u8 = 34;
/// An accounting key's kind, the top byte of its position's inode: the
/// lister prints 6 as `btree`, with the btree's id in the byte below.
const ACCT_BTREE: u64 = 6;

/// Change the first key of type `kt` in btree `id` of a copy of `set` (the
/// first that `pick` also accepts) with `change`, commit it with the
/// writer's raw `insert`, and require the checker to report `kind` for it.
fn refused(
    set: &str,
    id: u8,
    kt: u8,
    pick: impl Fn(&Bkey) -> bool,
    change: impl FnOnce(&mut Bkey),
    kind: &str,
) {
    let test = format!("{set}-{id}-{kt}-{kind}");
    // Removed when the test ends however it ends.
    let img = common::ScratchDir::new("check-key-positions").into_image(format!("{test}.img"));
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
    change(&mut k);
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

fn btree_kind(k: &Bkey) -> bool {
    k.pos.inode >> 56 == ACCT_BTREE
}

#[test]
fn a_subvolume_at_offset_0_is_refused() {
    let to_0 = |k: &mut Bkey| k.pos.offset = 0;
    refused(
        "default",
        SUBVOLUMES,
        SUBVOLUME,
        any,
        to_0,
        "subvol_pos_bad",
    );
}

#[test]
fn a_subvolume_at_inode_u64_max_is_refused() {
    let to_max = |k: &mut Bkey| k.pos.inode = u64::MAX;
    let kind = "subvol_pos_bad";
    refused("kernel-rootsnap", SUBVOLUMES, SUBVOLUME, any, to_max, kind);
}

/// Run 39018152705, default/block4k seed 1659458: the reference
/// rejects the root snapshot at U64_MAX:U32_MAX:0 as snapshot_pos_bad.
#[test]
fn a_snapshot_at_inode_u64_max_is_refused() {
    for set in ["default", "block4k"] {
        refused(
            set,
            SNAPSHOTS,
            SNAPSHOT,
            |k| k.pos.inode == 0 && k.pos.offset == u64::from(u32::MAX),
            |k| k.pos.inode = u64::MAX,
            "snapshot_pos_bad",
        );
    }
}

/// Run 39018152705, default/block4k seed 1659461: the reference
/// rejects snapshot tree 1 at 1:1:0 as snapshot_tree_pos_bad (#254).
#[test]
fn a_snapshot_tree_at_inode_1_is_refused() {
    for set in ["default", "block4k"] {
        refused(
            set,
            SNAPSHOT_TREES,
            SNAPSHOT_TREE,
            |k| k.pos.inode == 0 && k.pos.offset == 1 && k.pos.snapshot == 0,
            |k| k.pos.inode = 1,
            "snapshot_tree_pos_bad",
        );
    }
}

/// Run 39018152705, default/block4k seed 1659490: the reference rejects
/// inode_v3 at U64_MAX:2147483849:U32_MAX as inode_pos_inode_nonzero (#255).
/// Select a regular file by path so freshly generated inode numbers work too.
#[test]
fn an_inode_key_at_inode_u64_max_is_refused() {
    for set in ["default", "block4k"] {
        let ino = Filesystem::open(FileDevice::open(fixture(&format!("{set}.img"))).unwrap())
            .unwrap()
            .lookup("/many/file-0200.txt")
            .unwrap();
        refused(
            set,
            btree_id::INODES,
            key_type::INODE_V3,
            |k| k.pos.inode == 0 && k.pos.offset == ino && k.pos.snapshot == u32::MAX,
            |k| k.pos.inode = u64::MAX,
            "inode_pos_inode_nonzero",
        );
    }
}

#[test]
fn a_snapshot_tree_at_offset_0_is_refused() {
    let to_0 = |k: &mut Bkey| k.pos.offset = 0;
    let kind = "snapshot_tree_pos_bad";
    refused(
        "kernel-rootsnap",
        SNAPSHOT_TREES,
        SNAPSHOT_TREE,
        any,
        to_0,
        kind,
    );
}

/// Nightly run 38878947701, kernel seed 1659362: the reference rejects
/// snapshot tree 1 at 0:U64_MAX:0 as snapshot_tree_pos_bad (#219).
#[test]
fn a_snapshot_tree_at_offset_u64_max_is_refused() {
    refused(
        "kernel",
        SNAPSHOT_TREES,
        SNAPSHOT_TREE,
        |k| k.pos.inode == 0 && k.pos.offset == 1 && k.pos.snapshot == 0,
        |k| k.pos.offset = u64::MAX,
        "snapshot_tree_pos_bad",
    );
}

#[test]
fn a_btree_accounting_key_at_a_snapshot_is_refused() {
    let to_1 = |k: &mut Bkey| k.pos.snapshot = 1;
    let kind = "accounting_key_junk_at_end";
    refused("kernel", ACCOUNTING, ACCOUNTING_KEY, btree_kind, to_1, kind);
}

#[test]
fn a_btree_accounting_key_with_bytes_below_its_btree_id_is_refused() {
    // The run's damaged node held one at btree=backpointers with 0xff0000
    // set below the id byte.
    let junk = |k: &mut Bkey| k.pos.inode |= 0x00ff_0000;
    let kind = "accounting_key_junk_at_end";
    refused(
        "kernel-written",
        ACCOUNTING,
        ACCOUNTING_KEY,
        btree_kind,
        junk,
        kind,
    );
}

#[test]
fn an_xattr_key_with_a_nonzero_size_is_refused() {
    // The same attribute as aged seed 1659486 in run 39018152705 (#258),
    // selected by name because regenerated fixtures may renumber inodes.
    refused(
        "aged",
        btree_id::XATTRS,
        key_type::XATTR,
        |k| k.value.get(..2) == Some(&[0, 6]) && k.value.get(4..10) == Some(b"attr29"),
        |k| k.size = 1,
        "bkey_size_nonzero",
    );
}

#[test]
fn an_xattr_whose_lengths_run_past_its_value_is_refused() {
    // The run's change: the value's first word all ones, a name of 255
    // bytes and a value of 65535 in a value of five words.
    let ones = |k: &mut Bkey| k.value[0..8].copy_from_slice(&u64::MAX.to_le_bytes());
    let kind = "xattr_val_size_too_small";
    let xattr = key_type::XATTR;
    refused("aged", btree_id::XATTRS, xattr, any, ones, kind);
}
