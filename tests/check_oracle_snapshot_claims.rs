//! Two subvolumes that claim one snapshot, against the reference checker,
//! INSIDE the harness guest only (`chore test:vm`); anywhere else it fails
//! naming that. Nothing skips (#521).
//!
//! Each image is a private copy of a reference-made fixture, given a copy of a
//! subvolume the reference made at an unused id. A crash, a hang or an internal
//! fault of the reference stays private: it fails with no output.

#![cfg(feature = "write")]

mod common;
#[path = "common/key_field_bytes.rs"]
#[allow(dead_code)]
mod key_field_bytes;

use std::path::Path;

use common::{copy_sparse, fixture, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::Bkey;
use fs_bcachefs::btree;
use fs_bcachefs::check::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

const SETS: [&str; 4] = ["default", "aged", "kernel", "kernel-rootsnap"];
const CHILDREN: [(&str, u64); 3] = [("kernel", 2), ("kernel", 3), ("kernel-rootsnap", 2)];
const SUBVOLUMES: u8 = 8;
const KEY_SUBVOLUME: u8 = 21;
const LIVE: u32 = 0x4ad5_447e;
const UNLINKED: u32 = 0x2d35_8e8f;
const DELETED: u32 = 0x3c6b_2d4c;
const UNUSED: u64 = 4096;
const CLAIMED: &str = "snapshot_claimed_twice";
const REFERENCE_SAYS: &str = "which it backrefs, and also by";

fn reference(image: &Path) -> (i32, String) {
    let out = common::reference_runs::output(&["fsck", "-n"], image);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let faulted = [
        "BUG",
        "ssertion",
        "acktrace",
        "Call Trace",
        "panicked at",
        "Kernel panic",
    ]
    .iter()
    .any(|fault| text.contains(fault));
    let status = out.status.code();
    assert!(
        matches!(status, Some(0 | 1 | 9 | 12 | 13)) && !faulted,
        "reference fault: 1; no case details retained"
    );
    (status.unwrap(), text)
}

fn subvolume(set: &str, id: u64) -> Bkey {
    let device = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    btree::walk(&device, &superblock, SUBVOLUMES)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == KEY_SUBVOLUME && k.pos.offset == id)
        .unwrap_or_else(|| panic!("{set} has no reference-made subvolume {id}"))
}

/// A copy of `key` at `id`, its parents cleared so that nothing else is at
/// odds with it, in `state`.
fn copy_at(key: &Bkey, id: u64, state: u32) -> Bkey {
    let mut copy = with_state(key, state);
    copy.pos.offset = id;
    copy.value[16..24].fill(0);
    copy
}

fn with_state(key: &Bkey, state: u32) -> Bkey {
    let mut copy = key.clone();
    copy.value[40..44].copy_from_slice(&state.to_le_bytes());
    copy
}

fn image(set: &str, keys: Vec<Bkey>) -> ScratchImage {
    let image = ScratchDir::new("snapshot-claims").into_image("changed.img");
    copy_sparse(fixture(&format!("{set}.img")), &image);
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(SUBVOLUMES, keys).unwrap();
    drop(writer);
    image
}

fn kinds(image: &Path) -> Vec<&'static str> {
    check(&FileDevice::open(image).unwrap())
        .unwrap()
        .problems
        .iter()
        .map(|problem| problem.kind)
        .collect()
}

#[track_caller]
fn both_name_the_claim(name: &str, image: &Path) {
    let (status, text) = reference(image);
    assert_eq!(status, 1, "{name}: {text}");
    assert!(text.contains(REFERENCE_SAYS), "{name}: {text}");
    let kinds = kinds(image);
    assert!(kinds.contains(&CLAIMED), "{name}: {kinds:?}");
}

#[track_caller]
fn both_accept(name: &str, image: &Path) {
    let (status, text) = reference(image);
    assert_eq!(status, 0, "{name}: {text}");
    assert_eq!(kinds(image), Vec::<&str>::new(), "{name}");
}

#[test]
fn both_checkers_name_a_second_subvolume_claiming_a_snapshot() {
    for set in SETS {
        let root = subvolume(set, 1);
        // An unset word is read as live, whatever else the checkers say of it.
        for state in [LIVE, UNLINKED, 0, LIVE ^ 1] {
            let image = image(set, vec![copy_at(&root, UNUSED, state)]);
            both_name_the_claim(&format!("{set} {state:#x}"), &image);
        }
    }
    for (set, id) in CHILDREN {
        let child = subvolume(set, id);
        let image = image(set, vec![copy_at(&child, UNUSED, LIVE)]);
        both_name_the_claim(&format!("{set} of {id}"), &image);
    }
}

#[test]
fn both_checkers_name_the_claim_with_parents_and_with_a_third_subvolume() {
    for set in SETS {
        let root = subvolume(set, 1);
        let mut parents_kept = copy_at(&root, UNUSED, LIVE);
        parents_kept.value[16..24].copy_from_slice(&root.value[16..24]);
        both_name_the_claim(&format!("{set} parents"), &image(set, vec![parents_kept]));
        both_name_the_claim(
            &format!("{set} third"),
            &image(
                set,
                vec![
                    copy_at(&root, UNUSED, LIVE),
                    copy_at(&root, UNUSED + 1, LIVE),
                ],
            ),
        );
        // The deleted one is no claim, and the unlinked one still is.
        both_name_the_claim(
            &format!("{set} third deleted"),
            &image(
                set,
                vec![
                    copy_at(&root, UNUSED, UNLINKED),
                    copy_at(&root, UNUSED + 1, DELETED),
                ],
            ),
        );
    }
}

#[test]
fn a_deleted_subvolume_claims_nothing() {
    for set in SETS {
        let root = subvolume(set, 1);
        let image = image(set, vec![copy_at(&root, UNUSED, DELETED)]);
        both_accept(&format!("{set} deleted"), &image);
    }
    for (set, id) in CHILDREN {
        let child = subvolume(set, id);
        let image = image(set, vec![copy_at(&child, UNUSED, DELETED)]);
        both_accept(&format!("{set} deleted of {id}"), &image);
    }
    // A word a bit from deleted is read as deleted: both say so of the word,
    // and neither finds a claim.
    for set in SETS {
        let root = subvolume(set, 1);
        let image = image(set, vec![copy_at(&root, UNUSED, DELETED ^ 1)]);
        let (status, text) = reference(&image);
        assert_eq!(status, 9, "{set}: {text}");
        assert!(!text.contains(REFERENCE_SAYS), "{set}: {text}");
        assert_eq!(kinds(&image), ["subvol_state_bitflip"], "{set}");
    }
}

/// The reference's own finding differs, since it no longer takes the root for
/// the snapshot's owner, and it reports all the same.
#[test]
fn both_checkers_report_an_unlinked_root_beside_a_live_claimant() {
    for set in SETS {
        let root = subvolume(set, 1);
        let image = image(
            set,
            vec![with_state(&root, UNLINKED), copy_at(&root, UNUSED, LIVE)],
        );
        let (status, text) = reference(&image);
        assert_eq!(status, 9, "{set}: {text}");
        let kinds = kinds(&image);
        assert!(kinds.contains(&CLAIMED), "{set}: {kinds:?}");
    }
}

#[test]
fn both_checkers_name_the_claim_at_every_older_version() {
    for set in SETS {
        let root = subvolume(set, 1);
        for minor in [0u16, 20, 33, 36, 38] {
            for state in [LIVE, UNLINKED] {
                let image = image(set, vec![copy_at(&root, UNUSED, state)]);
                key_field_bytes::lower_version(&image, minor);
                both_name_the_claim(&format!("{set} 1.{minor} {state:#x}"), &image);
            }
        }
    }
}
