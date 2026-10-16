//! What the checker misses in the snapshot btrees that the reference
//! checker finds (#220): each test damages a copy of a fixture through the
//! writer's raw `insert` and requires the checker to name the damage.
//!
//! - a snapshot tree whose master subvolume the subvolumes btree does not
//!   have (the reference's "snapshot tree points to missing subvolume");
//! - a snapshot whose skiplist names a snapshot that is not one of its
//!   ancestors ("snapshot with bad skiplist pointer");
//! - a snapshot naming a tree the snapshot_trees btree does not have, or a
//!   tree whose root is not the root its parents lead to ("snapshot points
//!   to missing/incorrect tree"), a parent cycle included.
//!
//! The changes are the ones the differential run measured (CI run
//! 38579070302), and the same changes are judged by the reference checker
//! itself in the guest (`tests/check_oracle.rs`).
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::Bkey;
use fs_bcachefs::btree;
use fs_bcachefs::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

/// The subvolumes, snapshots and snapshot_trees btrees, and their key
/// types (docs/clean-room.md, "Snapshots and subvolumes -- read").
const SUBVOLUMES: u8 = 8;
const SNAPSHOTS: u8 = 9;
const SNAPSHOT_TREES: u8 = 15;
const KEY_SNAPSHOT: u8 = 22;
const KEY_SNAPSHOT_TREE: u8 = 31;
/// Byte offsets in a snapshot's value: the parent in the high half of the
/// first word, `tree` in the high half of the third, the depth and the
/// first skiplist entry in the fourth, the other two in the fifth.
const SNAP_PARENT: usize = 4;
const SNAP_SUBVOL: usize = 16;
const SNAP_TREE: usize = 20;
const SNAP_SKIP0: usize = 28;
const SNAP_SKIP12: usize = 32;
/// A snapshot tree's value: its master subvolume in the low half, its root
/// snapshot in the high half.
const TREE_SUBVOL: usize = 0;

/// The kernel image's snapshots besides the root subvolume's (4294967295):
/// the tree of `sv` (...292) and its snapshot `snap` (...293) under their
/// interior parent (...294).
const INTERIOR: u32 = u32::MAX - 1;
const SV: u32 = u32::MAX - 3;
const SNAP: u32 = u32::MAX - 2;

fn scratch(src: &str, test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("check-snapshots").into_image(format!("{test}.img"));
    std::fs::copy(fixture(src), &img).unwrap();
    img
}

/// The keys of btree `id`, through the journal where the image needs it
/// (aged-unclean), as the checker reads them.
fn keys(img: &std::path::Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let replay = btree::journal_for(&dev, &sb).unwrap();
    btree::walk_replayed(&dev, &sb, id, replay.as_ref()).unwrap()
}

/// The key of type `key_type` at `0:offset` in btree `id`.
fn key_at(img: &std::path::Path, id: u8, key_type: u8, offset: u64) -> Bkey {
    keys(img, id)
        .into_iter()
        .find(|k| k.pos.inode == 0 && k.pos.offset == offset && k.key_type == key_type)
        .unwrap_or_else(|| panic!("btree {id} has no key of type {key_type} at 0:{offset}"))
}

/// `img` with the key at `0:offset` in btree `id` changed by `change`.
fn rewrite(
    img: &std::path::Path,
    id: u8,
    key_type: u8,
    offset: u64,
    change: impl FnOnce(&mut Vec<u8>),
) {
    let mut k = key_at(img, id, key_type, offset);
    change(&mut k.value);
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    w.insert(id, vec![k]).unwrap();
}

fn put32(v: &mut [u8], at: usize, x: u32) {
    v[at..at + 4].copy_from_slice(&x.to_le_bytes());
}

fn put64(v: &mut [u8], at: usize, x: u64) {
    v[at..at + 8].copy_from_slice(&x.to_le_bytes());
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

/// The undamaged images the differential run lays its cases over, and the
/// kernel module's other two: each has its snapshot and its snapshot tree,
/// so the checks below are not vacuous on them, and each is clean.
#[test]
fn every_snapshot_fixture_is_clean_with_its_snapshot_btrees_present() {
    for name in [
        "default",
        "block4k",
        "lz4",
        "aged",
        "aged-unclean",
        "kernel",
        "kernel-rootsnap",
        "kernel-written",
        "kernel-reuse",
    ] {
        let img = fixture(&format!("{name}.img"));
        for (id, ty) in [
            (SNAPSHOTS, KEY_SNAPSHOT),
            (SNAPSHOT_TREES, KEY_SNAPSHOT_TREE),
        ] {
            assert!(
                keys(&img, id).iter().any(|k| k.key_type == ty),
                "{name}: btree {id} holds no key of type {ty}"
            );
        }
        assert!(
            keys(&img, SUBVOLUMES).iter().any(|k| k.pos.offset == 1),
            "{name}: no root subvolume"
        );
        let r = check::check(&FileDevice::open(&img).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(r.clean(), "{name}: {:?}", r.problems);
    }
}

/// The retained default seed 51 in differential run 38749903964: all
/// snapshot links remain valid, but the key's snapshot field is nonzero.
#[test]
fn a_snapshot_key_with_a_nonzero_snapshot_field_is_reported() {
    let img = scratch("default.img", "snapshot-field-nonzero");
    let mut k = key_at(&img, SNAPSHOTS, KEY_SNAPSHOT, u32::MAX.into());
    assert_eq!(
        k.pos.snapshot, 0,
        "the control key is not snapshot-versioned"
    );
    let value = k.value.clone();
    k.pos.snapshot = u32::MAX - 1;
    {
        let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
        w.insert(SNAPSHOTS, vec![k]).unwrap();
    }
    let changed = keys(&img, SNAPSHOTS)
        .into_iter()
        .find(|k| k.pos.offset == u64::from(u32::MAX) && k.pos.snapshot == u32::MAX - 1)
        .expect("the exact retained mutation reached the image");
    assert_eq!(changed.value, value, "the snapshot links are unchanged");
    assert_reported(
        &img,
        "bkey_snapshot_nonzero",
        "the retained snapshot key with a nonzero snapshot field",
    );
}

/// The measured case (`default.img` seed 19, `block4k.img` seed 19): the
/// tree's whole value made all ones, so it names subvolume 4294967295.
/// And on the kernel image, `sv`'s tree naming a subvolume that is not
/// there.
#[test]
fn a_snapshot_tree_naming_a_missing_subvolume_is_reported() {
    let img = scratch("default.img", "tree-missing-subvol");
    rewrite(&img, SNAPSHOT_TREES, KEY_SNAPSHOT_TREE, 1, |v| {
        put64(v, 0, u64::MAX)
    });
    assert_reported(
        &img,
        "snapshot_tree_to_missing_subvol",
        "a snapshot tree naming subvolume 4294967295",
    );

    let img = scratch("kernel.img", "tree-missing-subvol-kernel");
    rewrite(&img, SNAPSHOT_TREES, KEY_SNAPSHOT_TREE, 2, |v| {
        put32(v, TREE_SUBVOL, 99)
    });
    assert_reported(
        &img,
        "snapshot_tree_to_missing_subvol",
        "sv's snapshot tree naming subvolume 99",
    );
}

/// The measured case (`kernel.img` seed 39): the interior root's second
/// word of skiplist made all ones, so two entries name the root
/// subvolume's snapshot, in another tree. And a leaf whose first entry
/// names its sibling, which is a snapshot but not its ancestor.
#[test]
fn a_skiplist_entry_that_is_no_ancestor_is_reported() {
    let img = scratch("kernel.img", "skiplist-other-tree");
    rewrite(&img, SNAPSHOTS, KEY_SNAPSHOT, INTERIOR.into(), |v| {
        put64(v, SNAP_SKIP12, u64::MAX)
    });
    assert_reported(
        &img,
        "snapshot_bad_skiplist",
        "a root snapshot whose skiplist names 4294967295",
    );

    let img = scratch("kernel.img", "skiplist-sibling");
    rewrite(&img, SNAPSHOTS, KEY_SNAPSHOT, SV.into(), |v| {
        put32(v, SNAP_SKIP0, SNAP)
    });
    assert_reported(
        &img,
        "snapshot_bad_skiplist",
        "sv's snapshot whose skiplist names its sibling",
    );
}

/// The measured case (`kernel-rootsnap.img` seed 28): the third word of
/// subvolume 2's snapshot made 1, so it names subvolume 1 and tree 0,
/// which is not there. And `sv`'s snapshot naming tree 1, which is there, but whose
/// root is the root subvolume's snapshot, not `sv`'s.
#[test]
fn a_snapshot_naming_a_missing_or_wrong_tree_is_reported() {
    let img = scratch("kernel-rootsnap.img", "tree-missing");
    rewrite(&img, SNAPSHOTS, KEY_SNAPSHOT, INTERIOR.into(), |v| {
        put64(v, SNAP_SUBVOL, 1)
    });
    assert_reported(
        &img,
        "snapshot_to_bad_snapshot_tree",
        "a snapshot naming tree 0",
    );

    let img = scratch("kernel.img", "tree-wrong");
    rewrite(&img, SNAPSHOTS, KEY_SNAPSHOT, SV.into(), |v| {
        put32(v, SNAP_TREE, 1)
    });
    assert_reported(
        &img,
        "snapshot_to_bad_snapshot_tree",
        "sv's snapshot naming the root subvolume's tree",
    );
}

/// A parent cycle: the interior root made the child of its own child. The
/// check ends, and the parents of the snapshots in the cycle lead to no
/// root, so none of them is in the tree it names.
#[test]
fn a_parent_cycle_ends_the_walk_and_is_reported() {
    let img = scratch("kernel.img", "parent-cycle");
    rewrite(&img, SNAPSHOTS, KEY_SNAPSHOT, INTERIOR.into(), |v| {
        put32(v, SNAP_PARENT, SV)
    });
    assert_reported(
        &img,
        "snapshot_to_bad_snapshot_tree",
        "snapshots whose parents form a cycle",
    );
}
