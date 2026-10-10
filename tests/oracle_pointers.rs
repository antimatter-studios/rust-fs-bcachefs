//! Every extent pointer of every fixture against the allocator: the
//! pointer's bucket has an `alloc_v4` key, the key's generation is the
//! pointer's, the device is this one and no flag is set. The aged image only
//! sometimes reuses a bucket (#94), so a reused bucket is made on purpose by
//! the reference kernel module (`kernel-reuse`), and the generation
//! comparison is exercised at both values on every run.

mod common;

use std::collections::BTreeMap;

use common::{fixture, SETS};
use fs_bcachefs::bkey::key_type;
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::extent::{self, DataExtent};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

/// Every allocated extent pointer of image `name` checked against its
/// bucket's `alloc_v4` generation: `(pointers, pointers of generation 1+)`.
fn check_image(name: &str) -> (usize, usize) {
    let dev = FileDevice::open(fixture(&format!("{name}.img"))).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let bucket_size = u64::from(sb.members().unwrap()[sb.dev_idx as usize].bucket_size);
    let gens: BTreeMap<u64, u8> = btree::walk(&dev, &sb, btree_id::ALLOC)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
        .filter_map(|k| extent::alloc_v4_gen(&k.value).map(|g| (k.pos.offset, g)))
        .collect();
    assert!(!gens.is_empty(), "{name}: no alloc_v4 keys");
    let (mut pointers, mut reused) = (0usize, 0usize);
    for k in btree::walk(&dev, &sb, btree_id::EXTENTS).unwrap() {
        if k.key_type != key_type::EXTENT {
            continue;
        }
        let e = DataExtent::from_key(&k).unwrap();
        let gen = gens.get(&(e.ptr.offset / bucket_size)).copied();
        e.ptr
            .check(sb.dev_idx, gen)
            .unwrap_or_else(|err| panic!("{name}: extent {}: {err}", k.pos));
        pointers += 1;
        reused += usize::from(e.ptr.gen > 0);
    }
    assert!(pointers > 0, "{name}: no allocated extents");
    (pointers, reused)
}

#[test]
fn every_pointer_matches_its_buckets_generation() {
    for set in SETS {
        check_image(set);
    }
}

/// A bucket freed and written again (#94): the reference kernel module
/// writes a file, removes it and writes the next, on an image small enough
/// that the later files can only land in buckets freed by the earlier ones
/// (scripts/kernel-oracle.sh). Its pointers carry generation 1 or more, and
/// each matches its bucket's, so the comparison is exercised at both values
/// on every run, not only when the aged image happens to reuse a bucket.
#[test]
fn a_reused_bucket_s_pointers_carry_its_new_generation() {
    let (pointers, reused) = check_image("kernel-reuse");
    assert!(
        reused > 0,
        "kernel-reuse: {pointers} pointers, none of generation 1 or more: no bucket was reused"
    );
}
