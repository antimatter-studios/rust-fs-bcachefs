//! Metadata buckets whose alloc key names another data type while its counts
//! agree, or that have no alloc key at all (#576): the cases the host and
//! the guest both judge.

use super::common::{fixture, plant::write_sparse, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

const ALLOC_DATA_TYPE: usize = 14;

/// `expected` is what both checkers print for the damage.
pub struct Planted {
    pub image: ScratchImage,
    pub expected: String,
}

/// `(number, name, the number and name it is changed to)`.
const CASES: &[(u8, &str, u8, &str)] = &[
    (1, "sb", 3, "btree"),
    (2, "journal", 3, "btree"),
    (3, "btree", 4, "user"),
];

fn plant(set: &str, data_type: u8, name: &str, to: Option<(u8, &str)>) -> Planted {
    let source = fixture(&format!("{set}.img"));
    let what = to.map_or("missing", |(_, other)| other);
    let image = ScratchDir::new("bucket-types").into_image(format!("{name}-{what}.img"));
    write_sparse(&std::fs::read(&source).unwrap(), &image);
    let device = FileDevice::open(&source).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let mut key: Bkey = btree::walk(&device, &superblock, btree_id::ALLOC)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == key_type::ALLOC_V4 && k.value[ALLOC_DATA_TYPE] == data_type)
        .unwrap_or_else(|| panic!("{set} has no alloc key of data type {name}"));
    let (device_index, bucket, gen) = (key.pos.inode, key.pos.offset, key.value[12]);
    let (gen, got) = match to {
        Some((number, other)) => {
            key.value[ALLOC_DATA_TYPE] = number;
            (gen, other)
        }
        None => {
            key.key_type = key_type::DELETED;
            key.value.clear();
            (0, "free")
        }
    };
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(btree_id::ALLOC, vec![key]).unwrap();
    Planted {
        image,
        expected: format!(
            "bucket {device_index}:{bucket} gen {gen} has wrong data_type: got {got}, should be \
             {name}"
        ),
    }
}

/// Every metadata data type, changed and removed.
pub fn plant_all(set: &str) -> Vec<Planted> {
    CASES
        .iter()
        .flat_map(|&(number, name, other, other_name)| {
            [
                plant(set, number, name, Some((other, other_name))),
                plant(set, number, name, None),
            ]
        })
        .collect()
}
