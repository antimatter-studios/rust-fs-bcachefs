//! A replicas accounting key on copies of reference-made fixtures, held to
//! the reference checker's verdicts measured in docs/clean-room.md.
//! tests/check_oracle.rs repeats each case against the reference checker
//! itself, in the guest.
#![cfg(feature = "write")]

mod common;
#[path = "common/replicas_accounting.rs"]
mod replicas_accounting;

use fs_bcachefs::check::check;
use fs_core::FileDevice;
use replicas_accounting::{
    plant, Case, DEVICES_AND_COPIES, HOLDING_NOTHING, SETS, UNKNOWN_DATA_TYPES, WRONG_COUNTERS,
};

fn mismatches(cases: &[Case]) -> Vec<String> {
    let mut found = Vec::new();
    for set in SETS {
        for case in cases {
            let (image, position) = plant(set, case);
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            let judged = match case.verdict {
                Some((_, kind)) => report.problems.iter().any(|problem| {
                    problem.kind == kind && problem.detail.contains(&position.to_string())
                }),
                None => report.clean(),
            };
            if !judged {
                found.push(format!("{set} {case:?}: {:?}", report.problems));
            }
        }
    }
    found
}

#[test]
fn a_replicas_key_naming_a_missing_device_or_requiring_no_copies_is_reported() {
    let found = mismatches(DEVICES_AND_COPIES);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_replicas_key_with_an_unknown_data_type_is_reported() {
    let found = mismatches(UNKNOWN_DATA_TYPES);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_replicas_key_holding_nothing_is_not_judged() {
    let found = mismatches(HOLDING_NOTHING);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn replicas_counters_disagreeing_with_allocations_are_reported() {
    let found = mismatches(WRONG_COUNTERS);
    assert!(found.is_empty(), "{found:#?}");
}
