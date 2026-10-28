//! The `reconcile` fixture (#420): a filesystem with background
//! compression, written through the reference implementation's mount with
//! reconcile off, so the extents it wrote are still waiting on background
//! work. Each has a `set` key at its own position in reconcile_work and one
//! at its backpointer's position in reconcile_work_phys, which the
//! reconcile_work checker cases (tests/check_oracle.rs) damage. A build
//! whose image holds none would leave them nothing to damage; it fails
//! here, naming the btree, and this crate must read the same keys the
//! reference lister printed.

mod common;

use common::{fixture, read_text};
use fs_bcachefs::btree;
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

/// The `set` key type (S1 11.5).
const KEY_SET: u8 = 25;

/// The position of every key line the lister printed, with its type.
fn lister_keys(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with("u64s "))
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            // u64s N type T POS len L ver V ...
            format!("{} {}", w[3], w[4])
        })
        .collect()
}

#[test]
fn the_reconcile_fixture_holds_extents_waiting_on_background_work() {
    let dev = FileDevice::open(fixture("reconcile.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    for (id, name) in [(18, "reconcile_work"), (24, "reconcile_work_phys")] {
        let ours: Vec<String> = btree::walk(&dev, &sb, id)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .iter()
            .map(|k| {
                assert_eq!(
                    k.key_type, KEY_SET,
                    "{name}: a key at {} is not a set key",
                    k.pos
                );
                format!("set {}", k.pos)
            })
            .collect();
        assert!(
            !ours.is_empty(),
            "reconcile.img holds no {name} key: no extent is waiting on background work, \
             so the reconcile_work checker cases have nothing to damage"
        );
        let theirs = lister_keys(&read_text(&format!("reconcile.{name}.txt")));
        assert_eq!(
            ours, theirs,
            "{name}: ours, then the reference lister's (reconcile.{name}.txt)"
        );
    }
}
