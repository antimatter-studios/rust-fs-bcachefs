//! Extended attributes as this crate reads them, compared with what the
//! reference implementation's mount listed for every path of the aged
//! images (names and values, every namespace the guest could set).

mod common;

use common::{fixture, manifest};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn every_extended_attribute_matches_the_mount() {
    for set in ["aged", "aged-unclean"] {
        let fs = Filesystem::open(FileDevice::open(fixture(&format!("{set}.img"))).unwrap())
            .unwrap_or_else(|e| panic!("{set}: {e}"));
        let mut with = 0;
        for e in manifest(set) {
            let ino = fs.lookup(&e.path).unwrap();
            let mut ours: Vec<String> = fs
                .xattrs(ino)
                .unwrap_or_else(|err| panic!("{set} {}: {err}", e.path))
                .iter()
                .map(|x| format!("{}={}", String::from_utf8_lossy(&x.name), hex(&x.value)))
                .collect();
            ours.sort();
            let ours = (!ours.is_empty()).then(|| ours.join(";"));
            assert_eq!(ours, e.xattrs, "{set} {}: xattrs", e.path);
            with += usize::from(ours.is_some());
        }
        assert!(with >= 5, "{set}: only {with} paths carry xattrs");
    }
}
