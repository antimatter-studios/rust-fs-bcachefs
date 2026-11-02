//! The snapshot pad, subvolume state and dirent type bytes, on copies of
//! reference-made fixtures, held to the reference checker's verdicts measured
//! in docs/clean-room.md. tests/check_oracle.rs repeats each case against the
//! reference checker itself, in the guest.
#![cfg(feature = "write")]

mod common;
#[path = "common/key_field_bytes.rs"]
mod key_field_bytes;

use fs_bcachefs::check::{check, Report};
use fs_core::FileDevice;
use key_field_bytes::{
    plant_snapshot, plant_subvolume, plant_type_byte, type_byte_is_clean, type_bytes, SNAPSHOT_PAD,
    SNAPSHOT_SETS, SUBVOLUME_SETS, SUBVOLUME_STATE, TYPE_BYTE_DIRENTS,
};

fn reports(report: &Report, kind: &str, position: &str) -> bool {
    report
        .problems
        .iter()
        .any(|problem| problem.kind == kind && problem.detail.contains(position))
}

#[test]
fn a_nonzero_snapshot_pad_is_reported_and_nothing_else_is_called_one() {
    let mut mismatches = Vec::new();
    for set in SNAPSHOT_SETS {
        for &(edits, refused) in SNAPSHOT_PAD {
            let (image, position) = plant_snapshot(set, edits);
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            if reports(&report, "snapshot_pad_nonzero", &position.to_string()) != refused {
                mismatches.push(format!("{set} {edits:?}: {:?}", report.problems));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn an_unset_subvolume_state_is_reported_and_nothing_else_is_called_unset() {
    let mut mismatches = Vec::new();
    for set in SUBVOLUME_SETS {
        for &(edits, unset) in SUBVOLUME_STATE {
            let (image, position) = plant_subvolume(set, edits);
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            if reports(&report, "subvol_state_bad", &position.to_string()) != unset {
                mismatches.push(format!("{set} {edits:?}: {:?}", report.problems));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn dirent_type_bytes_are_judged_as_the_reference_judges_them() {
    let mut mismatches = Vec::new();
    for (set, original) in TYPE_BYTE_DIRENTS {
        for byte in type_bytes(set, original) {
            let image = plant_type_byte(set, original, byte);
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            if report.clean() != type_byte_is_clean(original, byte) {
                mismatches.push(format!(
                    "{set} {original:#x} {byte:#x}: {:?}",
                    report.problems
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
