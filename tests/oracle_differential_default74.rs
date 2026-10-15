//! The ours-only disagreement the differential run left open on #219: on
//! case default-74 (the `default` fixture, whole-image seed 74) this crate's
//! checker reports `extent_data`, an extent whose stored data checksum
//! 0x11ba7 is not the 0xa2f1ba7 its bytes give, and the reference checker's
//! `fsck -n` passes the image. The change is to a btree node at byte
//! 3014656 (32768 bytes), re-stamped so the node itself still verifies.
//!
//! What decides whether the finding is a fact or a misreading is the
//! checksum of the data itself. It is computed here a second time, bit by
//! bit from the Castagnoli polynomial, sharing no code with `src/csum.rs` or
//! the crate behind it. If that agrees with the checker's computed value and
//! not with the stored one, the image really holds an extent whose data does
//! not match its checksum, and the finding stands.
//!
//! What this does not settle is why the reference passes it: whether its
//! `fsck -n` reads extent data at all is not assumed here, and is left to the
//! differential replay, which runs the reference binary.
#![cfg(feature = "write")]
#![allow(dead_code, unused_imports)]

mod common;

use std::sync::Arc;

use fs_bcachefs::bkey::key_type;
use fs_bcachefs::btree::{self, btree_id};

include!("../fuzz/shared/helpers.rs");
include!("../fuzz/shared/whole_image.rs");

const SEED: u64 = 74;
const STORED: u64 = 0x11ba7;
const COMPUTED: u64 = 0xa2f1ba7;

/// CRC-32C, the reflected Castagnoli polynomial, one bit at a time from
/// `init`, with no final inversion.
fn castagnoli(init: u32, data: &[u8]) -> u32 {
    let mut c = init;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0x82f6_3b78
            } else {
                c >> 1
            };
        }
    }
    c
}

#[test]
fn the_default_74_extent_data_mismatch_is_in_the_image() {
    let path = common::fixture("default.img");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "reading {}: {e} (built by `chore fixtures`)",
            path.display()
        )
    });
    let base = Arc::new(bytes);
    let t = targets(&base);

    let control = Overlay::new(Arc::clone(&base), false);
    let r = fs_bcachefs::check::check(&*control).expect("the control is checked");
    assert!(
        r.problems.iter().all(|p| p.kind != "extent_data"),
        "the unchanged fixture already reports extent_data: {:?}",
        r.problems
    );

    let dev = Overlay::new(Arc::clone(&base), false);
    let what = apply_case(&dev, &t, SEED);
    eprintln!("default-{SEED}: {what}");
    let r = fs_bcachefs::check::check(&*dev).expect("the case is checked");
    let want = format!("stored {STORED:#x}, computed {COMPUTED:#x}");
    assert!(
        r.problems
            .iter()
            .any(|p| p.kind == "extent_data" && p.detail.contains(&want)),
        "default-{SEED} no longer reports extent_data with {want}: {:?}",
        r.problems
    );

    let sb = Superblock::read(&*dev).expect("the superblock is read");
    let keys = btree::walk(&*dev, &sb, btree_id::EXTENTS).expect("the extents btree is walked");
    let mut judged = 0;
    for k in keys.iter().filter(|k| k.key_type == key_type::EXTENT) {
        let Ok(e) = DataExtent::from_key(k) else {
            continue;
        };
        let Some(c) = e.crc.filter(|c| c.csum_lo == STORED) else {
            continue;
        };
        let mut raw = vec![0u8; c.compressed_size as usize * 512];
        fs_core::BlockRead::read_at(&*dev, e.ptr.offset * 512, &mut raw)
            .expect("the extent's data is read");
        let independent = match c.csum_type {
            1 => u64::from(!castagnoli(!0, &raw)),
            5 => u64::from(castagnoli(0, &raw)),
            other => panic!("extent {}: checksum type {other} is not a crc32c", k.pos),
        };
        assert_eq!(
            independent,
            COMPUTED,
            "extent {}: the independent crc32c of its {} bytes is {independent:#x}",
            k.pos,
            raw.len()
        );
        assert_ne!(independent, STORED);
        judged += 1;
    }
    assert_eq!(
        judged, 1,
        "default-{SEED}: expected one extent storing {STORED:#x}, judged {judged}"
    );
}
