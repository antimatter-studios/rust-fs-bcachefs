//! A filesystem the reference formatter wrote and nothing has started (#505).
//!
//! Below the newest format version the formatter leaves the first start to do
//! the rest: such an image holds three superblock copies and nothing else. The
//! reference checker starts it in memory and accepts it. All there is to judge
//! is the superblock, and so these are its rules, each measured with the
//! reference checker on copies of what the formatter wrote
//! (docs/clean-room.md, "Images formatted and never started"; the table in
//! `tests/common/never_started.rs`).
//!
//! No journal or btree is read: the reference reads neither here, and what the
//! device holds outside the copies is whatever it held before it was formatted.

use super::Report;
use crate::superblock::{Superblock, SB_HEADER_BYTES};
use fs_core::BlockRead;

const MAJOR_MAX: u16 = 1;
/// The oldest version and minimum the reference reads: 0.9.
const VERSION_FLOOR: u16 = 9;
/// The newest incompatible version the reference allows to be in use, 1.39.
/// Reference tools of a later format version will allow more.
const INCOMPAT_IN_USE_MAX: u64 = (1 << 10) | 39;
const FEATURE_BITS_KNOWN: u32 = 21;
const TIME_PRECISION_MAX: u32 = 1_000_000_000;
const BLOCK_SIZE_MAX: u64 = 64;
const LAYOUT_SIZE_BITS_MAX: u8 = 19;
const NBUCKETS_MIN: u64 = 512;
/// One past the most buckets the reference allows.
const NBUCKETS_END: u64 = (1 << 31) - 64;
const BITMAP_SHIFT_END: u8 = 58;
/// The reference runs out of room between 96.9% and 97.3% of a 256 MiB device,
/// and between 93.8% and 97.7% of a 512 MiB one.
const GC_RESERVE_PERCENT_OF_DEVICE: u128 = 97;

const FIELD_MEMBERS_V1: u32 = 1;
const FIELD_MEMBERS_V2: u32 = 11;
const FIELD_EXT: u32 = 13;
/// The member size the v1 field has, its entries starting after the field's
/// header. They are laid out as the first bytes of a v2 entry.
const MEMBER_V1_BYTES: usize = 56;
/// The word of the ext field holding the options named below.
const EXT_OPTIONS: usize = 88;

/// A bit field of the superblock's flag words and the values it may hold.
struct FlagOption {
    name: &'static str,
    word: usize,
    shift: u32,
    bits: u32,
    min: u64,
    max: u64,
}

const fn flag(
    name: &'static str,
    word: usize,
    shift: u32,
    bits: u32,
    min: u64,
    max: u64,
) -> FlagOption {
    FlagOption {
        name,
        word,
        shift,
        bits,
        min,
        max,
    }
}

/// `metadata_replicas` is 1: above that the reference cannot start one device,
/// even at a durability of 2.
const FLAG_OPTIONS: &[FlagOption] = &[
    flag("errors", 0, 8, 4, 0, 3),
    flag("gc_reserve_percent", 0, 28, 5, 5, 20),
    flag("root_reserve_percent", 0, 33, 7, 0, 100),
    flag("metadata_checksum", 0, 40, 4, 0, 3),
    flag("data_checksum", 0, 44, 4, 0, 3),
    flag("metadata_replicas", 0, 48, 4, 1, 1),
    flag("data_replicas", 0, 52, 4, 1, 4),
    flag("str_hash", 1, 0, 4, 0, 2),
    flag("compression", 1, 4, 4, 0, 3),
    flag("encryption_type", 1, 10, 4, 0, 0),
    flag("encoded_extent_max", 1, 14, 6, 3, 12),
    flag("background_compression", 2, 0, 4, 0, 3),
    flag("erasure_code", 3, 0, 16, 0, 1),
    flag("version_upgrade", 4, 54, 2, 0, 2),
    flag("shard_inode_numbers_bits", 6, 0, 4, 0, 8),
    flag("write_error_timeout", 6, 4, 10, 0, 300),
    flag("checksum_err_retry_nr", 6, 14, 6, 0, 32),
    flag("scrub_recent_journal_entries", 6, 48, 2, 0, 2),
    flag("ec_max_data_blocks", 6, 50, 8, 0, 15),
    flag("write_degraded", 6, 60, 2, 0, 2),
];

/// The same, in the ext field's word: name, shift, bits, the most it may hold.
const EXT_OPTION_RANGES: &[(&str, u32, u32, u64)] = &[
    ("ec_stripe_buf_limit", 20, 6, 25),
    ("scrub_journal_max_rewind_secs", 26, 12, 3600),
    ("journal_rewind_discard_buffer_percent", 38, 4, 10),
    ("missing_dev_timeout", 49, 12, 3600),
];

fn bits(word: u64, shift: u32, width: u32) -> u64 {
    (word >> shift) & ((1 << width) - 1)
}

fn le16_at(b: &[u8], at: usize) -> u64 {
    u64::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

/// Judge `sb`, read from `sector` of `dev`, which [`Superblock::never_started`]
/// says is one.
pub(super) fn judge(sb: &Superblock, sector: u64, dev: &dyn BlockRead, r: &mut Report) {
    header(sb, sector, r);
    layout(sb, r);
    flag_options(sb, r);
    ext_options(sb, r);
    member(sb, dev, r);
}

fn header(sb: &Superblock, sector: u64, r: &mut Report) {
    let (version, minimum) = (sb.version.0, sb.version_min.0);
    if sb.version.major() > MAJOR_MAX || version < VERSION_FLOOR {
        r.add("invalid_sb_version", format!("version {version:#x}"));
    }
    if minimum < VERSION_FLOOR || minimum > version {
        r.add(
            "invalid_sb_version",
            format!("minimum version {minimum:#x}, of version {version:#x}"),
        );
    }
    let in_use = bits(sb.flags[5], 32, 16);
    if in_use > INCOMPAT_IN_USE_MAX {
        r.add(
            "invalid_sb_features",
            format!("incompatible version {in_use:#x} in use"),
        );
    }
    if sb.offset != sector {
        r.add(
            "invalid_sb_offset",
            format!("says sector {}, read from {sector}", sb.offset),
        );
    }
    if sb.uuid == [0; 16] || sb.user_uuid == [0; 16] {
        r.add("invalid_sb_uuid", "a filesystem uuid is zero");
    }
    if sb.dev_idx >= sb.nr_devices {
        r.add(
            "invalid_sb_dev_idx",
            format!("device {} of {}", sb.dev_idx, sb.nr_devices),
        );
    }
    if !(1..=TIME_PRECISION_MAX).contains(&sb.time_precision) {
        r.add(
            "invalid_sb_time_precision",
            format!("time precision {}", sb.time_precision),
        );
    }
    // The reference's field is six bits wide. The reader looks at its low four,
    // which is all it knows: a higher bit reaches here as no checksum at all.
    let checksum = bits(sb.flags[0], 2, 6);
    if checksum >= 16 {
        r.add(
            "invalid_sb_checksum",
            format!("unknown checksum type {checksum}"),
        );
    }
    if sb.features[1] != 0 || sb.features[0] >> FEATURE_BITS_KNOWN != 0 {
        r.add(
            "invalid_sb_features",
            format!(
                "incompatible features {:#x} {:#x}",
                sb.features[0], sb.features[1]
            ),
        );
    }
}

fn layout(sb: &Superblock, r: &mut Report) {
    let l = &sb.layout;
    if l.layout_type != 0 {
        r.add("invalid_sb_layout", format!("type {}", l.layout_type));
    }
    if l.sb_offsets.is_empty() {
        r.add("invalid_sb_layout", "no superblocks");
    }
    if l.sb_max_size_bits > LAYOUT_SIZE_BITS_MAX {
        r.add(
            "invalid_sb_layout",
            format!("size bits {}", l.sb_max_size_bits),
        );
        return;
    }
    let sectors = 1u64 << l.sb_max_size_bits;
    let bytes = SB_HEADER_BYTES as u64 + u64::from(sb.u64s) * 8;
    if bytes > sectors * 512 {
        r.add(
            "invalid_sb_layout",
            format!("{bytes} bytes in a layout of {sectors} sectors a copy"),
        );
    }
    if l.sb_offsets
        .windows(2)
        .any(|w| w[0].saturating_add(sectors) > w[1])
    {
        r.add("invalid_sb_layout", "superblock copies overlap");
    }
}

fn flag_options(sb: &Superblock, r: &mut Report) {
    for o in FLAG_OPTIONS {
        let value = bits(sb.flags[o.word], o.shift, o.bits);
        if !(o.min..=o.max).contains(&value) {
            r.add(
                "invalid_sb_option",
                format!("{} is {value}, of {}..={}", o.name, o.min, o.max),
            );
        }
    }
    // A compression level means nothing without a type to set it for. The
    // levels sit apart from the types they belong to.
    let levels = [
        (
            "compression",
            bits(sb.flags[4], 56, 4),
            bits(sb.flags[1], 4, 4),
        ),
        (
            "background_compression",
            bits(sb.flags[4], 60, 4),
            bits(sb.flags[2], 0, 4),
        ),
    ];
    for (name, level, kind) in levels {
        if level != 0 && kind == 0 {
            r.add(
                "invalid_sb_option",
                format!("{name} level {level} with no compression type"),
            );
        }
    }
    let block = u64::from(sb.block_size);
    if !block.is_power_of_two() || block > BLOCK_SIZE_MAX {
        r.add("invalid_sb_option", format!("block_size {block} sectors"));
    }
    let node = u64::from(sb.btree_node_size());
    if !node.is_power_of_two() {
        r.add(
            "invalid_sb_option",
            format!("btree_node_size {node} sectors"),
        );
    }
}

fn ext_options(sb: &Superblock, r: &mut Report) {
    let Some(word) = sb
        .field(FIELD_EXT)
        .and_then(|f| f.body.get(EXT_OPTIONS..EXT_OPTIONS + 8))
    else {
        return;
    };
    let word = u64::from_le_bytes(word.try_into().unwrap());
    for (name, shift, width, max) in EXT_OPTION_RANGES {
        let value = bits(word, *shift, *width);
        if value > *max {
            r.add(
                "invalid_sb_option",
                format!("{name} is {value}, of at most {max}"),
            );
        }
    }
}

/// The one member's bytes: from `members_v2`, or from `members_v1` where there
/// is no `members_v2`, which the reference takes as well.
fn member_bytes<'a>(sb: &'a Superblock, r: &mut Report) -> Option<&'a [u8]> {
    if let Some(field) = sb.field(FIELD_MEMBERS_V2) {
        let b = &field.body;
        let size = b.get(..2).map_or(0, |_| le16_at(b, 0)) as usize;
        if size < MEMBER_V1_BYTES {
            r.add("invalid_sb_members", format!("member size {size}"));
            return None;
        }
        let member = b.get(8..8 + size);
        if member.is_none() {
            r.add(
                "invalid_sb_members",
                format!("member size {size} in a field of {}", b.len()),
            );
        }
        return member;
    }
    let Some(field) = sb.field(FIELD_MEMBERS_V1) else {
        r.add("invalid_sb_members_missing", "no member field");
        return None;
    };
    let member = field.body.get(..MEMBER_V1_BYTES);
    if member.is_none() {
        r.add(
            "invalid_sb_members",
            format!("members_v1 of {} bytes", field.body.len()),
        );
    }
    member
}

fn member(sb: &Superblock, dev: &dyn BlockRead, r: &mut Report) {
    let Some(m) = member_bytes(sb, r) else {
        return;
    };
    let nbuckets = u64::from_le_bytes(m[16..24].try_into().unwrap());
    let first_bucket = le16_at(m, 24);
    let bucket_size = le16_at(m, 26);
    let mut wrong = |what: String| r.add("invalid_sb_members", what);
    if m[..16] == [0; 16] {
        wrong("the member's uuid is zero".into());
    }
    if nbuckets.saturating_sub(first_bucket) < NBUCKETS_MIN {
        wrong(format!(
            "{nbuckets} buckets from {first_bucket}, of at least {NBUCKETS_MIN}"
        ));
    }
    if nbuckets >= NBUCKETS_END {
        wrong(format!("{nbuckets} buckets"));
    }
    if first_bucket != 0 {
        wrong(format!("first bucket {first_bucket}"));
    }
    if bucket_size < u64::from(sb.block_size) {
        wrong(format!(
            "bucket of {bucket_size} sectors, block of {}",
            sb.block_size
        ));
    }
    if bucket_size < u64::from(sb.btree_node_size()) {
        wrong(format!(
            "bucket of {bucket_size} sectors, btree node of {}",
            sb.btree_node_size()
        ));
    }
    if m[28] >= BITMAP_SHIFT_END {
        wrong(format!("btree bitmap shift {}", m[28]));
    }
    // The member's flags: its state in the low bits of byte 40, the data it
    // may hold from byte 42, its durability plus one in byte 43.
    if m[40] & 0x0f != 0 {
        wrong(format!("state {}", m[40] & 0x0f));
    }
    if m[42] & 0b110 != 0b110 {
        wrong("the device may not hold both journal and btree".into());
    }
    if (m[43] >> 4) & 0b11 == 1 {
        wrong("durability 0".into());
    }
    let sectors = u128::from(nbuckets) * u128::from(bucket_size);
    let device = u128::from(dev.size_bytes() / 512);
    if sectors > device {
        r.add(
            "device_size_too_small",
            format!("{sectors} sectors on a device of {device}"),
        );
    }
    // In sectors, and what remains has to hold the filesystem's first metadata.
    let reserve = u128::from(bits(sb.flags[2], 4, 55));
    if reserve * 100 >= sectors * GC_RESERVE_PERCENT_OF_DEVICE {
        r.add(
            "invalid_sb_option",
            format!("gc_reserve_bytes of {reserve} sectors on a device of {sectors}"),
        );
    }
}
