//! A journal entry recovery does not need may be damaged without making
//! the filesystem unreadable (#165). The journal is a ring: entries older
//! than the replay window are stale writes not yet overwritten, and the
//! reference replays `aged-unclean` without looking at them. This reader
//! parsed every entry in every bucket and refused the filesystem over a
//! damaged one anywhere. An entry inside the window is still damage that
//! matters, and is still refused.
//!
//! The damage: an entry's sub-entry claims more u64s than the entry holds,
//! with the entry's checksum recomputed, so it is no torn write.

mod common;

use common::fixture;
use fs_bcachefs::journal::{self, Jset};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::Filesystem;
use fs_core::{BlockRead, FileDevice};
use std::io::{Seek, SeekFrom, Write};

fn scratch(name: &str, test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-journal-window-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("{test}.img"));
    std::fs::copy(fixture(name), &img).unwrap();
    img
}

/// Every entry of the image's journal, and the replay window's first sequence.
fn entries(img: &std::path::Path) -> (Vec<Jset>, u64) {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let all = journal::read_entries(&dev, &sb).unwrap();
    let newest = all.iter().rev().find(|j| j.flush).expect("a flush entry");
    let from = newest.last_seq;
    (all, from)
}

/// Make entry `j`'s first sub-entry claim 0xffff u64s, and recompute the
/// entry's checksum over its new bytes.
fn damage(img: &std::path::Path, j: &Jset) {
    let dev = FileDevice::open(img).unwrap();
    let mut b = vec![0u8; j.bytes];
    dev.read_at(j.sector * 512, &mut b).unwrap();
    drop(dev);
    b[56..58].copy_from_slice(&0xffffu16.to_le_bytes());
    let csum_type = (u32::from_le_bytes(b[36..40].try_into().unwrap()) & 0xf) as u8;
    let sum = fs_bcachefs::csum::compute(csum_type, &b[16..]).unwrap();
    b[0..16].fill(0);
    b[0..8].copy_from_slice(&sum.to_le_bytes());
    let mut f = std::fs::OpenOptions::new().write(true).open(img).unwrap();
    f.seek(SeekFrom::Start(j.sector * 512)).unwrap();
    f.write_all(&b).unwrap();
}

#[test]
fn a_damaged_entry_older_than_the_replay_window_is_skipped() {
    let img = scratch("aged-unclean.img", "stale-entry");
    let (all, from) = entries(&img);
    let stale = all
        .iter()
        .find(|j| j.seq < from && j.bytes > 64)
        .expect("aged-unclean's journal holds an entry older than its replay window");
    damage(&img, stale);

    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap_or_else(|e| {
        panic!(
            "a damaged entry ({}) older than the replay window (from {from}) made the \
             filesystem unreadable: {e}",
            stale.seq
        )
    });
    fs.readdir(fs_bcachefs::inode::ROOT_INO)
        .expect("the root directory reads");
}

#[test]
fn a_damaged_entry_inside_the_replay_window_is_refused() {
    let img = scratch("aged-unclean.img", "window-entry");
    let (all, from) = entries(&img);
    let inside = all
        .iter()
        .find(|j| j.seq >= from && j.bytes > 64)
        .expect("aged-unclean's replay window holds an entry");
    damage(&img, inside);

    assert!(
        Filesystem::open(FileDevice::open(&img).unwrap()).is_err(),
        "a damaged entry ({}) inside the replay window (from {from}) was replayed",
        inside.seq
    );
}
