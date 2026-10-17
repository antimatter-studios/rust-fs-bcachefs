//! Host-side coverage of selected superblock bytes. The same cases are
//! judged independently by the reference checker, lister and mount in
//! write_oracle.rs inside the harness guest.
#![cfg(feature = "write")]

mod common;
#[path = "common/writer_superblock.rs"]
mod writer_superblock;

fn exercise(erase_primary: bool) {
    // Cover both backup locations, independently of equal-sequence ties.
    for backup in [1, 2] {
        let dir = common::ScratchDir::new("writer-superblock");
        let img = dir.copy_fixture("write-study/base.img");
        let selected = writer_superblock::prepare(&img, backup, erase_primary);
        writer_superblock::create_and_check(&img, &selected);
    }
}

#[test]
fn a_write_preserves_the_newer_backups_superblock() {
    exercise(false);
}

#[test]
fn a_write_recovers_the_superblock_from_a_zeroed_primary() {
    exercise(true);
}
