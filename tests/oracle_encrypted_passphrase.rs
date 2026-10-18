//! The passphrase fixture is a complete filesystem (#327, part of #273).
//!
//! `encrypted-passphrase.img` is the image the passphrase unlock (#273) is
//! learned from, so it must hold the same tree as `encrypted.img`, not only
//! the formatter's initial superblocks (docs/clean-room.md, "Encryption:
//! the crypt field"). These tests check it was shut down cleanly and that
//! the reference implementation, given the recorded passphrase, reads the
//! source tree from it.

mod common;

use common::{fixture, fixtures_dir, manifest, read_text};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

/// The image carries a clean field saying it was shut down cleanly, which
/// an image of superblocks only does not.
#[test]
fn the_passphrase_image_was_shut_down_cleanly() {
    let record = read_text("encrypted-passphrase.format.txt");
    let dev = FileDevice::open(fixture("encrypted-passphrase.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    assert!(
        sb.is_clean(),
        "encrypted-passphrase.img is not marked clean: it holds no complete filesystem:\n{record}"
    );
}

/// The reference implementation, unlocked with the recorded passphrase and
/// mounted over a copy of the image, reads the same tree from it as the
/// source tree `encrypted.img` was formatted from.
#[test]
fn the_reference_reads_the_source_tree_with_the_recorded_passphrase() {
    let record = read_text("encrypted-passphrase.mounted.txt");
    assert!(
        fixtures_dir()
            .join("encrypted-passphrase.mounted.json")
            .exists(),
        "the reference implementation did not read encrypted-passphrase.img:\n{record}"
    );
    let view = |set: &str| -> Vec<String> {
        manifest(set)
            .into_iter()
            .map(|e| {
                format!(
                    "{} {} {:o} {:?} {:?} {:?}",
                    e.path, e.kind, e.mode, e.size, e.sha256, e.target
                )
            })
            .collect()
    };
    let (want, theirs) = (view("encrypted"), view("encrypted-passphrase.mounted"));
    assert!(!want.is_empty(), "the source tree's manifest names nothing");
    if let Some(i) = (0..want.len().max(theirs.len())).find(|&i| want.get(i) != theirs.get(i)) {
        panic!(
            "entry {i}: the source tree's {:?}, the reference mount's {:?}\n{record}",
            want.get(i),
            theirs.get(i)
        );
    }
}
