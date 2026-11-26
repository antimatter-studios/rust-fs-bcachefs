//! The changes nightly differential run 45837218401 found the reference
//! reporting and this checker calling clean (#741), each planted on a private
//! copy of a reference-made fixture.
#![cfg(feature = "write")]

mod common;

use common::{copy_sparse, fixture, ScratchDir};
use fs_bcachefs::bkey::Bkey;
use fs_bcachefs::btree;
use fs_bcachefs::check::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

const BACKPOINTERS: u8 = 13;
const ACCOUNTING: u8 = 20;
const DIRENTS: u64 = 2;

/// The first key of `id` in a copy of `set.img` that `wanted` picks, changed
/// by `change` and committed through the writer's raw `insert`: this
/// checker must report `kind`, naming `named`.
fn planted(
    set: &str,
    id: u8,
    wanted: impl Fn(&Bkey) -> bool,
    change: impl FnOnce(&mut Bkey),
    kind: &str,
    named: &str,
) {
    let image = ScratchDir::new("nightly-accounting").into_image(format!("{set}.img"));
    copy_sparse(fixture(&format!("{set}.img")), &image);
    assert!(check(&FileDevice::open(&image).unwrap()).unwrap().clean());
    let device = FileDevice::open(&image).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let mut key = btree::walk(&device, &superblock, id)
        .unwrap()
        .into_iter()
        .find(|k| wanted(k))
        .expect("the fixture has such a key");
    drop(device);
    change(&mut key);
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(id, vec![key]).unwrap();
    drop(writer);
    let report = check(&FileDevice::open(&image).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == kind && p.detail.contains(named)),
        "{set}: {:?}",
        report.problems
    );
}

#[test]
fn a_second_backpointer_one_offset_past_an_extents_own_is_reported() {
    for set in ["default", "block4k"] {
        planted(
            set,
            BACKPOINTERS,
            // An extents btree data backpointer at offset 0 of its sector.
            |k| k.value.get(..3) == Some(&[0, 0, 4]) && k.pos.offset & 0xffff == 0,
            |k| k.pos.offset += 1,
            "backpointer_to_missing_ptr",
            "has no backpointer position",
        );
    }
}

#[test]
fn a_changed_dirents_node_count_is_reported() {
    planted(
        "kernel-rootsnap",
        ACCOUNTING,
        |k| k.pos.inode == (6 << 56) | (DIRENTS << 48),
        |k| k.value[8] ^= 4,
        "accounting_mismatch",
        "btree dirents",
    );
}

#[test]
fn a_changed_per_snapshot_data_sector_counter_is_reported() {
    planted(
        "kernel-rootsnap",
        ACCOUNTING,
        // Snapshot 4294967293's extents counter.
        |k| k.pos.inode == 0x05fd_ffff_ff00_0000 && k.pos.offset == 0,
        |k| k.value[16..24].copy_from_slice(&4654105073906763887u64.to_le_bytes()),
        "accounting_mismatch",
        "snapshot 4294967293 btree extents",
    );
}
