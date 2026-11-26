//! Key fields the nightly differential runs measured (#568, #569, #615, #623)
//! on copies of reference-made fixtures. tests/check_oracle.rs repeats each
//! case against the reference checker itself, in the guest.
#![cfg(feature = "write")]

mod common;
#[path = "common/measured_keys.rs"]
mod measured_keys;

use fs_bcachefs::check::check;
use fs_core::FileDevice;

#[test]
fn the_measured_key_fields_are_judged_as_the_reference_judged_them() {
    let mut mismatches = Vec::new();
    for case in measured_keys::CASES {
        let (image, position) = measured_keys::plant(case);
        let report = check(&FileDevice::open(&image).unwrap()).unwrap();
        let agreed = match case.verdict {
            Some((_, kind)) => report.problems.iter().any(|problem| {
                problem.kind == kind && problem.detail.contains(&position.to_string())
            }),
            None => report.clean(),
        };
        if !agreed {
            mismatches.push(format!("{case:?} at {position}: {:?}", report.problems));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
