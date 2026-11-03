//! One `replicas` accounting key of a reference-made fixture changed, or one
//! added, on a private copy, for the reference checker and this crate's to
//! judge. The tables are the measurements recorded in docs/clean-room.md,
//! "A replicas accounting key at accounting read".

use super::common::{fixture, plant::write_sparse, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

const ACCOUNTING: u8 = 20;
const KEY_ACCOUNTING: u8 = 34;
const ACCT_REPLICAS: u64 = 2;

const BTREE: u8 = 3;
const USER: u8 = 4;
const CACHED: u8 = 5;

pub const SETS: [&str; 2] = ["default", "kernel"];

#[derive(Clone, Copy, Debug)]
pub enum Plant {
    /// The first replicas key moves to the entry, keeping its counter.
    Move,
    /// A new key at the entry, holding this counter, beside the fixture's.
    Add(u64),
}

#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub data_type: u8,
    pub nr_required: u8,
    pub devs: &'static [u8],
    pub plant: Plant,
    /// What the reference prints, and the kind this checker names for it.
    /// `None`: both call the image clean.
    pub verdict: Option<(&'static str, &'static str)>,
}

const fn moved(
    data_type: u8,
    nr_required: u8,
    devs: &'static [u8],
    printed: &'static str,
    kind: &'static str,
) -> Case {
    Case {
        data_type,
        nr_required,
        devs,
        plant: Plant::Move,
        verdict: Some((printed, kind)),
    }
}

const fn added(
    data_type: u8,
    nr_required: u8,
    devs: &'static [u8],
    printed: &'static str,
    kind: &'static str,
) -> Case {
    Case {
        data_type,
        nr_required,
        devs,
        plant: Plant::Add(8),
        verdict: Some((printed, kind)),
    }
}

const fn empty(data_type: u8, nr_required: u8, devs: &'static [u8]) -> Case {
    Case {
        data_type,
        nr_required,
        devs,
        plant: Plant::Add(0),
        verdict: None,
    }
}

const INSUFFICIENT: &str = "insufficient_devices_to_start";
const INVALID_DEVICE: &str = "accounting_to_invalid_device";
const NOT_MARKED: &str = "accounting_replicas_not_marked";
const SHAPE: &str = "accounting_replicas_bad";
const MISMATCH: &str = "accounting_mismatch";

const INVALID_1: &str = "accounting entry points to invalid device 1";
const UNMARKED: &str = "accounting not marked in superblock replicas";
const BAD_REQUIRED: &str = "accounting key replicas entry with bad nr_required";

/// A device the filesystem lacks, or an entry requiring no copies (#422).
pub const DEVICES_AND_COPIES: &[Case] = &[
    moved(
        BTREE,
        1,
        &[1],
        "insufficient devices online (0) for replicas entry btree: 1/1 [1]",
        INSUFFICIENT,
    ),
    moved(
        BTREE,
        1,
        &[254],
        "insufficient devices online (0) for replicas entry btree: 1/1 [254]",
        INSUFFICIENT,
    ),
    moved(
        BTREE,
        1,
        &[1, 2],
        "insufficient devices online (0) for replicas entry btree: 1/2 [1 2]",
        INSUFFICIENT,
    ),
    moved(
        BTREE,
        2,
        &[0, 1, 2],
        "insufficient devices online (1) for replicas entry btree: 2/3 [0 1 2]",
        INSUFFICIENT,
    ),
    added(
        USER,
        1,
        &[1],
        "insufficient devices online (0) for replicas entry user: 1/1 [1]",
        INSUFFICIENT,
    ),
    moved(BTREE, 1, &[0, 1], INVALID_1, INVALID_DEVICE),
    moved(BTREE, 1, &[0, 1, 2], INVALID_1, INVALID_DEVICE),
    moved(BTREE, 0, &[1], INVALID_1, INVALID_DEVICE),
    moved(
        BTREE,
        1,
        &[0, 254],
        "accounting entry points to invalid device 254",
        INVALID_DEVICE,
    ),
    added(CACHED, 1, &[1], INVALID_1, INVALID_DEVICE),
    moved(BTREE, 0, &[0], UNMARKED, NOT_MARKED),
    moved(BTREE, 1, &[255], UNMARKED, NOT_MARKED),
    moved(BTREE, 1, &[0, 255], UNMARKED, NOT_MARKED),
    added(BTREE, 0, &[255], UNMARKED, NOT_MARKED),
    moved(0, 1, &[0], UNMARKED, NOT_MARKED),
    moved(1, 1, &[0], UNMARKED, NOT_MARKED),
    added(0, 0, &[0], UNMARKED, NOT_MARKED),
    added(1, 0, &[0], UNMARKED, NOT_MARKED),
    added(BTREE, 2, &[0, 1], BAD_REQUIRED, SHAPE),
    added(BTREE, 3, &[0, 1, 2], BAD_REQUIRED, SHAPE),
    added(USER, 4, &[0, 1, 2, 3], BAD_REQUIRED, SHAPE),
    Case {
        data_type: BTREE,
        nr_required: 2,
        devs: &[0, 1],
        plant: Plant::Add(0),
        verdict: Some((BAD_REQUIRED, SHAPE)),
    },
];

/// A data type the reference has no name for: it counts what such a key holds
/// as a mismatch at check_allocations (#423).
pub const UNKNOWN_DATA_TYPES: &[Case] = &[
    moved(
        255,
        1,
        &[0],
        "accounting mismatch for replicas (unknown data_type 255): 1/1 [0]",
        MISMATCH,
    ),
    moved(
        12,
        1,
        &[0],
        "accounting mismatch for replicas (unknown data_type 12): 1/1 [0]",
        MISMATCH,
    ),
    moved(
        100,
        1,
        &[0],
        "accounting mismatch for replicas (unknown data_type 100): 1/1 [0]",
        MISMATCH,
    ),
    added(
        255,
        1,
        &[0],
        "accounting mismatch for replicas (unknown data_type 255): 1/1 [0]",
        MISMATCH,
    ),
    added(
        255,
        0,
        &[0],
        "accounting mismatch for replicas (unknown data_type 255): 0/1 [0]",
        MISMATCH,
    ),
    added(
        255,
        1,
        &[255],
        "accounting mismatch for replicas (unknown data_type 255): 1/1 [255]",
        MISMATCH,
    ),
    added(
        255,
        1,
        &[1],
        "insufficient devices online (0) for replicas entry (unknown data_type 255): 1/1 [1]",
        INSUFFICIENT,
    ),
    added(255, 1, &[0, 1], INVALID_1, INVALID_DEVICE),
];

/// The same entries holding nothing: the reference judges none of them.
pub const HOLDING_NOTHING: &[Case] = &[
    empty(BTREE, 1, &[1]),
    empty(BTREE, 1, &[0, 1]),
    empty(BTREE, 0, &[0]),
    empty(BTREE, 1, &[255]),
    empty(255, 1, &[0]),
];

/// The first eight bytes of a replicas entry's position: kind, data type,
/// nr_devs, nr_required, then the devices.
fn entry(case: &Case) -> u64 {
    let mut bytes = [0; 8];
    bytes[0] = ACCT_REPLICAS as u8;
    bytes[1] = case.data_type;
    bytes[2] = case.devs.len() as u8;
    bytes[3] = case.nr_required;
    bytes[4..4 + case.devs.len()].copy_from_slice(case.devs);
    u64::from_be_bytes(bytes)
}

/// A private copy of the reference-made fixture.
pub fn copy(set: &str) -> ScratchImage {
    let image = ScratchDir::new("replicas-accounting").into_image("planted.img");
    write_sparse(
        &std::fs::read(fixture(&format!("{set}.img"))).unwrap(),
        &image,
    );
    image
}

/// The image with the case planted, and where its key is.
pub fn plant(set: &str, case: &Case) -> (ScratchImage, Bpos) {
    let source = fixture(&format!("{set}.img"));
    let image = copy(set);
    let device = FileDevice::open(&source).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let first = btree::walk(&device, &superblock, ACCOUNTING)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == KEY_ACCOUNTING && k.pos.inode >> 56 == ACCT_REPLICAS)
        .unwrap_or_else(|| panic!("{set}: the reference-made fixture has no replicas key"));
    let mut planted = first.clone();
    planted.pos.inode = entry(case);
    let mut keys = vec![planted.clone()];
    match case.plant {
        Plant::Move => keys.push(Bkey {
            key_type: key_type::DELETED,
            value: Vec::new(),
            ..first
        }),
        Plant::Add(counter) => keys[0].value = counter.to_le_bytes().to_vec(),
    }
    keys.sort_by_key(|k| k.pos);
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(ACCOUNTING, keys).unwrap();
    drop(writer);
    (image, planted.pos)
}
