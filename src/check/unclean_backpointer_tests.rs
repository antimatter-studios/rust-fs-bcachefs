//! A stale backpointer on a filesystem the reference wrote and did not shut
//! down cleanly, against what the reference checker said of it, in the unit
//! tier (#563). Both images in `tests/data` start as `casefold.img` of the
//! fixtures of CI run 41805364959 and have the first sector of
//! `Many/File-01.Txt` overwritten through the reference's read-write mount, and
//! the moved one first has that extent's backpointer moved within its bucket. The
//! reference checker exits 9 on it, `backpointer_to_missing_ptr`, and 0 on the
//! other. How they were made and packed is in `docs/clean-room.md`, "A stale
//! backpointer"; the guest holds the reference itself to the same verdicts
//! (`tests/check_oracle.rs`).

use super::never_started_tests::Ranges;
use super::*;

/// Each a raw deflate stream of the image's nonzero 4 KiB runs: an offset and
/// a length as two little-endian u64s, then its bytes.
const MOVED: &[u8] = include_bytes!("../../tests/data/unclean-moved-backpointer-41805364959.bin");
const UNCHANGED: &[u8] = include_bytes!("../../tests/data/unclean-unchanged-41805364959.bin");
const UNPACKED_LIMIT: usize = 1 << 20;
/// Where the moved backpointer is, and the extent it still names.
const MOVED_POS: &str = "0:390135553:0";
const EXTENT: &str = "2147483651:1:U32_MAX";

fn unpack(packed: &[u8]) -> Ranges {
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(packed, UNPACKED_LIMIT)
        .expect("the image is a deflate stream");
    let mut runs = Vec::new();
    let mut rest = raw.as_slice();
    while !rest.is_empty() {
        let at = u64::from_le_bytes(rest[..8].try_into().unwrap());
        let len = u64::from_le_bytes(rest[8..16].try_into().unwrap()) as usize;
        runs.push((at, rest[16..16 + len].to_vec()));
        rest = &rest[16 + len..];
    }
    Ranges(runs)
}

#[test]
fn a_backpointer_left_behind_on_an_uncleanly_shut_down_image_is_reported() {
    let dev = unpack(MOVED);
    assert!(!Superblock::read(&dev).unwrap().is_clean());
    let report = check(&dev).unwrap();
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert_eq!(report.problems[0].kind, "backpointer_to_missing_ptr");
    assert!(
        report.problems[0].detail.contains(MOVED_POS) && report.problems[0].detail.contains(EXTENT),
        "{:?}",
        report.problems
    );
}

#[test]
fn the_same_write_with_no_backpointer_moved_is_clean_as_the_reference_says() {
    let dev = unpack(UNCHANGED);
    assert!(!Superblock::read(&dev).unwrap().is_clean());
    let report = check(&dev).unwrap();
    assert!(report.clean(), "{:?}", report.problems);
}
