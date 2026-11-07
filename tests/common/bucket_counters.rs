//! Alloc keys whose sector counters disagree with what their buckets hold
//! (#380), and an extent gone from a bucket whose key still counts it: the
//! cases the host and the guest both judge.

use super::common::{fixture, plant::write_sparse, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::extent::{parse_entries, ExtentEntry};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

const ALLOC_DATA_TYPE: usize = 14;
const ALLOC_DIRTY: usize = 16;
const ALLOC_CACHED: usize = 20;
const ALLOC_STRIPE: usize = 56;

/// `kind` is what this crate names the damage, `expected` what the reference
/// prints.
pub struct Planted {
    pub image: ScratchImage,
    pub kind: &'static str,
    pub expected: String,
}

/// The data types of the classes the reference corrected: `(number, name)`.
pub const DATA_TYPES: &[(u8, &str)] = &[(1, "sb"), (2, "journal"), (3, "btree"), (4, "user")];

/// The counters, with what each is changed to given its value.
pub const COUNTERS: &[&str] = &["dirty_sectors", "cached_sectors", "stripe_sectors"];

fn copy(set: &str, what: &str) -> (ScratchImage, Vec<Bkey>) {
    let source = fixture(&format!("{set}.img"));
    let image = ScratchDir::new("bucket-counters").into_image(format!("{what}.img"));
    write_sparse(&std::fs::read(&source).unwrap(), &image);
    let device = FileDevice::open(&source).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let alloc = btree::walk(&device, &superblock, btree_id::ALLOC).unwrap();
    (image, alloc)
}

fn insert(image: &ScratchImage, id: u8, key: Bkey) {
    let mut writer = Writer::open(FileDevice::open_rw(image).unwrap()).unwrap();
    writer.insert(id, vec![key]).unwrap();
}

/// A bucket of `data_type` with `counter` changed: the first such bucket
/// holding its counter's expected value.
pub fn plant_counter(set: &str, (data_type, name): (u8, &str), counter: &str) -> Planted {
    let (image, alloc) = copy(set, &format!("{name}-{counter}"));
    let mut key = alloc
        .into_iter()
        .find(|k| k.key_type == key_type::ALLOC_V4 && k.value[ALLOC_DATA_TYPE] == data_type)
        .unwrap_or_else(|| panic!("{set} has no alloc key of data type {name}"));
    let gen = key.value[12];
    let bucket = key.pos;
    let (kind, got, should) = match counter {
        "dirty_sectors" => {
            let should =
                u32::from_le_bytes(key.value[ALLOC_DIRTY..ALLOC_DIRTY + 4].try_into().unwrap());
            let got = should + 3;
            key.value[ALLOC_DIRTY..ALLOC_DIRTY + 4].copy_from_slice(&got.to_le_bytes());
            ("alloc_key_dirty_sectors_wrong", got, should)
        }
        "cached_sectors" => {
            key.value[ALLOC_CACHED..ALLOC_CACHED + 4].copy_from_slice(&8192u32.to_le_bytes());
            ("alloc_key_cached_sectors_wrong", 8192, 0)
        }
        "stripe_sectors" => {
            key.value[ALLOC_STRIPE..ALLOC_STRIPE + 2].copy_from_slice(&8192u16.to_le_bytes());
            ("alloc_stripe_sectors_wrong", 8192, 0)
        }
        other => panic!("no counter {other}"),
    };
    insert(&image, btree_id::ALLOC, key);
    Planted {
        image,
        kind,
        expected: format!(
            "bucket {}:{} gen {gen} data type {name} has wrong {counter}: got {got}, should be \
             {should}",
            bucket.inode, bucket.offset
        ),
    }
}

/// A reservation in place of an extent: the extent's bucket no longer holds
/// it, and the bucket's alloc key still counts it. `kernel-written.img` seed
/// 1659864's class.
pub fn plant_extent_type(set: &str) -> Planted {
    let (image, alloc) = copy(set, "extent-type");
    let source = fixture(&format!("{set}.img"));
    let device = FileDevice::open(&source).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let bucket_size = u64::from(superblock.members().unwrap()[0].bucket_size);
    let stored_in = |key: &Bkey| {
        let (mut stored, mut sector) = (key.size, None);
        for entry in parse_entries(&key.value).unwrap() {
            match entry {
                ExtentEntry::Crc(c) if c.compression_type != 0 && c.compression_type != 5 => {
                    stored = c.compressed_size;
                }
                ExtentEntry::Ptr(p) => sector = Some(p.offset),
                _ => {}
            }
        }
        (stored, sector.unwrap() / bucket_size)
    };
    let dirty_of = |bucket: u64| {
        let key = alloc
            .iter()
            .find(|k| k.key_type == key_type::ALLOC_V4 && k.pos.offset == bucket)
            .unwrap();
        let dirty = &key.value[ALLOC_DIRTY..ALLOC_DIRTY + 4];
        (u32::from_le_bytes(dirty.try_into().unwrap()), key.value[12])
    };
    // In a bucket that holds more than this extent: of an emptied one the
    // reference names the data type it would become, not user.
    let mut key = btree::walk(&device, &superblock, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == key_type::EXTENT)
        .find(|k| {
            let (stored, bucket) = stored_in(k);
            dirty_of(bucket).0 > stored
        })
        .unwrap_or_else(|| panic!("{set} has no bucket holding two extents"));
    let (stored, bucket) = stored_in(&key);
    let (got, gen) = dirty_of(bucket);
    key.key_type = key_type::RESERVATION;
    key.value = vec![0, 0, 0, 0, 1, 0, 0, 0];
    insert(&image, btree_id::EXTENTS, key);
    Planted {
        image,
        kind: "alloc_key_dirty_sectors_wrong",
        expected: format!(
            "bucket 0:{bucket} gen {gen} data type user has wrong dirty_sectors: got {got}, \
             should be {}",
            got - stored
        ),
    }
}
