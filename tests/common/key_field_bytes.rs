//! One field of one reference-made key changed on a private copy of its
//! fixture, for the reference checker and this crate's to judge. The tables
//! are the measurements recorded in docs/clean-room.md, "Snapshot pad,
//! subvolume state and dirent type bytes".

use super::common::{
    fixture,
    plant::{restamp_superblock, write_sparse},
    ScratchDir, ScratchImage,
};
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::{Superblock, SB_HEADER_BYTES};
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const SUBVOLUMES: u8 = 8;
const SNAPSHOTS: u8 = 9;
const KEY_SUBVOLUME: u8 = 21;
const KEY_SNAPSHOT: u8 = 22;

#[derive(Clone, Copy, Debug)]
pub enum Edit {
    /// Exclusive-or one byte of the value with a mask.
    Flip(usize, u8),
    /// Store a little-endian word in the value.
    Word(usize, u32),
    /// Keep this many bytes of the value.
    Truncate(usize),
    /// Lengthen the value by this many bytes of this filler.
    Extend(usize, u8),
}

pub const SNAPSHOT_SETS: [&str; 2] = ["default", "kernel-rootsnap"];
pub const SUBVOLUME_SETS: [&str; 3] = ["default", "aged", "kernel"];

/// Changes to a snapshot value, and whether the reference refuses the
/// reserved pad field for them: only bytes 60..64, wherever a value holds them.
pub const SNAPSHOT_PAD: &[(&[Edit], bool)] = &[
    (&[], false),
    (&[Edit::Flip(60, 0x01)], true),
    (&[Edit::Flip(60, 0x80)], true),
    (&[Edit::Flip(61, 0x01)], true),
    (&[Edit::Flip(62, 0x01)], true),
    (&[Edit::Flip(63, 0x80)], true),
    (&[Edit::Word(60, u32::MAX)], true),
    (&[Edit::Flip(40, 0x01)], false),
    (&[Edit::Flip(44, 0x01)], false),
    (&[Edit::Flip(48, 0x01)], false),
    (&[Edit::Flip(52, 0x01)], false),
    (&[Edit::Word(56, 0)], false),
    (&[Edit::Flip(59, 0x80)], false),
    (&[Edit::Truncate(56)], false),
    (&[Edit::Extend(8, 0xff)], false),
    (&[Edit::Extend(16, 0xff)], false),
    (&[Edit::Extend(8, 0), Edit::Flip(63, 0x80)], true),
];

/// Changes to a subvolume value, and whether the reference calls its state
/// unset for them: a whole state word of zero at bytes 40..44, or a value of
/// at least 16 bytes that ends before it. Below 16 it refuses the value's size.
pub const SUBVOLUME_STATE: &[(&[Edit], bool)] = &[
    (&[], false),
    (&[Edit::Word(40, 0)], true),
    (&[Edit::Word(40, 0), Edit::Extend(8, 0xff)], true),
    (&[Edit::Truncate(40)], true),
    (&[Edit::Truncate(32)], true),
    (&[Edit::Truncate(24)], true),
    (&[Edit::Truncate(16)], true),
    (&[Edit::Truncate(8)], false),
    (&[Edit::Truncate(0)], false),
    (&[Edit::Flip(40, 0x01)], false),
    (&[Edit::Flip(43, 0x80)], false),
    (&[Edit::Word(40, 1)], false),
    (&[Edit::Word(40, u32::MAX)], false),
    (&[Edit::Flip(44, 0x01)], false),
    (&[Edit::Flip(24, 0x01)], false),
    (&[Edit::Extend(8, 0xff)], false),
];

/// One changed value, what the reference prints for it and the kind this
/// crate reports. `None` is a value neither of them has a finding for.
pub type WordCase = (&'static [Edit], Option<(&'static str, &'static str)>);

/// The words the reference key editor stores for each name it accepts: a
/// snapshot's state is live, will_delete, no_keys or deleted, and a subvolume's
/// live, unlinked or deleted.
const WILL_DELETE: u32 = 0x3316_a8d2;
const NO_KEYS: u32 = 0x372b_5a01;
const SNAP_DELETED: u32 = 0x7cd4_a225;
const UNLINKED: u32 = 0x2d35_8e8f;

const SNAP_FLIP: &str = "snapshot_state_bitflip";
const SNAP_BAD: &str = "snapshot_state_bad";

/// Changes to a snapshot's state word at bytes 56..60, on a snapshot with a
/// live subvolume and keys accounted to it.
pub const SNAPSHOT_STATE_WORDS: &[WordCase] = &[
    (&[], None),
    (
        &[Edit::Word(56, 0)],
        Some(("snapshot state unset", SNAP_BAD)),
    ),
    (
        &[Edit::Truncate(56)],
        Some(("snapshot state unset", SNAP_BAD)),
    ),
    (
        &[Edit::Truncate(24)],
        Some(("snapshot state unset", SNAP_BAD)),
    ),
    (
        &[Edit::Flip(56, 0x01)],
        Some(("is a 1-bit flip of live", SNAP_FLIP)),
    ),
    (
        &[Edit::Flip(59, 0x80)],
        Some(("is a 1-bit flip of live", SNAP_FLIP)),
    ),
    (
        &[Edit::Flip(56, 0x03)],
        Some(("is a 2-bit flip of live", SNAP_FLIP)),
    ),
    (
        &[Edit::Flip(56, 0x07)],
        Some(("is 3 bits from live", SNAP_BAD)),
    ),
    (
        &[Edit::Flip(56, 0x3f)],
        Some(("is 6 bits from live", SNAP_BAD)),
    ),
    (&[Edit::Flip(56, 0x7f)], Some(("is garbage", SNAP_BAD))),
    (&[Edit::Word(56, 1)], Some(("is garbage", SNAP_BAD))),
    (&[Edit::Word(56, u32::MAX)], Some(("is garbage", SNAP_BAD))),
    (
        &[Edit::Word(56, WILL_DELETE ^ 1)],
        Some(("is a 1-bit flip of will_delete", SNAP_FLIP)),
    ),
    (
        &[Edit::Word(56, NO_KEYS ^ 1)],
        Some(("is a 1-bit flip of no_keys", SNAP_FLIP)),
    ),
    (
        &[Edit::Word(56, SNAP_DELETED ^ 1)],
        Some(("is a 1-bit flip of deleted", SNAP_FLIP)),
    ),
    (
        &[Edit::Word(56, WILL_DELETE)],
        Some((
            "marked will_delete but its subvolume is live",
            "snapshot_deleted_but_subvol_live",
        )),
    ),
    (
        &[Edit::Word(56, NO_KEYS)],
        Some((
            "snapshot_deleted_but_has_data",
            "snapshot_deleted_but_has_data",
        )),
    ),
    (
        &[Edit::Word(56, SNAP_DELETED)],
        Some((
            "snapshot_deleted_but_has_data",
            "snapshot_deleted_but_has_data",
        )),
    ),
];

/// The sets with an interior snapshot, which names no subvolume.
pub const INTERIOR_SNAPSHOT_SETS: [&str; 2] = ["kernel", "kernel-rootsnap"];

/// Changes to an interior snapshot's state word.
pub const INTERIOR_SNAPSHOT_WORDS: &[WordCase] = &[
    (&[], None),
    (
        &[Edit::Word(56, WILL_DELETE)],
        Some((
            "marked will_delete but has two live children",
            "snapshot_deleted_has_live_children",
        )),
    ),
];

const SUBVOL_FLIP: &str = "subvol_state_bitflip";
const SUBVOL_BAD: &str = "subvol_state_bad";
const SUBVOL_INVALID: &str = "subvol_state_invalid";

/// Changes to a subvolume's state word at bytes 40..44, other than to zero.
pub const SUBVOLUME_STATE_WORDS: &[WordCase] = &[
    (&[], None),
    (
        &[Edit::Flip(40, 0x01)],
        Some(("is a 1-bit flip of live", SUBVOL_FLIP)),
    ),
    (
        &[Edit::Flip(43, 0x80)],
        Some(("is a 1-bit flip of live", SUBVOL_FLIP)),
    ),
    (
        &[Edit::Flip(40, 0x03)],
        Some(("is a 2-bit flip of live", SUBVOL_FLIP)),
    ),
    (
        &[Edit::Flip(40, 0x07)],
        Some(("is 3 bits from live", SUBVOL_BAD)),
    ),
    (
        &[Edit::Flip(40, 0x3f)],
        Some(("is 6 bits from live", SUBVOL_BAD)),
    ),
    (
        &[Edit::Flip(40, 0x7f)],
        Some(("has invalid state", SUBVOL_INVALID)),
    ),
    (
        &[Edit::Word(40, 1)],
        Some(("has invalid state", SUBVOL_INVALID)),
    ),
    (
        &[Edit::Word(40, u32::MAX)],
        Some(("has invalid state", SUBVOL_INVALID)),
    ),
];

/// Words near a subvolume's other states, on the one fixture measured.
pub const OTHER_STATE_SETS: [&str; 1] = ["default"];
pub const SUBVOLUME_OTHER_STATE_WORDS: &[WordCase] = &[
    (
        &[Edit::Word(40, UNLINKED ^ 1)],
        Some(("is a 1-bit flip of unlinked", SUBVOL_FLIP)),
    ),
    (
        &[Edit::Word(40, 0x3c6f_2ccc)],
        Some(("is 3 bits from deleted", SUBVOL_BAD)),
    ),
];

const NONZERO_PAD: Option<(&str, &str)> =
    Some(("reserved pad field nonzero", "subvol_pad_nonzero"));

/// Changes to a subvolume's pad word at bytes 44..48, fixed there whatever
/// the value's length, and to the bytes after it.
pub const SUBVOLUME_PAD_WORDS: &[WordCase] = &[
    (&[Edit::Flip(44, 0x01)], NONZERO_PAD),
    (&[Edit::Flip(45, 0x01)], NONZERO_PAD),
    (&[Edit::Flip(46, 0x01)], NONZERO_PAD),
    (&[Edit::Flip(47, 0x80)], NONZERO_PAD),
    (&[Edit::Word(44, u32::MAX)], NONZERO_PAD),
    (&[Edit::Flip(47, 0x80), Edit::Extend(8, 0)], NONZERO_PAD),
    (&[Edit::Extend(8, 0xff)], None),
    (&[Edit::Extend(16, 0xff)], None),
];

/// Below format version 1.39 the reference has no finding for these: its output
/// is the same as on the same copy with the value unchanged.
pub const OLDER_SNAPSHOT_QUIET: &[&[Edit]] = &[
    &[Edit::Word(56, 0)],
    &[Edit::Truncate(56)],
    &[Edit::Flip(56, 0x07)],
    &[Edit::Flip(56, 0x3f)],
    &[Edit::Word(56, 1)],
];
pub const OLDER_SUBVOLUME_QUIET: &[&[Edit]] = &[
    &[Edit::Word(40, 0)],
    &[Edit::Truncate(40)],
    &[Edit::Truncate(16)],
    &[Edit::Flip(40, 0x07)],
    &[Edit::Flip(40, 0x3f)],
];

/// And it judges these as at 1.39.
pub const OLDER_SNAPSHOT_JUDGED: &[WordCase] = &[
    (
        &[Edit::Flip(56, 0x01)],
        Some(("is a 1-bit flip of live", SNAP_FLIP)),
    ),
    (
        &[Edit::Flip(56, 0x03)],
        Some(("is a 2-bit flip of live", SNAP_FLIP)),
    ),
];
pub const OLDER_SUBVOLUME_JUDGED: &[WordCase] = &[
    (
        &[Edit::Flip(40, 0x01)],
        Some(("is a 1-bit flip of live", SUBVOL_FLIP)),
    ),
    (
        &[Edit::Flip(40, 0x03)],
        Some(("is a 2-bit flip of live", SUBVOL_FLIP)),
    ),
    (
        &[Edit::Word(40, 1)],
        Some(("has invalid state", SUBVOL_INVALID)),
    ),
    (&[Edit::Flip(44, 0x01)], NONZERO_PAD),
];

/// The minor versions a copy's superblock is lowered to.
pub const OLDER_MINORS: [u16; 6] = [0, 20, 33, 35, 36, 38];

/// Reference-made dirents by fixture and original type byte: a file, a
/// directory, a symlink, casefolded entries and entries naming a subvolume.
pub const TYPE_BYTE_DIRENTS: [(&str, u8); 7] = [
    ("default", 0x08),
    ("lz4", 0x08),
    ("default", 0x04),
    ("aged", 0x0a),
    ("casefold", 0x88),
    ("casefold", 0x84),
    ("kernel", 0x10),
];

/// The bytes to try on one dirent: every byte on the regular file the
/// differential run found, a sample of each kind of disagreement elsewhere.
pub fn type_bytes(set: &str, original: u8) -> Vec<u8> {
    if (set, original) == ("default", 0x08) {
        return (0..=255).collect();
    }
    let layout = original & 0x80;
    let mut bytes = vec![
        original,
        original ^ 0x20,
        original ^ 0x40,
        original ^ 0x60,
        original ^ 0x80,
        original ^ 0x01,
        original ^ 0x02,
        original ^ 0x10,
        layout,
        layout | 0x1f,
        layout | 0x30,
    ];
    bytes.sort_unstable();
    bytes.dedup();
    bytes
}

/// Whether the reference calls a dirent clean with this type byte. It reads
/// the type from bits 0..5 and the casefolded layout from bit 7, and does not
/// read bits 5 and 6.
pub fn type_byte_is_clean(original: u8, byte: u8) -> bool {
    const IGNORED: u8 = 0x60;
    byte & !IGNORED == original & !IGNORED
}

fn apply(value: &mut Vec<u8>, edits: &[Edit]) {
    for edit in edits {
        match *edit {
            Edit::Flip(at, mask) => value[at] ^= mask,
            Edit::Word(at, word) => value[at..at + 4].copy_from_slice(&word.to_le_bytes()),
            Edit::Truncate(len) => value.truncate(len),
            Edit::Extend(by, filler) => value.resize(value.len() + by, filler),
        }
    }
}

fn plant(
    set: &str,
    id: u8,
    pick: impl Fn(&Bkey) -> bool,
    edit: impl FnOnce(&mut Vec<u8>),
) -> (ScratchImage, Bpos) {
    let source = fixture(&format!("{set}.img"));
    let image = ScratchDir::new("key-field-bytes").into_image("planted.img");
    write_sparse(&std::fs::read(&source).unwrap(), &image);
    let device = FileDevice::open(&source).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let mut key = btree::walk(&device, &superblock, id)
        .unwrap()
        .into_iter()
        .find(pick)
        .unwrap_or_else(|| panic!("{set}: the reference-made fixture has no such key"));
    edit(&mut key.value);
    let position = key.pos;
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(id, vec![key]).unwrap();
    drop(writer);
    (image, position)
}

pub fn plant_snapshot(set: &str, edits: &[Edit]) -> (ScratchImage, Bpos) {
    plant(
        set,
        SNAPSHOTS,
        |k| k.key_type == KEY_SNAPSHOT,
        |v| apply(v, edits),
    )
}

pub fn plant_subvolume(set: &str, edits: &[Edit]) -> (ScratchImage, Bpos) {
    plant(
        set,
        SUBVOLUMES,
        |k| k.key_type == KEY_SUBVOLUME,
        |v| apply(v, edits),
    )
}

pub fn plant_interior_snapshot(set: &str, edits: &[Edit]) -> (ScratchImage, Bpos) {
    plant(
        set,
        SNAPSHOTS,
        |k| k.key_type == KEY_SNAPSHOT && k.value[8..16] != [0; 8],
        |v| apply(v, edits),
    )
}

/// Every copy of the superblock's version words lowered to 1.`minor`: the
/// version and its minimum, and the three later words holding versions, which
/// the reference refuses to find above the version (docs/clean-room.md).
pub fn lower_version(image: &Path, minor: u16) {
    const VERSION_WORDS: [usize; 5] = [0x10, 0x12, 0xb8, 0xbc, 0xbe];
    let sb = Superblock::read(&FileDevice::open(image).unwrap()).unwrap();
    let version = (1u16 << 10) | minor;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .unwrap();
    let mut sectors = sb.layout.sb_offsets.clone();
    if !sectors.contains(&8) {
        sectors.push(8);
    }
    for sector in sectors {
        let mut bytes = vec![0; SB_HEADER_BYTES + sb.u64s as usize * 8];
        file.seek(SeekFrom::Start(sector * 512)).unwrap();
        file.read_exact(&mut bytes).unwrap();
        for at in VERSION_WORDS {
            let word = u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
            bytes[at..at + 2].copy_from_slice(&word.min(version).to_le_bytes());
        }
        restamp_superblock(&mut bytes);
        file.seek(SeekFrom::Start(sector * 512)).unwrap();
        file.write_all(&bytes).unwrap();
    }
    file.sync_all().unwrap();
}

pub fn plant_type_byte(set: &str, original: u8, byte: u8) -> ScratchImage {
    plant(
        set,
        btree_id::DIRENTS,
        |k| k.key_type == key_type::DIRENT && k.value.get(8) == Some(&original),
        |v| v[8] = byte,
    )
    .0
}
