//! An extent whose data checksum type is not the one its options ask for, and
//! which records no reconcile work, against the reference checker, INSIDE the
//! harness guest only (`chore test:vm`); anywhere else it fails naming that.
//! Nothing skips (#614).
//!
//! Each image is a private copy of a reference-made fixture with one extent's
//! checksum type changed through the writer. The unchanged copy is the control,
//! and both checkers must accept it first. A crash, a hang or an internal fault
//! of the reference stays private: it fails with no output.

#![cfg(feature = "write")]

mod common;

use std::path::Path;

use common::{copy_sparse, fixture, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::Bkey;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::{BlockRead, FileDevice};

const KEY_EXTENT: u8 = 6;
const NOT_SET: &str = "extent_io_opts_not_set";
const REFERENCE_SAYS: &str = "extent with incorrect/missing reconcile opts";

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

fn word(v: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(v[at..at + 8].try_into().unwrap())
}

fn bits(w: u64, at: u32, n: u32) -> u64 {
    (w >> at) & ((1 << n) - 1)
}

/// The first extent of `set` whose value is exactly one crc entry of `kind`
/// (1: crc32, 2: crc64, by the position of its type bit) with checksum type
/// `csum` and no compression, then one written pointer: no reconcile or
/// flags entry, and nothing else.
fn extent(set: &str, kind: u32, csum: u64) -> Bkey {
    let device = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let (at, len) = match kind {
        1 => (24, 16),
        2 => (40, 24),
        _ => unreachable!(),
    };
    btree::walk(&device, &superblock, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .find(|k| {
            let v = &k.value;
            k.key_type == KEY_EXTENT
                && v.len() == len
                && word(v, 0).trailing_zeros() == kind
                && bits(word(v, 0), at, 4) == csum
                && bits(word(v, 0), at + 4, 4) == 0
                && word(v, len - 8).trailing_zeros() == 0
                // The pointer's flags (bits 1..3): 0, written and not cached.
                && bits(word(v, len - 8), 1, 3) == 0
        })
        .unwrap_or_else(|| panic!("{set} has no extent of crc kind {kind}, checksum {csum}"))
}

/// A private copy of `set` holding `key` in place of the extent at its position.
fn image(set: &str, key: Bkey) -> ScratchImage {
    let image = ScratchDir::new("extent-checksum").into_image("changed.img");
    copy_sparse(fixture(&format!("{set}.img")), &image);
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(btree_id::EXTENTS, vec![key.clone()]).unwrap();
    drop(writer);
    let device = FileDevice::open(&image).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let stored = btree::walk(&device, &superblock, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .find(|k| k.pos == key.pos)
        .expect("the changed extent is stored");
    assert_eq!(stored, key, "the extent must be stored as changed");
    image
}

fn ours(image: &Path) -> Vec<(&'static str, String)> {
    check(&FileDevice::open(image).unwrap())
        .unwrap()
        .problems
        .into_iter()
        .map(|problem| (problem.kind, problem.detail))
        .collect()
}

#[track_caller]
fn both_accept_the_control(set: &str) {
    let control = ScratchDir::new("extent-checksum").into_image("control.img");
    copy_sparse(fixture(&format!("{set}.img")), &control);
    let (status, text) = reference(&control);
    assert_eq!(
        status, 0,
        "{set}: the unchanged control must be clean:\n{text}"
    );
    assert!(!text.contains(NOT_SET), "{set}: {text}");
    assert_eq!(ours(&control), Vec::new(), "{set}: the unchanged control");
}

/// The reference names the extent, the reconcile work it needs
/// (`need_rb=data_checksum`), and `data_checksum 0 != N`, N being the option in
/// force; the 0 is the same whatever the extent's checksum type.
#[track_caller]
fn both_report(set: &str, key: &Bkey, image: &Path, reference_compares: &str) {
    let (status, text) = reference(image);
    assert_ne!(status, 0, "{set}: the reference must refuse it:\n{text}");
    assert!(
        text.contains(REFERENCE_SAYS)
            && text.contains(NOT_SET)
            && text.contains("need_rb=data_checksum")
            && text.contains(reference_compares),
        "{set}: wrong native finding:\n{text}"
    );
    let found = ours(image);
    assert!(
        found
            .iter()
            .any(|(kind, detail)| *kind == NOT_SET && detail.contains(&key.pos.to_string())),
        "{set}: extent {} must be reported as {NOT_SET}: {found:?}\n{text}",
        key.pos
    );
}

/// Differential run 42475842401, `default.img` seed 1661200 (#614): one extent's
/// crc32 entry had its checksum type cleared from crc32c to none, on a
/// filesystem whose data_checksum is crc32c. The reference reported the
/// extent's missing reconcile work, and this checker called the image clean.
/// The extent is chosen by its shape, because a regenerated fixture moves it.
#[test]
fn both_checkers_report_an_extent_checksum_cleared_to_none_with_no_reconcile_work() {
    both_accept_the_control("default");
    let mut key = extent("default", 1, 5);
    // The crc32 entry's checksum type is the low half of its fourth byte.
    key.value[3] &= 0xf0;
    let changed = image("default", key.clone());
    both_report("default", &key, &changed, "data_checksum 0 != 1");
}

/// The same disagreement with nothing missing from the data: an extent of
/// `crc64.img` (data_checksum crc64) given a crc32c checksum of its own data,
/// which verifies, in place of its crc64 one.
#[test]
fn both_checkers_report_an_extent_checksummed_with_another_type() {
    both_accept_the_control("crc64");
    let mut key = extent("crc64", 2, 6);
    let device = FileDevice::open(fixture("crc64.img")).unwrap();
    let first = word(&key.value, 0);
    let sectors = bits(first, 3, 9) + 1;
    let pointer = bits(word(&key.value, 16), 4, 44);
    let mut data = vec![0; sectors as usize * 512];
    device.read_at(pointer * 512, &mut data).unwrap();
    let crc32c = fs_bcachefs::csum::compute(5, &data).unwrap();
    // Checksum type 5 at bits 40..43, and the checksum's high bits (48..63)
    // cleared; the second word holds the rest of it.
    let first = (first & !(0xf << 40) & !(0xffff << 48)) | (5 << 40);
    key.value[0..8].copy_from_slice(&first.to_le_bytes());
    key.value[8..16].copy_from_slice(&crc32c.to_le_bytes());
    let changed = image("crc64", key.clone());
    both_report("crc64", &key, &changed, "data_checksum 0 != 2");
    let found = ours(&changed);
    assert!(
        !found.iter().any(|(kind, _)| *kind == "extent_data"),
        "the crc32c checksum must verify: {found:?}"
    );
}
