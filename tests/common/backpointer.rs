//! #256's value-only mutation, located by the extent rather than a
//! regenerated fixture's inode or device sector. No reference source.

use std::path::Path;

use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::extent::{compression, DataExtent};
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

pub const TREE: u8 = 13;

pub fn pair(img: &Path) -> (Bkey, Bkey) {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let extent = btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .find(|k| {
            k.key_type == key_type::EXTENT
                && k.size == 8
                && k.pos.snapshot == u32::MAX
                && DataExtent::from_key(k).is_ok_and(|e| {
                    e.ptr.flags == 0
                        && e.crc.is_some_and(|c| {
                            c.compression_type == compression::NONE && c.offset == 0
                        })
                })
        })
        .expect("the fixture has an eight-sector uncompressed data extent");
    let e = DataExtent::from_key(&extent).unwrap();
    let at = Bpos {
        inode: u64::from(e.ptr.dev),
        offset: e.ptr.offset << 16,
        snapshot: 0,
    };
    let bp = btree::walk(&dev, &sb, TREE)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == 28 && k.pos == at)
        .expect("the extent has a backpointer");
    assert_eq!(bp.value.len(), 32);
    assert_eq!(&bp.value[..3], &[btree_id::EXTENTS, 0, 4]);
    assert_eq!(u32::from_le_bytes(bp.value[8..12].try_into().unwrap()), 8);
    assert_eq!(Bpos::parse(&bp.value[12..]).unwrap(), extent.pos);
    (extent, bp)
}

/// Run 39018152705, block4k seed 1659443 replaces word one by 1:
/// length 8 -> 1 and target snapshot U32_MAX -> 0 together.
pub fn change(bp: &mut Bkey, case: &str) {
    match case {
        "word-one" => bp.value[8..16].copy_from_slice(&1u64.to_le_bytes()),
        "length" => bp.value[8..12].copy_from_slice(&1u32.to_le_bytes()),
        "snapshot" => bp.value[12..16].copy_from_slice(&0u32.to_le_bytes()),
        _ => panic!("unknown backpointer mutation {case}"),
    }
}
