//! An extent's checksum type against the `data_checksum` option in force
//! (#614). The guest holds the reference to the same:
//! `tests/check_oracle_extent_checksum.rs`.

use super::extent_checksum::{judge, option_in_force};
use super::*;

const KEY_EXTENT: u8 = 6;
/// Differential run 42475842401's extent on `default.img`, as the fixture
/// holds it: `crc32: c_size 64 size 64 offset 0 nonce 0 csum crc32c
/// 0:b0e313ab compress none`, then `ptr: 0:237:0 gen 0`.
const CRC32C: u64 = 0xb0e3_13ab_0500_7efe;
const POINTER: u64 = 0x0000_0000_0003_b401;
/// The same crc32 entry as that run's case left it: `csum none 0:b0e30000`.
const NONE: u64 = 0xb0e3_0000_0000_7efe;
/// The `bgcompress` fixture's reconcile entry.
const RECONCILE: u64 = 0x0000_0010_9010_0080;
/// A flags entry marking the extent poisoned.
const POISONED: u64 = 0xc0;

fn extent(words: &[u64]) -> Bkey {
    Bkey {
        key_type: KEY_EXTENT,
        pos: Bpos {
            inode: 2147483653,
            offset: 800,
            snapshot: u32::MAX,
        },
        size: 64,
        version_hi: 0,
        version_lo: 0,
        value: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

fn found(words: &[u64], want: u64) -> Vec<String> {
    let mut report = Report::default();
    judge(&extent(words), want, &mut report);
    report
        .problems
        .iter()
        .map(|problem| {
            assert_eq!(problem.kind, "extent_io_opts_not_set");
            problem.detail.clone()
        })
        .collect()
}

#[test]
fn a_checksum_cleared_to_none_under_crc32c_is_reported() {
    let found = found(&[NONE, POINTER], 1);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("2147483653:800:U32_MAX"), "{found:?}");
    assert!(
        found[0].contains("is none where its options ask for crc32c"),
        "{found:?}"
    );
}

#[test]
fn a_checksum_of_the_type_its_options_ask_for_is_accepted() {
    assert_eq!(found(&[CRC32C, POINTER], 1), Vec::<String>::new());
    assert_eq!(found(&[NONE, POINTER], 0), Vec::<String>::new());
}

#[test]
fn a_checksum_of_another_type_is_reported_by_both_names() {
    // crc32c under crc64, and crc64 (6) under crc32c.
    let crc64 = (CRC32C & !(0xf << 24)) | (6 << 24);
    let under_crc64 = found(&[CRC32C, POINTER], 2);
    assert!(
        under_crc64[0].contains("is crc32c where its options ask for crc64"),
        "{under_crc64:?}"
    );
    let under_crc32c = found(&[crc64, POINTER], 1);
    assert!(
        under_crc32c[0].contains("is crc64 where its options ask for crc32c"),
        "{under_crc32c:?}"
    );
}

#[test]
fn an_extent_recording_reconcile_work_is_not_judged() {
    assert_eq!(found(&[NONE, POINTER, RECONCILE], 1), Vec::<String>::new());
}

#[test]
fn an_extent_no_rule_was_observed_for_is_not_judged() {
    // Poisoned, unwritten, and the nonzero crc32c type (1), which no image
    // has shown.
    assert_eq!(found(&[POISONED, NONE, POINTER], 1), Vec::<String>::new());
    let unwritten = POINTER | (u64::from(crate::extent::PTR_UNWRITTEN) << 1);
    assert_eq!(found(&[NONE, unwritten], 1), Vec::<String>::new());
    let nonzero = (CRC32C & !(0xf << 24)) | (1 << 24);
    assert_eq!(found(&[nonzero, POINTER], 0), Vec::<String>::new());
}

#[test]
fn the_inode_option_is_in_force_before_the_filesystem_one() {
    // Stored as the value plus one; 0 is unset.
    assert_eq!(option_in_force(0, 1), 1);
    assert_eq!(option_in_force(1, 1), 0);
    assert_eq!(option_in_force(3, 1), 2);
    assert_eq!(option_in_force(0, 3), 3);
}
