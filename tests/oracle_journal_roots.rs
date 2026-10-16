//! A btree root the superblock records and the one the journal records
//! (#166). S1's journal entry list has `btree_root` entries, "Btree root
//! pointers, recorded every journal write", so a filesystem shut down
//! cleanly holds two copies of every root: the clean section's and the
//! newest flush entry's. The reference reads the journal even after a clean
//! shutdown (it prints the sequence it recovered from), and opened cleanly
//! two images whose superblock root had been damaged (the differential run,
//! #164), so it does not depend on the superblock's copy alone.
//!
//! Observed here first: on every clean fixture the two copies agree. Then:
//! a copy of `default` whose superblock records another btree's root for
//! the subvolumes btree opens, reads every file the fixture's record holds,
//! and checks clean, through the journal's roots.

mod common;

use common::{fixture, manifest, SETS};
use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::btree_id;
use fs_bcachefs::superblock::{Superblock, SB_HEADER_BYTES};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

/// The subvolumes btree (S1 11.3), as src/check.rs numbers it.
const SUBVOLUMES: u8 = 8;

const UNPACKED: BkeyFormat = BkeyFormat {
    key_u64s: 5,
    nr_fields: 6,
    bits: [0; 6],
    field_offset: [0; 6],
};

#[test]
fn on_every_clean_fixture_the_superblock_and_the_journal_record_the_same_roots() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        assert!(sb.is_clean(), "{set} is a clean fixture");
        let replay = fs_bcachefs::journal::replay(&dev, &sb)
            .unwrap_or_else(|e| panic!("{set}: the journal does not replay: {e}"));
        for root in sb.btree_roots().unwrap() {
            let key = bkey::decode(&root.key, &UNPACKED).unwrap();
            let theirs = replay
                .roots
                .iter()
                .find(|(id, _, _)| *id == root.btree_id)
                .unwrap_or_else(|| {
                    panic!(
                        "{set}: the journal records no root for btree {}",
                        root.btree_id
                    )
                });
            assert_eq!(
                (theirs.1, &theirs.2),
                (root.level, &key),
                "{set}: btree {}'s root differs between the superblock and the journal",
                root.btree_id
            );
        }
    }
}

#[test]
fn a_superblock_root_that_names_another_btrees_node_is_read_through_the_journal() {
    let dir = common::ScratchDir::new("journal-roots");
    let img = dir.join("subvolumes-root.img");
    std::fs::copy(fixture("default.img"), &img).unwrap();

    // The subvolumes btree's root key, overwritten in the superblock with the
    // inodes btree's: a pointer to a node that is not that btree's.
    let mut raw = std::fs::read(&img).unwrap();
    let sb = Superblock::read(&FileDevice::open(&img).unwrap()).unwrap();
    let roots = sb.btree_roots().unwrap();
    let key = |id: u8| {
        roots
            .iter()
            .find(|r| r.btree_id == id)
            .unwrap_or_else(|| panic!("default records no root for btree {id}"))
            .key
            .clone()
    };
    let (victim, donor) = (key(SUBVOLUMES), key(btree_id::INODES));
    assert_eq!(
        victim.len(),
        donor.len(),
        "the two root keys differ in length"
    );
    let start = 4096;
    let end = start + SB_HEADER_BYTES + sb.u64s as usize * 8;
    let at = raw[start..end]
        .windows(victim.len())
        .position(|w| w == victim.as_slice())
        .expect("the subvolumes root key is in the primary superblock")
        + start;
    raw[at..at + donor.len()].copy_from_slice(&donor);
    let csum = fs_bcachefs::csum::compute(sb.csum_type(), &raw[start + 16..end]).unwrap();
    raw[start..start + 16].fill(0);
    raw[start..start + 8].copy_from_slice(&csum.to_le_bytes());
    std::fs::write(&img, &raw).unwrap();
    let damaged = Superblock::read(&FileDevice::open(&img).unwrap()).unwrap();
    assert_ne!(
        damaged.btree_roots().unwrap(),
        roots,
        "the damage did not reach the superblock"
    );

    let fs = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap_or_else(|e| panic!("a damaged superblock root refuses the filesystem: {e}"));
    let mut files = 0;
    for e in manifest("default") {
        let node = fs
            .resolve(&e.path)
            .unwrap_or_else(|err| panic!("{}: {err}", e.path));
        if e.kind == "file" {
            let data = fs.read(node.ino).unwrap();
            assert_eq!(
                Some(format!("{:x}", Sha256::digest(&data))),
                e.sha256,
                "{}: contents",
                e.path
            );
            files += 1;
        }
    }
    assert!(files > 300, "only {files} files read");
    let report = fs_bcachefs::check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report.clean(),
        "the checker, through the journal's roots: {:?}",
        report.problems
    );
    std::fs::remove_file(&img).ok();
}
