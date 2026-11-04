//! Each rule of `bucket_sectors.rs`, with the numbers the reference checker
//! printed or the fixtures hold (docs/clean-room.md, "A bucket's sector
//! counters"). Judged against the reference in `tests/check_oracle.rs`.

use super::*;
use crate::bkey::{key_type, Bpos};

const BUCKET: u64 = 64;
/// The layout of the `default` fixture.
fn layout() -> Layout {
    Layout {
        layout_type: 0,
        sb_max_size_bits: 11,
        sb_offsets: vec![8, 2056, 129024],
    }
}

/// An alloc_v4 key: gen byte 12, data type byte 14, dirty sectors at 16,
/// cached sectors at 20.
fn alloc(bucket: u64, data_type: u8, dirty: u32, cached: u32) -> Bkey {
    let mut value = vec![0; 64];
    value[14] = data_type;
    value[16..20].copy_from_slice(&dirty.to_le_bytes());
    value[20..24].copy_from_slice(&cached.to_le_bytes());
    Bkey {
        key_type: key_type::ALLOC_V4,
        size: 0,
        version_hi: 0,
        version_lo: 0,
        pos: Bpos {
            inode: 0,
            offset: bucket,
            snapshot: 0,
        },
        value,
    }
}

fn ptr_word(sector: u64, flags: u64) -> u64 {
    1 | (flags << 1) | (sector << 4)
}

/// A crc32 entry: bits 2..8 compressed size less one, 9..15 uncompressed
/// size less one, 28..31 the compression type.
fn crc_word(compressed: u64, uncompressed: u64, compression: u64) -> u64 {
    0b10 | ((compressed - 1) << 2) | ((uncompressed - 1) << 9) | (compression << 28)
}

fn value(words: &[u64]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn node_ptr(sector: u64) -> Ptr {
    Ptr {
        dev: 0,
        offset: sector,
        gen: 0,
        flags: 0,
    }
}

fn findings(held: &BucketSectors, keys: &[Bkey]) -> Vec<(&'static str, String)> {
    let mut r = Report::default();
    held.judge(keys, &mut r);
    r.problems.into_iter().map(|p| (p.kind, p.detail)).collect()
}

fn held() -> BucketSectors {
    BucketSectors::new(BUCKET, 0, 0)
}

/// `dirty` is what `held` must say the bucket holds: right is clean, and
/// one more or one fewer is named with both numbers.
fn assert_holds(held: &BucketSectors, bucket: u64, data_type: u8, dirty: u32, name: &str) {
    assert_eq!(
        findings(held, &[alloc(bucket, data_type, dirty, 0)]),
        vec![],
        "bucket {bucket} holding {dirty} sectors was reported"
    );
    for got in [dirty + 1, dirty.wrapping_sub(1)] {
        assert_eq!(
            findings(held, &[alloc(bucket, data_type, got, 0)]),
            vec![(
                "alloc_key_dirty_sectors_wrong",
                format!(
                    "bucket 0:{bucket} gen 0 data type {name} has wrong dirty_sectors: got {got}, \
                     should be {dirty}"
                ),
            )],
        );
    }
}

/// kernel-written.img, seed 1659864: the reference's "bucket 0:87 gen 1 data
/// type user has wrong dirty_sectors: got 23, should be 20", the extent of
/// three sectors having become another type of key.
#[test]
fn a_user_bucket_counts_what_its_extents_hold() {
    let mut h = held();
    let sector = 87 * BUCKET;
    h.extent(12, &value(&[ptr_word(sector, 0)]));
    h.extent(8, &value(&[ptr_word(sector + 12, 0)]));
    assert_holds(&h, 87, 4, 20, "user");
}

/// `aged` bucket 105: pieces of 111 and 120 sectors of one 256-sector region
/// count their live sectors, not the region's.
#[test]
fn a_piece_of_an_uncompressed_region_counts_its_live_sectors() {
    let mut h = BucketSectors::new(256, 0, 0);
    let crc = crc_word(128, 128, 0);
    h.extent(40, &value(&[crc, ptr_word(105 * 256, 0)]));
    h.extent(50, &value(&[crc, ptr_word(105 * 256, 0)]));
    assert_holds(&h, 105, 4, 90, "user");
}

/// The fixtures' whole compressed extents count their stored size, and an
/// incompressible one, stored raw, its size.
#[test]
fn a_whole_compressed_extent_counts_its_stored_sectors() {
    let mut h = held();
    let sector = 9 * BUCKET;
    h.extent(64, &value(&[crc_word(5, 64, 3), ptr_word(sector, 0)]));
    h.extent(9, &value(&[crc_word(9, 9, 5), ptr_word(sector + 5, 0)]));
    assert_holds(&h, 9, 4, 14, "user");
}

/// `kernel-prealloc`: an unwritten extent's bucket counts it.
#[test]
fn an_unwritten_extent_counts_its_sectors() {
    let mut h = held();
    h.extent(32, &value(&[ptr_word(3 * BUCKET, 0b100)]));
    assert_holds(&h, 3, 4, 32, "user");
}

/// `crc128-overwrite`: 512-sector nodes in 1024-sector buckets.
#[test]
fn a_btree_bucket_counts_its_nodes() {
    let mut h = BucketSectors::new(1024, 0, 0);
    h.node(&[node_ptr(5 * 1024)], 512);
    assert_holds(&h, 5, 3, 512, "btree");
    h.node(&[node_ptr(5 * 1024 + 512)], 512);
    assert_holds(&h, 5, 3, 1024, "btree");
}

#[test]
fn a_journal_bucket_counts_all_of_itself() {
    let mut h = held();
    h.journal(&[(113, 16)]);
    assert_holds(&h, 113, 2, 64, "journal");
    assert_holds(&h, 128, 2, 64, "journal");
    // One past the range holds nothing.
    assert_holds(&h, 129, 3, 0, "btree");
}

/// `default`: the copies at sectors 8 and 2056 run on to sector 4104, into
/// bucket 64, and the one at 129024 fills the last 32 buckets.
#[test]
fn a_superblock_bucket_counts_the_copies_in_it() {
    let mut h = held();
    h.superblock(&layout());
    assert_holds(&h, 0, 1, 64, "sb");
    assert_holds(&h, 63, 1, 64, "sb");
    assert_holds(&h, 64, 1, 8, "sb");
    assert_holds(&h, 65, 4, 0, "user");
    assert_holds(&h, 2015, 4, 0, "user");
    assert_holds(&h, 2016, 1, 64, "sb");
    assert_holds(&h, 2047, 1, 64, "sb");
}

/// `crc128`: 1024-sector buckets, and the second copy at sector 2112. The 56
/// sectors after the first copy hold nothing, and bucket 2 does not count them.
#[test]
fn a_superblock_bucket_does_not_count_a_gap_between_copies() {
    let mut h = BucketSectors::new(1024, 0, 0);
    h.superblock(&Layout {
        layout_type: 0,
        sb_max_size_bits: 11,
        sb_offsets: vec![8, 2112, 2095104],
    });
    assert_holds(&h, 1, 1, 1024, "sb");
    assert_holds(&h, 2, 1, 968, "sb");
    assert_holds(&h, 3, 1, 1024, "sb");
    assert_holds(&h, 4, 1, 64, "sb");
}

/// `default.img` seed 1660430's "bucket 0:59 gen 0 data type sb has wrong
/// cached_sectors: got 131072, should be 0", and `block4k.img` seed 1659895's
/// "bucket 0:228 gen 0 data type user has wrong cached_sectors: got 8192,
/// should be 0".
#[test]
fn cached_sectors_with_nothing_cached_are_named() {
    let mut h = held();
    h.superblock(&layout());
    h.extent(8, &value(&[ptr_word(228 * BUCKET, 0)]));
    assert_eq!(
        findings(&h, &[alloc(59, 1, 64, 131072)]),
        vec![(
            "alloc_key_cached_sectors_wrong",
            "bucket 0:59 gen 0 data type sb has wrong cached_sectors: got 131072, should be 0"
                .to_string(),
        )],
    );
    assert_eq!(
        findings(&h, &[alloc(228, 4, 8, 8192)]),
        vec![(
            "alloc_key_cached_sectors_wrong",
            "bucket 0:228 gen 0 data type user has wrong cached_sectors: got 8192, should be 0"
                .to_string(),
        )],
    );
}

/// Both wrong: each counter is named once.
#[test]
fn each_wrong_counter_of_a_bucket_is_named() {
    let kinds: Vec<_> = findings(&held(), &[alloc(300, 4, 8, 8)])
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    assert_eq!(
        kinds,
        [
            "alloc_key_dirty_sectors_wrong",
            "alloc_key_cached_sectors_wrong"
        ]
    );
}

/// Where this check has nothing to compare, it reports nothing: another
/// device's key, a key type that holds no counters, a value too short to
/// hold them, and a bucket below the device's first.
#[test]
fn a_key_with_no_counters_to_compare_is_not_judged() {
    let h = BucketSectors::new(BUCKET, 0, 100);
    let mut other_device = alloc(200, 4, 8, 8);
    other_device.pos.inode = 1;
    let mut other_type = alloc(200, 4, 8, 8);
    other_type.key_type = key_type::EXTENT;
    let mut short = alloc(200, 4, 8, 8);
    short.value.truncate(23);
    let below_first = alloc(99, 4, 8, 8);
    assert_eq!(
        findings(&h, &[other_device, other_type, short, below_first]),
        vec![]
    );
}

/// A sector count of a device's other pointers is another device's, and a
/// value that does not parse leaves the buckets unknown: `extent_data` names
/// it, and nothing here guesses.
#[test]
fn only_what_can_be_read_is_counted() {
    let mut h = held();
    h.extent(8, &value(&[ptr_word(5 * BUCKET, 0) | (1 << 48)]));
    assert_eq!(findings(&h, &[alloc(5, 4, 0, 0)]), vec![]);
    // A crc64 entry cut short.
    h.extent(8, &[0b100, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(findings(&h, &[alloc(5, 4, 99, 99)]), vec![]);
}

/// A damaged layout or journal field can name any number: no sum overflows,
/// and no loop runs over the buckets they name.
#[test]
fn numbers_at_the_edges_of_their_ranges_are_harmless() {
    let mut h = held();
    h.superblock(&Layout {
        layout_type: 0,
        sb_max_size_bits: u8::MAX,
        sb_offsets: vec![u64::MAX, 8, u64::MAX - 1],
    });
    h.journal(&[(u64::MAX, u64::MAX), (0, u64::MAX)]);
    h.extent(u32::MAX, &value(&[ptr_word((1 << 44) - 1, 0)]));
    let _ = findings(&h, &[alloc(u64::MAX, 2, 0, 0), alloc(0, 2, 0, 0)]);
}
