//! Every extent pointer of every fixture against the allocator: the
//! pointer's bucket has an `alloc_v4` key, the key's generation is the
//! pointer's, the device is this one and no flag is set. The aged image has
//! reused buckets (the lister prints `gen 1` on nine pointers), so the
//! generation comparison is exercised at both values.

mod common;

use std::collections::BTreeMap;

use common::{fixture, SETS};
use fs_bcachefs::bkey::key_type;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::extent::{self, DataExtent};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

#[test]
fn every_pointer_matches_its_buckets_generation() {
    let mut gen1 = 0usize;
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let bucket_size = u64::from(sb.members().unwrap()[sb.dev_idx as usize].bucket_size);
        let gens: BTreeMap<u64, u8> = btree::walk(&dev, &sb, btree_id::ALLOC)
            .unwrap()
            .into_iter()
            .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
            .filter_map(|k| extent::alloc_v4_gen(&k.value).map(|g| (k.pos.offset, g)))
            .collect();
        assert!(!gens.is_empty(), "{set}: no alloc_v4 keys");
        let mut pointers = 0usize;
        for k in btree::walk(&dev, &sb, btree_id::EXTENTS).unwrap() {
            if k.key_type != key_type::EXTENT {
                continue;
            }
            let e = DataExtent::from_key(&k).unwrap();
            let gen = gens.get(&(e.ptr.offset / bucket_size)).copied();
            e.ptr
                .check(sb.dev_idx, gen)
                .unwrap_or_else(|err| panic!("{set}: extent {}: {err}", k.pos));
            pointers += 1;
            if e.ptr.gen > 0 {
                gen1 += 1;
            }
        }
        assert!(pointers > 0, "{set}: no allocated extents");
    }
    assert!(
        gen1 > 0,
        "no fixture has a pointer of generation above 0; the comparison is untested"
    );
}
