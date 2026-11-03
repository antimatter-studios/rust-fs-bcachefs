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
    lower_version, plant_interior_snapshot, plant_snapshot, plant_subvolume, plant_type_byte,
    type_byte_is_clean, type_bytes, Edit, WordCase, INTERIOR_SNAPSHOT_SETS,
    INTERIOR_SNAPSHOT_WORDS, OLDER_MINORS, OLDER_SNAPSHOT_JUDGED, OLDER_SNAPSHOT_QUIET,
    OLDER_SUBVOLUME_JUDGED, OLDER_SUBVOLUME_QUIET, OTHER_STATE_SETS, SNAPSHOT_PAD, SNAPSHOT_SETS,
    SNAPSHOT_STATE_WORDS, SUBVOLUME_OTHER_STATE_WORDS, SUBVOLUME_PAD_WORDS, SUBVOLUME_SETS,
    SUBVOLUME_STATE, SUBVOLUME_STATE_WORDS, TYPE_BYTE_DIRENTS,
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

type Plant = fn(&str, &[Edit]) -> (common::ScratchImage, fs_bcachefs::bkey::Bpos);

fn judged_as_measured(sets: &[&str], table: &[WordCase], plant: Plant) {
    let mut mismatches = Vec::new();
    for set in sets {
        for &(edits, finding) in table {
            let (image, position) = plant(set, edits);
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            let agrees = match finding {
                None => report.clean(),
                Some((_, kind)) => reports(&report, kind, &position.to_string()),
            };
            if !agrees {
                mismatches.push(format!("{set} {edits:?}: {:?}", report.problems));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn a_short_snapshot_value_is_named_as_the_reference_names_it() {
    judged_as_measured(
        &key_field_bytes::VALUE_SIZE_SETS,
        key_field_bytes::SNAPSHOT_VALUE_SIZES,
        plant_snapshot,
    );
}

#[test]
fn a_short_subvolume_value_is_named_as_the_reference_names_it() {
    judged_as_measured(
        &key_field_bytes::VALUE_SIZE_SETS,
        key_field_bytes::SUBVOLUME_VALUE_SIZES,
        plant_subvolume,
    );
}

#[test]
fn a_snapshot_state_word_is_judged_as_the_reference_judges_it() {
    judged_as_measured(&SNAPSHOT_SETS, SNAPSHOT_STATE_WORDS, plant_snapshot);
}

#[test]
fn an_interior_snapshot_state_word_is_judged_as_the_reference_judges_it() {
    judged_as_measured(
        &INTERIOR_SNAPSHOT_SETS,
        INTERIOR_SNAPSHOT_WORDS,
        plant_interior_snapshot,
    );
}

#[test]
fn a_subvolume_state_word_is_judged_as_the_reference_judges_it() {
    judged_as_measured(&SUBVOLUME_SETS, SUBVOLUME_STATE_WORDS, plant_subvolume);
}

#[test]
fn a_word_near_a_subvolume_state_other_than_live_is_judged_as_the_reference_judges_it() {
    judged_as_measured(
        &OTHER_STATE_SETS,
        SUBVOLUME_OTHER_STATE_WORDS,
        plant_subvolume,
    );
}

#[test]
fn a_subvolume_pad_word_is_judged_as_the_reference_judges_it() {
    judged_as_measured(&SUBVOLUME_SETS, SUBVOLUME_PAD_WORDS, plant_subvolume);
}

#[test]
fn state_words_are_judged_as_the_reference_judges_them_below_the_required_version() {
    let kinds = [
        "snapshot_state_bad",
        "subvol_state_bad",
        "subvol_state_invalid",
    ];
    let mut mismatches = Vec::new();
    for minor in OLDER_MINORS {
        for (plant, quiet, judged) in [
            (
                plant_snapshot as Plant,
                OLDER_SNAPSHOT_QUIET,
                OLDER_SNAPSHOT_JUDGED,
            ),
            (
                plant_subvolume as Plant,
                OLDER_SUBVOLUME_QUIET,
                OLDER_SUBVOLUME_JUDGED,
            ),
        ] {
            for &edits in quiet {
                let (image, _) = plant("default", edits);
                lower_version(&image, minor);
                let report = check(&FileDevice::open(&image).unwrap()).unwrap();
                if report.problems.iter().any(|p| kinds.contains(&p.kind)) {
                    mismatches.push(format!("1.{minor} {edits:?}: {:?}", report.problems));
                }
            }
            for &(edits, finding) in judged {
                let (image, position) = plant("default", edits);
                lower_version(&image, minor);
                let report = check(&FileDevice::open(&image).unwrap()).unwrap();
                if !reports(&report, finding.unwrap().1, &position.to_string()) {
                    mismatches.push(format!("1.{minor} {edits:?}: {:?}", report.problems));
                }
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
