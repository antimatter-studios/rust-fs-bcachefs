//! What the reference checker said of the superblock of a filesystem the
//! reference formatter started, one edit at a time (#533, docs/clean-room.md
//! "Superblock damage on a started filesystem").
//!
//! Every edit goes to a copy whose checksum type has been set to none, so
//! nothing needs restamping, and every copy has its checksum type set to none,
//! so that which copy is read is the only thing a copy's edit can change. The
//! unit tier holds this checker to the table on `tests/data/started-image-1.39.bin`;
//! the guest (`tests/check_oracle_started_superblock.rs`) holds the reference
//! checker to it as well, on an image it formats there.

#![allow(dead_code)]

use super::never_started::{
    add_field, drop_fields, ext, field_at, member, set_field, u16_at, u32_at, u64_at, Case, Edit,
    On, COPIES, FIELD_EXT, FIELD_MEMBERS_V1, FIELD_MEMBERS_V2,
};

const HEADER: usize = 0x2f0;
const U64S: usize = 0x7c;
const VERSION: usize = 0x10;
const VERSION_MIN: usize = 0x12;
const INCOMPAT_IN_USE: usize = 0xbc;
const FEATURES: usize = 0xd0;
/// What the formatter sets in the first feature word of a started image.
const FEATURES_FORMATTED: u64 = 0xfba80;

const fn all(name: &'static str, reported: bool, edit: Edit) -> Case {
    Case {
        name,
        on: On::All,
        edit,
        reported,
    }
}

const fn one(name: &'static str, copy: usize, reported: bool, edit: Edit) -> Case {
    Case {
        name,
        on: On::Copy(copy),
        edit,
        reported,
    }
}

/// A version as the superblock holds it.
const fn v(major: u16, minor: u16) -> u16 {
    (major << 10) | minor
}

macro_rules! version {
    ($name:literal, $reported:literal, $value:expr) => {
        all($name, $reported, |b| u16_at(b, VERSION, $value))
    };
}

macro_rules! minimum {
    ($name:literal, $reported:literal, $value:expr) => {
        all($name, $reported, |b| u16_at(b, VERSION_MIN, $value))
    };
}

macro_rules! lowered {
    ($name:literal, $reported:literal, $value:expr) => {
        all($name, $reported, |b| {
            u16_at(b, VERSION, $value);
            u16_at(b, VERSION_MIN, $value);
        })
    };
}

macro_rules! lowered_with_use {
    ($name:literal, $reported:literal, $value:expr) => {
        all($name, $reported, |b| {
            u16_at(b, VERSION, $value);
            u16_at(b, VERSION_MIN, $value);
            u16_at(b, INCOMPAT_IN_USE, $value);
        })
    };
}

macro_rules! in_use {
    ($name:literal, $reported:literal, $value:expr) => {
        all($name, $reported, |b| u16_at(b, INCOMPAT_IN_USE, $value))
    };
}

macro_rules! offset {
    ($name:literal, $reported:literal, $value:expr) => {
        all($name, $reported, |b| u64_at(b, 0x68, $value))
    };
}

macro_rules! precision {
    ($name:literal, $reported:literal, $value:expr) => {
        all($name, $reported, |b| u32_at(b, 0x8c, $value))
    };
}

macro_rules! feature {
    ($name:literal, $reported:literal, $value:expr) => {
        all($name, $reported, |b| u64_at(b, FEATURES, $value))
    };
}

macro_rules! opt {
    ($name:literal, $reported:literal, $word:literal, $shift:literal, $bits:literal, $value:expr) => {
        all($name, $reported, |b| {
            set_field(b, $word, $shift, $bits, $value)
        })
    };
}

macro_rules! ext_opt {
    ($name:literal, $reported:literal, $bit:literal, $bits:literal, $value:expr) => {
        all($name, $reported, |b| ext(b, $bit, $bits, $value))
    };
}

macro_rules! layout_at {
    ($name:literal, $reported:literal, $at:literal, $value:literal) => {
        all($name, $reported, |b| b[$at] = $value)
    };
}

macro_rules! shift {
    ($name:literal, $reported:literal, $value:literal) => {
        all($name, $reported, |b| {
            let at = member(b) + 28;
            b[at] = $value;
        })
    };
}

/// One sector count of a 256 MiB device's 524,288, in tenths of a percent.
const fn reserve(tenths: u64) -> u64 {
    tenths * 524_288 / 1000
}

/// Every case, measured with reference checker 1.39.7 on 2026-11-04. A verdict
/// is the same on all of its repeats.
pub const CASES: &[Case] = &[
    all("unchanged", false, |_| {}),
    all("sequence 1", false, |b| u64_at(b, 0x70, 1)),
    all("compat features set", false, |b| u64_at(b, 0xe0, 0x7f)),
    all("oldest on disk 2.0", false, |b| u16_at(b, 0xbe, v(2, 0))),
    all("an unknown field", false, |b| add_field(b, 99, &[0; 8])),
    // The version.
    version!("version 0.0", true, v(0, 0)),
    version!("version 0.9", true, v(0, 9)),
    version!("version 1.0", true, v(1, 0)),
    version!("version 1.38, below the minimum", true, v(1, 38)),
    version!("version 1.39", false, v(1, 39)),
    version!("version 1.40", false, v(1, 40)),
    version!("version 1.1023", false, v(1, 1023)),
    version!("version 2.0", true, v(2, 0)),
    version!("version 3.0", true, v(3, 0)),
    // The minimum: 0.14 is the oldest a started filesystem may hold.
    minimum!("minimum 0.0", true, v(0, 0)),
    minimum!("minimum 0.8", true, v(0, 8)),
    minimum!("minimum 0.9", true, v(0, 9)),
    minimum!("minimum 0.13", true, v(0, 13)),
    minimum!("minimum 0.14", false, v(0, 14)),
    minimum!("minimum 0.255", false, v(0, 255)),
    minimum!("minimum 1.0", false, v(1, 0)),
    minimum!("minimum 1.38", false, v(1, 38)),
    minimum!("minimum 1.39", false, v(1, 39)),
    minimum!("minimum 1.40", true, v(1, 40)),
    minimum!("minimum 1.1023", true, v(1, 1023)),
    minimum!("minimum 2.0", true, v(2, 0)),
    // The version and the minimum together: they may not fall below the
    // incompatible version in use, which the formatter leaves at 1.36.
    lowered!("version and minimum 1.38", false, v(1, 38)),
    lowered!("version and minimum 1.37", false, v(1, 37)),
    lowered!("version and minimum 1.36", false, v(1, 36)),
    lowered!("version and minimum 1.35", true, v(1, 35)),
    lowered!("version and minimum 1.30", true, v(1, 30)),
    lowered!("version and minimum 1.0", true, v(1, 0)),
    lowered!("version and minimum 0.14", true, v(0, 14)),
    lowered!("version and minimum 0.9", true, v(0, 9)),
    lowered_with_use!("all three versions 1.38", false, v(1, 38)),
    // The incompatible version in use.
    in_use!("incompat in use 0.0", false, v(0, 0)),
    in_use!("incompat in use 1.0", false, v(1, 0)),
    in_use!("incompat in use 1.39", false, v(1, 39)),
    in_use!("incompat in use 1.40", true, v(1, 40)),
    in_use!("incompat in use 1.1023", true, v(1, 1023)),
    in_use!("incompat in use 2.0", true, v(2, 0)),
    // Where the copy says it is. On a tie the reference believes the last copy.
    offset!("offset 0", true, 0),
    offset!("offset 7", true, 7),
    offset!("offset 9", true, 9),
    offset!("offset of the second copy", true, 2056),
    offset!("offset of the last copy", false, 522_240),
    offset!("offset past any device", true, 1 << 40),
    one("offset wrong on the primary only", 0, false, |b| {
        u64_at(b, 0x68, 9)
    }),
    one("offset wrong on the last copy only", 2, true, |b| {
        u64_at(b, 0x68, 9)
    }),
    // Identity.
    all("internal uuid zero", true, |b| b[0x28..0x38].fill(0)),
    all("user uuid zero", true, |b| b[0x38..0x48].fill(0)),
    one("user uuid zero on the primary only", 0, false, |b| {
        b[0x38..0x48].fill(0)
    }),
    one("user uuid zero on the second copy only", 1, false, |b| {
        b[0x38..0x48].fill(0)
    }),
    one("user uuid zero on the last copy only", 2, true, |b| {
        b[0x38..0x48].fill(0)
    }),
    all("device index 1", true, |b| b[0x7a] = 1),
    all("no member devices", true, |b| b[0x7b] = 0),
    // Time precision.
    precision!("time precision 0", true, 0),
    precision!("time precision 1", false, 1),
    precision!("time precision 1000", false, 1000),
    precision!("time precision 1e9", false, 1_000_000_000),
    precision!("time precision just over 1e9", true, 1_000_000_001),
    precision!("time precision 2e9", true, 2_000_000_000),
    precision!("time precision of all ones", true, u32::MAX),
    // The checksum type.
    all("checksum type crc32c without a checksum", true, |b| {
        set_field(b, 0, 2, 6, 1)
    }),
    all("checksum type 4", true, |b| set_field(b, 0, 2, 6, 4)),
    all("checksum type 16", true, |b| set_field(b, 0, 2, 6, 16)),
    all("checksum type 32", true, |b| set_field(b, 0, 2, 6, 32)),
    // The first feature word: to bit 23 is known to the reference. Clearing
    // new_extent_overwrite and setting bit 21 are each refused as well.
    feature!("no features", true, 0),
    feature!("bit 9 cleared", true, FEATURES_FORMATTED & !(1 << 9)),
    feature!("bit 0 set", false, FEATURES_FORMATTED | 1),
    feature!("bit 7 cleared", false, FEATURES_FORMATTED & !(1 << 7)),
    feature!("bit 20 set", false, FEATURES_FORMATTED | (1 << 20)),
    feature!("bit 21 set", true, FEATURES_FORMATTED | (1 << 21)),
    all("bit 21 set, freespace flag cleared", true, |b| {
        u64_at(b, FEATURES, FEATURES_FORMATTED | (1 << 21));
        let at = member(b) + 43;
        b[at] &= !0x40;
    }),
    feature!("bit 22 set", false, FEATURES_FORMATTED | (1 << 22)),
    feature!("bit 23 set", false, FEATURES_FORMATTED | (1 << 23)),
    feature!("bits 22 and 23 set", false, FEATURES_FORMATTED | (3 << 22)),
    feature!("bit 24 set", true, FEATURES_FORMATTED | (1 << 24)),
    feature!("bit 25 set", true, FEATURES_FORMATTED | (1 << 25)),
    feature!("bit 31 set", true, FEATURES_FORMATTED | (1 << 31)),
    feature!("bit 32 set", true, FEATURES_FORMATTED | (1 << 32)),
    feature!("bit 63 set", true, FEATURES_FORMATTED | (1 << 63)),
    all("second feature word bit 0", true, |b| u64_at(b, 0xd8, 1)),
    all("second feature word bit 31", true, |b| {
        u64_at(b, 0xd8, 1 << 31)
    }),
    all("second feature word bit 63", true, |b| {
        u64_at(b, 0xd8, 1 << 63)
    }),
    // The layout.
    all("layout magic wrong", true, |b| b[0xf0..0x100].fill(0x11)),
    layout_at!("layout type 1", true, 0x100, 1),
    layout_at!("layout type 9", true, 0x100, 9),
    layout_at!("layout type 255", true, 0x100, 255),
    layout_at!("layout size bits 0", true, 0x101, 0),
    layout_at!("layout size bits 1", true, 0x101, 1),
    layout_at!("layout size bits 11", false, 0x101, 11),
    layout_at!("layout size bits 12", true, 0x101, 12),
    layout_at!("layout size bits 19", true, 0x101, 19),
    layout_at!("layout size bits 20", true, 0x101, 20),
    layout_at!("layout size bits 255", true, 0x101, 255),
    layout_at!("layout of no copies", true, 0x102, 0),
    layout_at!("layout of 3 copies", false, 0x102, 3),
    layout_at!("layout of 4 copies", true, 0x102, 4),
    layout_at!("layout of 61 copies", true, 0x102, 61),
    layout_at!("layout of 62 copies", true, 0x102, 62),
    layout_at!("layout of 255 copies", true, 0x102, 255),
    // Block size, in sectors.
    all("block size 0", true, |b| u16_at(b, 0x78, 0)),
    all("block size 1", false, |b| u16_at(b, 0x78, 1)),
    all("block size 2", true, |b| u16_at(b, 0x78, 2)),
    all("block size 3", true, |b| u16_at(b, 0x78, 3)),
    all("block size 64", true, |b| u16_at(b, 0x78, 64)),
    all("block size 128", true, |b| u16_at(b, 0x78, 128)),
    all("block size over the bucket", true, |b| u16_at(b, 0x78, 512)),
    all("block size of all ones", true, |b| {
        u16_at(b, 0x78, u16::MAX)
    }),
    one("block size 0 on the primary only", 0, false, |b| {
        u16_at(b, 0x78, 0)
    }),
    one("block size 0 on the second copy only", 1, false, |b| {
        u16_at(b, 0x78, 0)
    }),
    one("block size 0 on the last copy only", 2, true, |b| {
        u16_at(b, 0x78, 0)
    }),
    // Options in the flag words.
    opt!("errors 3", false, 0, 8, 4, 3),
    opt!("errors 4", true, 0, 8, 4, 4),
    opt!("btree node size 0", true, 0, 12, 16, 0),
    opt!("btree node size 3", true, 0, 12, 16, 3),
    opt!("btree node size 256", false, 0, 12, 16, 256),
    opt!("btree node over the bucket", true, 0, 12, 16, 512),
    opt!("gc reserve percent 4", true, 0, 28, 5, 4),
    opt!("gc reserve percent 5", false, 0, 28, 5, 5),
    opt!("gc reserve percent 20", false, 0, 28, 5, 20),
    opt!("gc reserve percent 21", true, 0, 28, 5, 21),
    opt!("root reserve percent 100", false, 0, 33, 7, 100),
    opt!("root reserve percent 101", true, 0, 33, 7, 101),
    opt!("metadata checksum 3", false, 0, 40, 4, 3),
    opt!("metadata checksum 4", true, 0, 40, 4, 4),
    opt!("data checksum 3", false, 0, 44, 4, 3),
    opt!("data checksum 4", true, 0, 44, 4, 4),
    opt!("metadata replicas 0", true, 0, 48, 4, 0),
    opt!("metadata replicas 5", true, 0, 48, 4, 5),
    opt!("data replicas 0", true, 0, 52, 4, 0),
    opt!("data replicas 2", false, 0, 52, 4, 2),
    opt!("data replicas 4", false, 0, 52, 4, 4),
    opt!("data replicas 5", true, 0, 52, 4, 5),
    opt!("string hash 2", false, 1, 0, 4, 2),
    opt!("string hash 3", true, 1, 0, 4, 3),
    opt!("compression 3", false, 1, 4, 4, 3),
    opt!("compression 4", true, 1, 4, 4, 4),
    opt!("encryption word 1", true, 1, 10, 4, 1),
    opt!("encryption word 8", true, 1, 10, 4, 8),
    opt!("encoded extent limit 2", true, 1, 14, 6, 2),
    opt!("encoded extent limit 3", false, 1, 14, 6, 3),
    opt!("encoded extent limit 12", false, 1, 14, 6, 12),
    opt!("encoded extent limit 13", true, 1, 14, 6, 13),
    opt!("background compression 3", false, 2, 0, 4, 3),
    opt!("background compression 4", true, 2, 0, 4, 4),
    // A device reserve is a rule for a first start alone: it needs room to
    // start in, and a started filesystem has started.
    opt!(
        "gc reserve bytes half the device",
        false,
        2,
        4,
        55,
        reserve(500)
    ),
    opt!(
        "gc reserve bytes 97% of the device",
        false,
        2,
        4,
        55,
        reserve(970)
    ),
    opt!(
        "gc reserve bytes the whole device",
        false,
        2,
        4,
        55,
        reserve(1000)
    ),
    opt!(
        "gc reserve bytes past the device",
        false,
        2,
        4,
        55,
        reserve(1010)
    ),
    opt!("erasure code 1", false, 3, 0, 16, 1),
    opt!("erasure code 2", true, 3, 0, 16, 2),
    opt!("version upgrade 2", false, 4, 54, 2, 2),
    opt!("version upgrade 3", true, 4, 54, 2, 3),
    opt!("compression level without a type", true, 4, 56, 4, 1),
    opt!("background level without a type", true, 4, 60, 4, 1),
    opt!("write error timeout 300", false, 6, 4, 10, 300),
    opt!("write error timeout 301", true, 6, 4, 10, 301),
    opt!("checksum retries 32", false, 6, 14, 6, 32),
    opt!("checksum retries 33", true, 6, 14, 6, 33),
    opt!("recent journal scrub 2", false, 6, 48, 2, 2),
    opt!("recent journal scrub 3", true, 6, 48, 2, 3),
    opt!("stripe data blocks 15", false, 6, 50, 8, 15),
    opt!("stripe data blocks 16", true, 6, 50, 8, 16),
    opt!("write degraded 2", false, 6, 60, 2, 2),
    opt!("write degraded 3", true, 6, 60, 2, 3),
    // Options in the ext field.
    ext_opt!("stripe buffer limit 25", false, 20, 6, 25),
    ext_opt!("stripe buffer limit 26", true, 20, 6, 26),
    ext_opt!("journal rewind seconds 3600", false, 26, 12, 3600),
    ext_opt!("journal rewind seconds 3601", true, 26, 12, 3601),
    ext_opt!("rewind discard percent 10", false, 38, 4, 10),
    ext_opt!("rewind discard percent 11", true, 38, 4, 11),
    ext_opt!("missing device timeout 3600", false, 49, 12, 3600),
    ext_opt!("missing device timeout 3601", true, 49, 12, 3601),
    // The field list.
    all("no fields", true, |b| u32_at(b, U64S, 0)),
    all("more fields than it holds", true, |b| {
        u32_at(b, U64S, 1 << 16)
    }),
    all("no ext field", false, |b| drop_fields(b, &[FIELD_EXT])),
    all("no members_v2 field", true, |b| {
        drop_fields(b, &[FIELD_MEMBERS_V2])
    }),
    all("a field of no size", true, |b| {
        let at = field_at(b, FIELD_EXT);
        u32_at(b, at, 0);
    }),
    all("a field past the end", true, |b| {
        let at = field_at(b, FIELD_EXT);
        u32_at(b, at, 1 << 20);
    }),
    all("members_v1 beside members_v2", false, |b| {
        let at = member(b);
        let mut body = b[at..at + 56].to_vec();
        body.resize(64, 0);
        add_field(b, FIELD_MEMBERS_V1, &body);
    }),
    // The member, in members_v2.
    all("member size 0", true, |b| {
        let at = member(b) - 8;
        u16_at(b, at, 0);
    }),
    all("member size 16", true, |b| {
        let at = member(b) - 8;
        u16_at(b, at, 16);
    }),
    all("member size 4096", true, |b| {
        let at = member(b) - 8;
        u16_at(b, at, 4096);
    }),
    all("member uuid zero", true, |b| {
        let at = member(b);
        b[at..at + 16].fill(0);
    }),
    all("511 buckets", true, |b| {
        let at = member(b) + 16;
        u64_at(b, at, 511);
    }),
    all("one bucket past the device", true, |b| {
        let at = member(b) + 16;
        u64_at(b, at, 2049);
    }),
    all("more buckets than the limit", true, |b| {
        let at = member(b) + 16;
        u64_at(b, at, (1 << 31) - 64);
    }),
    all("first bucket 1", true, |b| {
        let at = member(b) + 24;
        u16_at(b, at, 1);
    }),
    all("bucket smaller than a btree node", true, |b| {
        let at = member(b) + 26;
        u16_at(b, at, 128);
    }),
    all("buckets past the device", true, |b| {
        let at = member(b) + 26;
        u16_at(b, at, 257);
    }),
    shift!("bitmap shift 8", false, 8),
    shift!("bitmap shift 58", true, 58),
    shift!("bitmap shift 63", true, 63),
    shift!("bitmap shift 255", true, 255),
    // A first start refuses these. A started filesystem does not.
    all("member not read-write", false, |b| {
        let at = member(b) + 40;
        b[at] |= 1;
    }),
    all("member holds no journal", false, |b| {
        let at = member(b) + 42;
        b[at] &= !2;
    }),
    all("member holds no btree", false, |b| {
        let at = member(b) + 42;
        b[at] &= !4;
    }),
    all("member durability 0", false, |b| {
        let at = member(b) + 43;
        b[at] = (b[at] & !0x30) | 0x10;
    }),
    all("freespace flag cleared", false, |b| {
        let at = member(b) + 43;
        b[at] &= !0x40;
    }),
];

/// Reported by the reference and not by this checker, whose verdict the guest
/// does not hold: the superblock itself is accepted and what reports them is a
/// pass over the filesystem finding what they imply, a bucket holding a copy
/// the layout no longer names (#553), a bitmap that names no bucket of a node
/// (#554), extents written with other options (#555).
pub const REFERENCE_SAYS_MORE: &[Case] = &[
    layout_at!("layout of 1 copy", true, 0x102, 1),
    layout_at!("layout of 2 copies", true, 0x102, 2),
    layout_at!("layout size bits 5", true, 0x101, 5),
    layout_at!("layout size bits 10", true, 0x101, 10),
    shift!("bitmap shift 0", true, 0),
    shift!("bitmap shift 57", true, 57),
    all("members_v1 in place of members_v2", true, |b| {
        let at = member(b);
        let mut body = b[at..at + 56].to_vec();
        body.resize(64, 0);
        drop_fields(b, &[FIELD_MEMBERS_V2]);
        add_field(b, FIELD_MEMBERS_V1, &body);
    }),
    opt!("metadata replicas 2", true, 0, 48, 4, 2),
    opt!("metadata replicas 3", true, 0, 48, 4, 3),
    opt!("metadata replicas 4", true, 0, 48, 4, 4),
];

/// The edited copies of a started image, as byte ranges: `read` gives `len`
/// bytes from a byte offset of the image. Each copy has its checksum type set
/// to none and its checksum zeroed, and `on` says which of them take the edit.
pub fn edited_copies(
    read: &mut dyn FnMut(u64, usize) -> Vec<u8>,
    on: On,
    edit: Edit,
) -> Vec<(u64, Vec<u8>)> {
    let mut out = Vec::new();
    for (i, sector) in COPIES.iter().enumerate() {
        let at = sector * 512;
        let head = read(at, HEADER);
        let u64s = u32::from_le_bytes(head[U64S..U64S + 4].try_into().unwrap()) as usize;
        let mut bytes = read(at, HEADER + u64s * 8);
        set_field(&mut bytes, 0, 2, 6, 0);
        bytes[..16].fill(0);
        if matches!(on, On::All) || matches!(on, On::Copy(c) if c == i) {
            edit(&mut bytes);
        }
        out.push((at, bytes));
    }
    out
}
