//! One field of a reference-made key changed on a private copy, as the nightly
//! differential runs changed it (#568, #569, #615, #623), for the reference
//! checker and this crate's to judge.

use super::common::{fixture, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::Bpos;
use fs_bcachefs::btree;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

const ACCOUNTING: u8 = 20;
const RECONCILE_WORK_PHYS: u8 = 24;
const SET: u8 = 25;
const ACCOUNTING_KEY: u8 = 34;
const ACCT_COMPRESSION: u64 = 4;
const ACCT_SNAPSHOT: u64 = 5;
const ACCT_INUM: u64 = 8;

#[derive(Clone, Copy, Debug)]
pub enum Change {
    Size(u32),
    Snapshot(u32),
    KeyType(u8),
}

#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub set: &'static str,
    btree: u8,
    key_type: u8,
    /// An accounting key's kind, the top byte of its position's inode.
    accounting_kind: Option<u64>,
    change: Change,
    /// What the reference prints, and the kind this checker names for it.
    /// `None`: both call the image clean.
    pub verdict: Option<(&'static str, &'static str)>,
}

pub const CASES: &[Case] = &[
    Case {
        set: "kernel",
        btree: RECONCILE_WORK_PHYS,
        key_type: SET,
        accounting_kind: None,
        change: Change::Size(1),
        verdict: Some(("size != 0", "bkey_size_nonzero")),
    },
    Case {
        set: "kernel",
        btree: ACCOUNTING,
        key_type: ACCOUNTING_KEY,
        accounting_kind: Some(ACCT_COMPRESSION),
        change: Change::Snapshot(1),
        verdict: Some((
            "junk at end of accounting key",
            "accounting_key_junk_at_end",
        )),
    },
    Case {
        set: "kernel",
        btree: ACCOUNTING,
        key_type: ACCOUNTING_KEY,
        accounting_kind: Some(ACCT_SNAPSHOT),
        change: Change::Snapshot(1),
        verdict: Some((
            "junk at end of accounting key",
            "accounting_key_junk_at_end",
        )),
    },
    Case {
        set: "kernel-rootsnap",
        btree: ACCOUNTING,
        key_type: ACCOUNTING_KEY,
        accounting_kind: Some(ACCT_SNAPSHOT),
        change: Change::KeyType(SET),
        verdict: Some(("incorrect value size", "bkey_val_size_nonzero")),
    },
    Case {
        set: "default",
        btree: ACCOUNTING,
        key_type: ACCOUNTING_KEY,
        accounting_kind: Some(ACCT_INUM),
        change: Change::KeyType(50),
        verdict: None,
    },
];

/// A private copy of the reference-made fixture.
pub fn copy(set: &str) -> ScratchImage {
    let image = ScratchDir::new("measured-keys").into_image("planted.img");
    super::common::copy_sparse(fixture(&format!("{set}.img")), &image);
    image
}

/// The copy with the case committed through the writer's raw `insert`, and
/// where the changed key is.
pub fn plant(case: &Case) -> (ScratchImage, Bpos) {
    let image = copy(case.set);
    let device = FileDevice::open(&image).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let mut key = btree::walk(&device, &superblock, case.btree)
        .unwrap()
        .into_iter()
        .find(|k| {
            k.key_type == case.key_type
                && case
                    .accounting_kind
                    .is_none_or(|kind| k.pos.inode >> 56 == kind)
        })
        .unwrap_or_else(|| panic!("{case:?}: the fixture has no such key"));
    drop(device);
    match case.change {
        Change::Size(size) => key.size = size,
        Change::Snapshot(snapshot) => key.pos.snapshot = snapshot,
        Change::KeyType(key_type) => key.key_type = key_type,
    }
    let position = key.pos;
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(case.btree, vec![key]).unwrap();
    drop(writer);
    (image, position)
}
