//! The `nr_inodes` accounting key on copies of reference-made fixtures
//! (#564). tests/check_oracle.rs repeats each change against the reference
//! checker itself, in the guest.
#![cfg(feature = "write")]

mod common;
#[path = "common/nr_inodes.rs"]
mod nr_inodes;

use fs_bcachefs::check::check;
use fs_core::FileDevice;

#[test]
fn an_inode_count_that_disagrees_with_the_inodes_is_reported() {
    let mut found = Vec::new();
    for set in ["default", "kernel", "kernel-rootsnap"] {
        let unchanged = nr_inodes::copy(set);
        let report = check(&FileDevice::open(&unchanged).unwrap()).unwrap();
        assert!(report.clean(), "{set} unchanged: {:?}", report.problems);
        for change in nr_inodes::CHANGES {
            let image = nr_inodes::plant(set, change);
            let report = check(&FileDevice::open(&image).unwrap()).unwrap();
            if !report.problems.iter().any(|problem| {
                problem.kind == "accounting_mismatch" && problem.detail.contains("nr_inodes")
            }) {
                found.push(format!("{set} {change:?}: {:?}", report.problems));
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}
