#![cfg(feature = "write")]

mod common;
#[path = "common/subvolume_states.rs"]
mod subvolume_states;

use fs_bcachefs::check::{check, Report};
use fs_core::FileDevice;
use subvolume_states::{plant, CHILDREN, DELETED, LIVE, SETS, UNLINKED};

fn report(set: &str, id: u64, state: u32, unreferenced: bool) -> Report {
    let (image, _) = plant(set, id, state, unreferenced);
    check(&FileDevice::open(&image).unwrap()).unwrap()
}

fn has(report: &Report, kind: &str) -> bool {
    report.problems.iter().any(|problem| problem.kind == kind)
}

#[test]
fn a_non_live_root_subvolume_is_not_a_usable_root() {
    for set in SETS {
        for state in [LIVE, UNLINKED] {
            let report = report(set, 1, state, false);
            assert_eq!(
                has(&report, "root_subvol_missing"),
                state != LIVE,
                "{set} {state:#x}: {report:?}"
            );
            if state == LIVE {
                assert!(report.clean(), "{set}: {report:?}");
            }
        }
    }
}

#[test]
fn a_non_live_subvolume_is_not_a_dirent_target() {
    for (set, id) in CHILDREN {
        for state in [LIVE, UNLINKED, DELETED] {
            let report = report(set, id, state, false);
            assert_eq!(
                has(&report, "dirent_to_missing_subvolume"),
                state != LIVE,
                "{set} {id} {state:#x}: {report:?}"
            );
            if state == LIVE {
                assert!(report.clean(), "{set}: {report:?}");
            }
        }
    }
}

#[test]
fn references_to_a_deleted_subvolume_are_inconsistent() {
    for (set, id) in CHILDREN {
        let report = report(set, id, DELETED, false);
        assert!(
            has(&report, "snapshot_subvol_state_mismatch"),
            "{set} {id}: {report:?}"
        );
        if id == 1 || (set == "kernel" && id == 2) {
            assert!(
                has(&report, "snapshot_tree_to_missing_subvol"),
                "{set} {id}: {report:?}"
            );
        }
        if id != 1 {
            assert!(
                has(&report, "subvol_children_entry_wrong"),
                "{set} {id}: {report:?}"
            );
        }
    }
}

#[test]
fn an_unreferenced_deleted_subvolume_is_accepted() {
    for set in SETS {
        let report = report(set, 1, DELETED, true);
        assert!(report.clean(), "{set}: {report:?}");
    }
}
