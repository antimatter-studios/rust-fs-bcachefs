//! This checker against what the reference checker said of a superblock on a
//! filesystem the reference formatter started, in the unit tier (#533). The
//! image in `tests/data` is one the formatter wrote at `--version=1.39` on a
//! 256 MiB file: its nonzero 4 KiB runs, each an offset and a length as two
//! little-endian u64s and then its bytes. The guest holds the reference itself
//! to the same table (`tests/check_oracle_started_superblock.rs`).

use super::never_started_tests::{cases as never_started, Ranges};
use super::*;

#[path = "../../tests/common/started_superblock.rs"]
mod cases;

use never_started::{Edit, On};

const IMAGE: &[u8] = include_bytes!("../../tests/data/started-image-1.39.bin");

fn formatted() -> Vec<(u64, Vec<u8>)> {
    let mut runs = Vec::new();
    let mut rest = IMAGE;
    while !rest.is_empty() {
        let at = u64::from_le_bytes(rest[..8].try_into().unwrap());
        let len = u64::from_le_bytes(rest[8..16].try_into().unwrap()) as usize;
        runs.push((at, rest[16..16 + len].to_vec()));
        rest = &rest[16 + len..];
    }
    runs
}

/// The image with a case's edit laid over it: the copies come last, and
/// [`Ranges`] lets the last range holding a byte say what it is.
fn edited(on: On, edit: Edit) -> Ranges {
    let mut image = Ranges(formatted());
    let copies = cases::edited_copies(
        &mut |at, len| {
            let mut bytes = vec![0; len];
            image.read_at(at, &mut bytes).unwrap();
            bytes
        },
        on,
        edit,
    );
    image.0.extend(copies);
    image
}

#[test]
fn the_image_is_clean_as_the_formatter_left_it() {
    let report = check(&Ranges(formatted())).unwrap();
    assert!(report.clean(), "{:?}", report.problems);
    assert!(report.inodes > 0, "it was started, so it holds its root");
}

#[test]
fn every_superblock_edit_of_a_started_image_has_the_reference_verdict() {
    let mismatches: Vec<_> = cases::CASES
        .iter()
        .filter_map(|case| {
            let result = check(&edited(case.on, case.edit));
            // `Err` is a filesystem the checker cannot check: not a clean verdict.
            let reported = result.as_ref().map_or(true, |report| !report.clean());
            (reported != case.reported).then(|| {
                format!(
                    "{}: expected reported={}, got {:?}",
                    case.name,
                    case.reported,
                    result.map(|report| report.problems)
                )
            })
        })
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
