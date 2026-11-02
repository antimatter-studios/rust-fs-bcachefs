#![cfg(feature = "write")]

#[path = "common/alloc_v4.rs"]
mod alloc_v4;
mod common;

use fs_bcachefs::check::check;
use fs_core::FileDevice;

#[test]
fn alloc_v4_value_sizes_refused_by_the_reference_are_reported() {
    for set in ["default", "lz4"] {
        let control =
            check(&FileDevice::open(common::fixture(&format!("{set}.img"))).unwrap()).unwrap();
        assert!(control.clean(), "{set}: {:?}", control.problems);
        let mut mismatches = Vec::new();
        for &case in alloc_v4::CASES {
            let (image, position) = alloc_v4::plant(set, case);
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            let found = report.problems.iter().any(|problem| {
                problem.kind == "alloc_v4_val_size_bad"
                    && problem.detail.contains(&position.to_string())
            });
            if found != case.3.is_some() {
                mismatches.push(format!("{case:?}: {:?}", report.problems));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{set}: missing or incorrect alloc_v4_val_size_bad: {mismatches:#?}"
        );
    }
}
