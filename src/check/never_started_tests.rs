//! This checker against what the reference checker said of a superblock the
//! reference formatter wrote, in the unit tier (#505). The superblock in
//! `tests/data` is one the formatter wrote at `--version=1.38`; the guest
//! holds the reference itself to the same table
//! (`tests/check_oracle_never_started.rs`).

use super::*;

#[path = "../../tests/common/never_started.rs"]
mod cases;

use cases::{Case, On, CLAIMS_A_START, DEVICE_BYTES};

const SUPERBLOCK: &[u8] = include_bytes!("../../tests/data/never-started-superblock-1.38.bin");

/// An image that is only the byte ranges it was given; the rest reads as zero.
struct Ranges(Vec<(u64, Vec<u8>)>);

impl BlockRead for Ranges {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        buf.fill(0);
        for (start, bytes) in &self.0 {
            let end = start + bytes.len() as u64;
            let (from, to) = (offset.max(*start), (offset + buf.len() as u64).min(end));
            if from < to {
                buf[(from - offset) as usize..(to - offset) as usize]
                    .copy_from_slice(&bytes[(from - start) as usize..(to - start) as usize]);
            }
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        DEVICE_BYTES
    }
}

/// `Err` is a filesystem the checker cannot check: not a clean verdict.
fn verdict(ranges: Vec<(u64, Vec<u8>)>) -> Result<Report> {
    check(&Ranges(ranges))
}

fn reported(ranges: Vec<(u64, Vec<u8>)>) -> bool {
    verdict(ranges).map_or(true, |report| !report.clean())
}

#[test]
fn a_formatted_image_nothing_started_is_recognised() {
    let report = verdict(cases::image(SUPERBLOCK, On::All, |_| {})).unwrap();
    assert!(report.clean(), "{:?}", report.problems);
    assert!(!report.replayed);
    assert_eq!(
        (report.inodes, report.dirents, report.extents),
        (0, 0, 0),
        "it holds nothing yet"
    );
}

#[test]
fn every_superblock_edit_has_the_reference_verdict() {
    let mismatches: Vec<_> = cases::CASES
        .iter()
        .chain(cases::REFERENCE_VARIES)
        .filter_map(
            |Case {
                 name,
                 on,
                 edit,
                 reported: expected,
             }| {
                let ranges = cases::image(SUPERBLOCK, *on, *edit);
                let got = reported(ranges.clone());
                (got != *expected).then(|| {
                    format!(
                        "{name}: expected reported={expected}, got {:?}",
                        verdict(ranges).map(|r| r.problems)
                    )
                })
            },
        )
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A `journal_v2` or `clean` field is what a start leaves. The reference goes
/// by the first flag word's bit 0 alone and starts these anew, so its verdict
/// is that nothing is wrong: this is the one direction this checker says more,
/// because such a superblock is damaged, not new.
#[test]
fn a_superblock_that_records_a_start_is_not_taken_for_new() {
    for (name, edit) in CLAIMS_A_START {
        assert!(
            reported(cases::image(SUPERBLOCK, On::All, *edit)),
            "{name} was accepted"
        );
    }
}
