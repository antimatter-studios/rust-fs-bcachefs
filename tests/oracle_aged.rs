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
/// newest keys are only in the journal. Reading the roots anyway would show
/// an old tree as if it were current; until the journal is replayed
/// (#5), the image is refused.
#[test]
fn an_uncleanly_unmounted_image_is_refused_not_read_stale() {
    match open("aged-unclean") {
        Err(fs_bcachefs::Error::Unsupported(m)) => assert!(
            m.contains("not cleanly unmounted"),
            "refused, but for another reason: {m}"
        ),
        Err(e) => panic!("refused with the wrong error: {e}"),
        Ok(fs) => panic!(
            "read an unclean image from its stale roots: / has {} entries",
            fs.readdir(4096).map(|d| d.len()).unwrap_or(0)
        ),
    }
}
