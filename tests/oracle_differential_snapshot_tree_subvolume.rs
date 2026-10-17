//! Issue #265: kernel.img seed 1659511, from the digest-pinned fixture
//! artifact, overwrites subvolume 2 with the root subvolume's value. The
//! master subvolume still exists, but its snapshot now belongs to tree 1,
//! not tree 2. The reference checker's black-box verdict is retained by
//! differential.yml alongside the same 80-case replay that found it.
#![cfg(feature = "pinned-fixtures")]
#![allow(dead_code, unused_imports)]

mod common;

use std::sync::Arc;

include!("../fuzz/shared/helpers.rs");
include!("../fuzz/shared/whole_image.rs");

const SEED: u64 = 1_659_511;
const PIN: &str = "sha256:9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e";

fn subvolume_snapshot(dev: &Overlay, id: u64) -> u32 {
    let sb = Superblock::read(dev).unwrap();
    let key = fs_bcachefs::btree::walk(dev, &sb, 8)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 21 && k.pos.inode == 0 && k.pos.offset == id)
        .expect("the subvolume exists");
    le32(&key.value, 4)
}

#[test]
fn kernel_1659511_reports_a_master_subvolume_in_another_snapshot_tree() {
    let recorded = std::fs::read_to_string(common::fixtures_dir().join("../fixtures-pin"))
        .expect("differential.yml must fetch and verify the pinned fixtures first");
    assert_eq!(
        recorded.trim(),
        PIN,
        "the fixtures are not the pinned artifact"
    );

    // Acceptance is part of this regression, including every control the
    // original 648-case differential run judged.
    for name in WI_FIXTURES {
        let bytes = Arc::new(std::fs::read(common::fixture(name)).unwrap());
        let control = Overlay::new(bytes, false);
        let report = fs_bcachefs::check::check(&*control).expect("the control is checked");
        assert!(report.clean(), "{name}: {:?}", report.problems);
    }

    let base = Arc::new(std::fs::read(common::fixture("kernel.img")).unwrap());
    let t = targets(&base);
    let dev = Overlay::new(base, false);
    assert_eq!(subvolume_snapshot(&dev, 2), u32::MAX - 3);
    assert_eq!(
        apply_case(&dev, &t, SEED),
        "btree 8 key at 0:2:0: offset changed, insert committed",
        "the pinned seed must reproduce the exact mutation from #265"
    );
    assert_eq!(subvolume_snapshot(&dev, 2), u32::MAX);

    let report = fs_bcachefs::check::check(&*dev).expect("the mutation is checked");
    assert!(
        report.problems.iter().any(|p| {
            p.kind == "snapshot_tree_to_wrong_subvol"
                && p.detail.contains("snapshot tree 2")
                && p.detail.contains("subvolume 2")
                && p.detail.contains("4294967295")
        }),
        "kernel-{SEED}: the master subvolume exists but points outside tree 2: {:?}",
        report.problems
    );
}
