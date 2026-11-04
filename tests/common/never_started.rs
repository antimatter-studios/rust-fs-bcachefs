//! What the reference checker said of a superblock the reference formatter
//! wrote and nothing has started, one edit at a time (#505,
//! docs/clean-room.md "Images formatted and never started").
//!
//! Such an image is three superblock copies and the layout at sector 7, and
//! its superblock carries no checksum (type 0), so an edit needs no restamping
//! and a whole image is a handful of byte ranges. The table holds the edit and
//! the verdict measured. The unit tier holds this checker to the verdict; the
//! guest (`tests/check_oracle_never_started.rs`) holds the reference checker
//! to it as well, so a table that drifts from the reference fails there.

#![allow(dead_code)]

/// Sectors of the copies the formatter writes on an image of [`DEVICE_BYTES`].
pub const COPIES: [u64; 3] = [8, 2056, 522240];
pub const DEVICE_BYTES: u64 = 256 << 20;
const LAYOUT_AT: u64 = 7 * 512;
const LAYOUT_IN_SB: usize = 0xf0;
const LAYOUT_BYTES: usize = 512;
const HEADER: usize = 0x2f0;
const OFFSET: usize = 0x68;
const FLAGS: usize = 0x90;
const U64S: usize = 0x7c;

const FIELD_MEMBERS_V1: u32 = 1;
const FIELD_CLEAN: u32 = 6;
const FIELD_JOURNAL_V2: u32 = 9;
const FIELD_MEMBERS_V2: u32 = 11;
const FIELD_EXT: u32 = 13;

/// One edit to a superblock's bytes.
pub type Edit = fn(&mut Vec<u8>);

/// Which copies an edit goes to.
#[derive(Clone, Copy, Debug)]
pub enum On {
    All,
    Copy(usize),
}

pub struct Case {
    pub name: &'static str,
    pub on: On,
    pub edit: Edit,
    /// Whether the reference checker reports a problem.
    pub reported: bool,
}

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

fn u16_at(b: &mut [u8], at: usize, value: u16) {
    b[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn u32_at(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn u64_at(b: &mut [u8], at: usize, value: u64) {
    b[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// The bit field `flags[word] >> shift`, `bits` wide.
fn set_field(b: &mut [u8], word: usize, shift: u32, bits: u32, value: u64) {
    let at = FLAGS + word * 8;
    let mask = if bits == 64 {
        u64::MAX
    } else {
        ((1u64 << bits) - 1) << shift
    };
    let old = u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
    u64_at(b, at, (old & !mask) | ((value << shift) & mask));
}

fn fields(b: &[u8]) -> Vec<(u32, usize, usize)> {
    let end = HEADER + u32::from_le_bytes(b[U64S..U64S + 4].try_into().unwrap()) as usize * 8;
    let (mut at, mut out) = (HEADER, Vec::new());
    while at + 8 <= end {
        let size = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize * 8;
        if size == 0 {
            break;
        }
        out.push((
            u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()),
            at,
            size,
        ));
        at += size;
    }
    out
}

fn field_at(b: &[u8], field_type: u32) -> usize {
    fields(b)
        .iter()
        .find(|(t, _, _)| *t == field_type)
        .map(|(_, at, _)| *at)
        .expect("the formatter writes this field")
}

/// Where member 0 starts in `members_v2`: after the field header and the word
/// holding the member size.
fn member(b: &[u8]) -> usize {
    field_at(b, FIELD_MEMBERS_V2) + 16
}

fn drop_fields(b: &mut Vec<u8>, gone: &[u32]) {
    let mut out = b[..HEADER].to_vec();
    for (t, at, size) in fields(b) {
        if !gone.contains(&t) {
            out.extend_from_slice(&b[at..at + size]);
        }
    }
    let u64s = ((out.len() - HEADER) / 8) as u32;
    u32_at(&mut out, U64S, u64s);
    *b = out;
}

fn add_field(b: &mut Vec<u8>, field_type: u32, body: &[u8]) {
    assert_eq!(body.len() % 8, 0);
    let u64s = 1 + (body.len() / 8) as u32;
    b.extend_from_slice(&u64s.to_le_bytes());
    b.extend_from_slice(&field_type.to_le_bytes());
    b.extend_from_slice(body);
    let total = ((b.len() - HEADER) / 8) as u32;
    u32_at(b, U64S, total);
}

fn ext(b: &mut [u8], at_bit: u32, bits: u32, value: u64) {
    let word = field_at(b, FIELD_EXT) + 8 + 88;
    let old = u64::from_le_bytes(b[word..word + 8].try_into().unwrap());
    let mask = ((1u64 << bits) - 1) << at_bit;
    u64_at(b, word, (old & !mask) | ((value << at_bit) & mask));
}

macro_rules! opt {
    ($name:literal, $reported:literal, $word:literal, $shift:literal, $bits:literal, $value:expr) => {
        all($name, $reported, |b| {
            set_field(b, $word, $shift, $bits, $value)
        })
    };
}

macro_rules! ext {
    ($name:literal, $reported:literal, $bit:literal, $bits:literal, $value:expr) => {
        all($name, $reported, |b| ext(b, $bit, $bits, $value))
    };
}

/// Every case, measured with reference checker 1.39.7 on 2026-11-03.
pub const CASES: &[Case] = &[
    all("unchanged", false, |_| {}),
    // What marks it as started, and what does not.
    all("marked started", true, |b| set_field(b, 0, 0, 1, 1)),
    all("marked started and clean", true, |b| {
        set_field(b, 0, 0, 2, 3)
    }),
    all("marked clean only", false, |b| set_field(b, 0, 1, 1, 1)),
    all("sequence 1", false, |b| u64_at(b, 0x70, 1)),
    all("sequence 26", false, |b| u64_at(b, 0x70, 26)),
    all("compat features set", false, |b| u64_at(b, 0xe0, 0x7f)),
    all("no features", false, |b| u64_at(b, 0xd0, 0)),
    all("freespace marked initialized", false, |b| {
        let at = member(b) + 43;
        b[at] |= 0x40;
    }),
    // The copy the reference believes decides: the highest sequence, the
    // later copy on a tie.
    one("primary marked started", 0, false, |b| {
        set_field(b, 0, 0, 1, 1)
    }),
    one("second copy marked started", 1, false, |b| {
        set_field(b, 0, 0, 1, 1)
    }),
    one("last copy marked started", 2, true, |b| {
        set_field(b, 0, 0, 1, 1)
    }),
    one("primary newest and marked started", 0, true, |b| {
        u64_at(b, 0x70, 1);
        set_field(b, 0, 0, 1, 1);
    }),
    one("second copy newest and marked started", 1, true, |b| {
        u64_at(b, 0x70, 1);
        set_field(b, 0, 0, 1, 1);
    }),
    // Versions.
    all("version 1.39", false, |b| u16_at(b, 0x10, 0x0427)),
    all("version 1.1023", false, |b| u16_at(b, 0x10, 0x07ff)),
    all("version 2.0", true, |b| u16_at(b, 0x10, 0x0800)),
    all("version 0.0", true, |b| u16_at(b, 0x10, 0)),
    all("version below its minimum", true, |b| {
        u16_at(b, 0x10, 0x0425)
    }),
    all("minimum 0.9", false, |b| u16_at(b, 0x12, 9)),
    all("minimum 0.8", true, |b| u16_at(b, 0x12, 8)),
    all("minimum above the version", true, |b| {
        u16_at(b, 0x12, 0x0427)
    }),
    all("incompat in use 1.39", false, |b| u16_at(b, 0xbc, 0x0427)),
    all("incompat in use 1.40", true, |b| u16_at(b, 0xbc, 0x0428)),
    all("incompat in use 2.0", true, |b| u16_at(b, 0xbc, 0x0800)),
    all("oldest on disk 2.0", false, |b| u16_at(b, 0xbe, 0x0800)),
    // The header.
    all("offset wrong", true, |b| u64_at(b, OFFSET, 9)),
    all("internal uuid zero", true, |b| b[0x28..0x38].fill(0)),
    all("user uuid zero", true, |b| b[0x38..0x48].fill(0)),
    all("device index 1", true, |b| b[0x7a] = 1),
    all("no member devices", true, |b| b[0x7b] = 0),
    all("time precision 0", true, |b| u32_at(b, 0x8c, 0)),
    all("time precision 1e9", false, |b| {
        u32_at(b, 0x8c, 1_000_000_000)
    }),
    all("time precision over 1e9", true, |b| {
        u32_at(b, 0x8c, 1_000_000_001)
    }),
    all("no fields", true, |b| u32_at(b, U64S, 0)),
    all("more fields than it holds", true, |b| {
        u32_at(b, U64S, 1 << 16)
    }),
    all("larger than its layout allows", true, |b| {
        u32_at(b, U64S, 1 << 20)
    }),
    all("magic wrong", true, |b| b[0x18..0x28].fill(0x11)),
    all("checksum type crc32c without a checksum", true, |b| {
        set_field(b, 0, 2, 6, 1)
    }),
    all("checksum type 4", true, |b| set_field(b, 0, 2, 6, 4)),
    all("checksum type 16", true, |b| set_field(b, 0, 2, 6, 16)),
    all("checksum type 32", true, |b| set_field(b, 0, 2, 6, 32)),
    // The layout.
    all("layout magic wrong", true, |b| {
        b[LAYOUT_IN_SB..LAYOUT_IN_SB + 16].fill(0x11)
    }),
    all("layout type 9", true, |b| b[0x100] = 9),
    all("layout size bits 0", true, |b| b[0x101] = 0),
    all("layout size bits 20", true, |b| b[0x101] = 20),
    all("layout of no copies", true, |b| b[0x102] = 0),
    all("layout of one copy", false, |b| b[0x102] = 1),
    all("layout of copies that overlap", true, |b| b[0x102] = 4),
    all("layout of 62 copies", true, |b| b[0x102] = 62),
    // Feature bits.
    all("feature bit 20 set", false, |b| {
        u64_at(b, 0xd0, 0xfba80 | 1 << 20)
    }),
    all("feature bit 21 set", true, |b| {
        u64_at(b, 0xd0, 0xfba80 ^ 1 << 21)
    }),
    all("feature bit 22 set", true, |b| {
        u64_at(b, 0xd0, 0xfba80 ^ 1 << 22)
    }),
    all("feature bit 23 set", true, |b| {
        u64_at(b, 0xd0, 0xfba80 ^ 1 << 23)
    }),
    all("feature bit 24 set", true, |b| {
        u64_at(b, 0xd0, 0xfba80 ^ 1 << 24)
    }),
    all("feature bit 63 set", true, |b| {
        u64_at(b, 0xd0, 0xfba80 ^ 1 << 63)
    }),
    all("second feature word set", true, |b| u64_at(b, 0xd8, 1)),
    // Block size, in sectors.
    all("block size 0", true, |b| u16_at(b, 0x78, 0)),
    all("block size 2", false, |b| u16_at(b, 0x78, 2)),
    all("block size 3", true, |b| u16_at(b, 0x78, 3)),
    all("block size 64", false, |b| u16_at(b, 0x78, 64)),
    all("block size 128", true, |b| u16_at(b, 0x78, 128)),
    all("block size over the bucket", true, |b| u16_at(b, 0x78, 512)),
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
    opt!("metadata replicas 2", true, 0, 48, 4, 2),
    opt!("metadata replicas 4", true, 0, 48, 4, 4),
    opt!("metadata replicas 5", true, 0, 48, 4, 5),
    opt!("data replicas 0", true, 0, 52, 4, 0),
    opt!("data replicas 4", false, 0, 52, 4, 4),
    opt!("data replicas 5", true, 0, 52, 4, 5),
    opt!("string hash 2", false, 1, 0, 4, 2),
    opt!("string hash 3", true, 1, 0, 4, 3),
    opt!("compression 3", false, 1, 4, 4, 3),
    opt!("compression 4", true, 1, 4, 4, 4),
    opt!("encryption word 1", true, 1, 10, 4, 1),
    opt!("encryption word 2", true, 1, 10, 4, 2),
    opt!("encryption word 8", true, 1, 10, 4, 8),
    opt!("encoded extent limit 2", true, 1, 14, 6, 2),
    opt!("encoded extent limit 3", false, 1, 14, 6, 3),
    opt!("encoded extent limit 12", false, 1, 14, 6, 12),
    opt!("encoded extent limit 13", true, 1, 14, 6, 13),
    opt!("background compression 3", false, 2, 0, 4, 3),
    opt!("background compression 4", true, 2, 0, 4, 4),
    opt!("gc reserve bytes half the device", false, 2, 4, 55, 1 << 18),
    opt!(
        "gc reserve bytes 96.5% of the device",
        false,
        2,
        4,
        55,
        505_937
    ),
    opt!(
        "gc reserve bytes 97.3% of the device",
        true,
        2,
        4,
        55,
        510_000
    ),
    opt!("gc reserve bytes the whole device", true, 2, 4, 55, 1 << 19),
    opt!("gc reserve bytes past the device", true, 2, 4, 55, 1 << 40),
    opt!("erasure code 1", false, 3, 0, 16, 1),
    opt!("erasure code 2", true, 3, 0, 16, 2),
    opt!("version upgrade 2", false, 4, 54, 2, 2),
    opt!("version upgrade 3", true, 4, 54, 2, 3),
    opt!("compression level without a type", true, 4, 56, 4, 1),
    opt!("background level without a type", true, 4, 60, 4, 1),
    opt!("inode shard bits 8", false, 6, 0, 4, 8),
    opt!("write error timeout 300", false, 6, 4, 10, 300),
    opt!("write error timeout 301", true, 6, 4, 10, 301),
    opt!("checksum retries 32", false, 6, 14, 6, 32),
    opt!("checksum retries 33", true, 6, 14, 6, 33),
    opt!("degraded 3", false, 6, 20, 2, 3),
    opt!("recent journal scrub 2", false, 6, 48, 2, 2),
    opt!("recent journal scrub 3", true, 6, 48, 2, 3),
    opt!("stripe data blocks 15", false, 6, 50, 8, 15),
    opt!("stripe data blocks 16", true, 6, 50, 8, 16),
    opt!("write degraded 2", false, 6, 60, 2, 2),
    opt!("write degraded 3", true, 6, 60, 2, 3),
    // Options in the ext field.
    ext!("stripe buffer limit 25", false, 20, 6, 25),
    ext!("stripe buffer limit 26", true, 20, 6, 26),
    ext!("journal rewind seconds 3600", false, 26, 12, 3600),
    ext!("journal rewind seconds 3601", true, 26, 12, 3601),
    ext!("rewind discard percent 10", false, 38, 4, 10),
    ext!("rewind discard percent 11", true, 38, 4, 11),
    ext!("missing device timeout 3600", false, 49, 12, 3600),
    ext!("missing device timeout 3601", true, 49, 12, 3601),
    all("recovery passes required", false, |b| {
        let at = field_at(b, FIELD_EXT) + 8;
        b[at] = 1;
    }),
    // The field list.
    all("no ext field", false, |b| drop_fields(b, &[FIELD_EXT])),
    all("no members_v1 field", false, |b| {
        drop_fields(b, &[FIELD_MEMBERS_V1])
    }),
    all("no members_v2 field", false, |b| {
        drop_fields(b, &[FIELD_MEMBERS_V2])
    }),
    all("neither member field", true, |b| {
        drop_fields(b, &[FIELD_MEMBERS_V1, FIELD_MEMBERS_V2])
    }),
    all("members_v1 damaged beside members_v2", false, |b| {
        let at = field_at(b, FIELD_MEMBERS_V1) + 8;
        u64_at(b, at + 16, 4);
    }),
    all("members_v1 alone and too few buckets", true, |b| {
        let at = field_at(b, FIELD_MEMBERS_V1) + 8;
        u64_at(b, at + 16, 4);
        drop_fields(b, &[FIELD_MEMBERS_V2]);
    }),
    all("a field of no size", true, |b| {
        let at = field_at(b, FIELD_EXT);
        u32_at(b, at, 0);
    }),
    all("a field past the end", true, |b| {
        let at = field_at(b, FIELD_EXT);
        u32_at(b, at, 1 << 20);
    }),
    all("an unknown field", false, |b| add_field(b, 99, &[0; 8])),
    // The member, in members_v2.
    all("member size 0", true, |b| {
        let at = member(b) - 8;
        u16_at(b, at, 0);
    }),
    all("member size 16", true, |b| {
        let at = member(b) - 8;
        u16_at(b, at, 16);
    }),
    all("member larger than its field", true, |b| {
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
    all("512 buckets", false, |b| {
        let at = member(b) + 16;
        u64_at(b, at, 512);
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
    all("first bucket 5", true, |b| {
        let at = member(b) + 24;
        u16_at(b, at, 5);
    }),
    all("bucket smaller than a btree node", true, |b| {
        let at = member(b) + 26;
        u16_at(b, at, 128);
    }),
    all("buckets past the device", true, |b| {
        let at = member(b) + 26;
        u16_at(b, at, 257);
    }),
    all("bucket size not a power of two", false, |b| {
        let at = member(b) + 16;
        u64_at(b, at, 1000);
        u16_at(b, at + 10, 257);
    }),
    all("bitmap shift 57", false, |b| {
        let at = member(b) + 28;
        b[at] = 57;
    }),
    all("bitmap shift 58", true, |b| {
        let at = member(b) + 28;
        b[at] = 58;
    }),
    all("member not read-write", true, |b| {
        let at = member(b) + 40;
        b[at] |= 1;
    }),
    all("member holds no journal", true, |b| {
        let at = member(b) + 42;
        b[at] &= !2;
    }),
    all("member holds no btree", true, |b| {
        let at = member(b) + 42;
        b[at] &= !4;
    }),
    all("member holds only journal and btree", false, |b| {
        let at = member(b) + 42;
        b[at] = (b[at] & 0xf0) | 6;
    }),
    all("member durability 0", true, |b| {
        let at = member(b) + 43;
        b[at] = (b[at] & !0x30) | 0x10;
    }),
    all("member durability 2", false, |b| {
        let at = member(b) + 43;
        b[at] |= 0x30;
    }),
];

/// Images on which the reference's own verdict varies from run to run, so the
/// guest holds it to none: only this checker's verdict, in the unit tier, is held
/// here (#548). Measured on one image each, and in docs/clean-room.md.
///
/// - A device reserve from 96.7% to 97.2% of 256 MiB: accepted 51 of 60 runs at
///   96.9%, and reported 48 of 60 at 97.2%.
/// - Inode shard bits above 8: it shards new inode numbers by the CPU it happens to
///   run on, and some of them have no room. At 9 bits, reported twice and accepted
///   once.
pub const REFERENCE_VARIES: &[Case] = &[
    opt!(
        "gc reserve bytes 96.9% of the device",
        false,
        2,
        4,
        55,
        508_000
    ),
    opt!(
        "gc reserve bytes 97.2% of the device",
        true,
        2,
        4,
        55,
        509_607
    ),
    opt!("inode shard bits 9", true, 6, 0, 4, 9),
];

/// A started image's journal field, naming buckets that hold nothing here.
pub const JOURNAL_FIELD_BODY: [u8; 16] = [0x11, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0];

/// A superblock that claims to have been started although it is otherwise a
/// formatted one: the reference still takes it for new, because bit 0 of the
/// first flag word is what it goes by, and this checker reports it, because a
/// `journal_v2` or `clean` field is what a start leaves.
pub const CLAIMS_A_START: &[(&str, Edit)] = &[
    ("a journal field", |b| {
        add_field(b, FIELD_JOURNAL_V2, &JOURNAL_FIELD_BODY)
    }),
    ("a clean field", |b| add_field(b, FIELD_CLEAN, &[0; 16])),
    ("both fields", |b| {
        add_field(b, FIELD_JOURNAL_V2, &JOURNAL_FIELD_BODY);
        add_field(b, FIELD_CLEAN, &[0; 16]);
    }),
];

/// The byte ranges of a whole image: the layout, and `sb` as each copy with
/// the offset it is read from, its edit applied where `on` says.
pub fn image(sb: &[u8], on: On, edit: Edit) -> Vec<(u64, Vec<u8>)> {
    let mut out = vec![(
        LAYOUT_AT,
        sb[LAYOUT_IN_SB..LAYOUT_IN_SB + LAYOUT_BYTES].to_vec(),
    )];
    for (i, sector) in COPIES.iter().enumerate() {
        let mut copy = sb.to_vec();
        u64_at(&mut copy, OFFSET, *sector);
        if matches!(on, On::All) || matches!(on, On::Copy(c) if c == i) {
            edit(&mut copy);
        }
        out.push((sector * 512, copy));
    }
    out
}
