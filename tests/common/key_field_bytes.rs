//! One field of one reference-made key changed on a private copy of its
//! fixture, for the reference checker and this crate's to judge. The tables
//! are the measurements recorded in docs/clean-room.md, "Snapshot pad,
//! subvolume state and dirent type bytes".

use super::common::{fixture, plant::write_sparse, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

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

pub fn plant_type_byte(set: &str, original: u8, byte: u8) -> ScratchImage {
    plant(
        set,
        btree_id::DIRENTS,
        |k| k.key_type == key_type::DIRENT && k.value.get(8) == Some(&original),
        |v| v[8] = byte,
    )
    .0
}
