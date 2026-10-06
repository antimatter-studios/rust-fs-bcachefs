//! What only an aged filesystem has: hard links, link counts as the mount
//! reported them, and an image left behind without a clean shutdown.
//!
//! The `aged` set was mounted and aged by the reference implementation in
//! the harness guest (scripts/guest-age.py); its manifest was taken through
//! that mount, so each entry's inode number and link count are what the
//! filesystem itself reported.

mod common;

use common::{fixture, manifest};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn open(set: &str) -> fs_bcachefs::Result<Filesystem<FileDevice>> {
    Filesystem::open(FileDevice::open(fixture(&format!("{set}.img"))).unwrap())
}

#[test]
fn every_inode_number_and_link_count_is_the_mounts() {
    let fs = open("aged").unwrap();
    let m = manifest("aged");
    let mut checked = 0;
    for e in &m {
        let (Some(ino), Some(nlink)) = (e.ino, e.nlink) else {
            panic!(
                "aged {}: the manifest has no inode number or link count",
                e.path
            );
        };
        let ours = fs.lookup(&e.path).unwrap();
        assert_eq!(ours, ino, "aged {}: inode number", e.path);
        assert_eq!(
            fs.inode(ours).unwrap().link_count(),
            nlink,
            "aged {}: link count",
            e.path
        );
        checked += 1;
    }
    assert!(checked > 2000, "aged: only {checked} entries checked");
}

#[test]
fn hard_links_resolve_to_one_inode() {
    let fs = open("aged").unwrap();
    let a = fs.lookup("/links/orig.txt").unwrap();
    assert_eq!(fs.lookup("/links/second.txt").unwrap(), a);
    assert_eq!(fs.lookup("/links/sub/third.txt").unwrap(), a);
    assert_eq!(fs.inode(a).unwrap().link_count(), 3);
}

/// Without a clean shutdown the superblock's btree roots are stale and the
/// newest keys are only in the journal. The image is read through a replay
/// of the journal, in memory: it shows the burst of changes under /late
/// that only the journal carries, and the device is not written.
#[test]
fn an_uncleanly_unmounted_image_is_read_through_its_journal() {
    use sha2::{Digest, Sha256};
    let path = fixture("aged-unclean.img");
    let before = Sha256::digest(std::fs::read(&path).unwrap());
    let fs = open("aged-unclean").unwrap_or_else(|e| panic!("aged-unclean: {e}"));
    // As many entries under /late as the reference saw after its replay
    // (what the burst's last writes left of it is the reference's call).
    let want = manifest("aged-unclean")
        .iter()
        .filter(|e| e.path.starts_with("/late/"))
        .count();
    assert!(want > 200, "the burst left only {want} entries");
    let late = fs.lookup("/late").unwrap();
    assert_eq!(fs.readdir(late).unwrap().len(), want);
    drop(fs);
    let after = Sha256::digest(std::fs::read(&path).unwrap());
    assert_eq!(before, after, "the image changed while it was read");
}
