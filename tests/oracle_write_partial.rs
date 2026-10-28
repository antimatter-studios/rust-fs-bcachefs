//! A write touches only the extents it overlaps (#366). The writer used to
//! read a file whole, free every extent it had and write all of it back,
//! so a 10-byte write into a large file cost the whole file in memory, in
//! I/O and in fresh buckets. Each extent the write does not overlap now
//! keeps its key, and with it its pointer and its bucket.
#![cfg(feature = "write")]

mod common;

use common::fixture;
use fs_bcachefs::bkey::{key_type, Bkey};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

const MIB: u64 = 1 << 20;

fn scratch(test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("write-partial").into_image(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();
    img
}

/// File `ino`'s keys in the extents btree, whiteouts left out.
fn extent_keys(img: &std::path::Path, ino: u64) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.pos.inode == ino && k.size > 0)
        .filter(|k| !matches!(k.key_type, key_type::WHITEOUT | key_type::EXTENT_WHITEOUT))
        .collect()
}

fn clean(img: &std::path::Path) {
    let r = fs_bcachefs::check::check(&FileDevice::open(img).unwrap()).unwrap();
    assert!(r.clean(), "{:?}", r.problems);
}

/// The issue's red: a 10-byte write in the middle of a 1 MiB file on
/// `default`. Every extent that does not overlap the written block is
/// unchanged, key, pointer and checksum, and the file reads back as
/// written.
#[test]
fn a_write_into_a_large_file_leaves_the_extents_it_does_not_overlap_as_they_were() {
    let img = scratch("mid");
    let block = {
        let dev = FileDevice::open(&img).unwrap();
        (u64::from(Superblock::read(&dev).unwrap().block_size) * 512).max(512)
    };
    let mut model: Vec<u8> = (0..MIB as u32).map(|i| (i % 251) as u8 ^ 0x5a).collect();
    let root = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w.create_file(root, b"large", &model, 0o644).unwrap();
    drop(w);
    let before = extent_keys(&img, ino);
    assert!(
        before.len() > 2,
        "a 1 MiB file is {} extents; the test needs several",
        before.len()
    );

    let at = MIB / 2 + 3;
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.write_at(ino, at, b"0123456789").unwrap();
    drop(w);
    model[at as usize..at as usize + 10].copy_from_slice(b"0123456789");

    let (from, to) = (at - at % block, (at + 10).next_multiple_of(block));
    let after = extent_keys(&img, ino);
    let untouched: Vec<&Bkey> = before
        .iter()
        .filter(|k| k.pos.offset * 512 <= from || k.start_offset() * 512 >= to)
        .collect();
    let rewritten: Vec<&&Bkey> = untouched.iter().filter(|k| !after.contains(k)).collect();
    assert!(
        rewritten.is_empty(),
        "a 10-byte write at byte {at} rewrote {} of the {} extents outside the written \
         block ({from}..{to}), the first at {}",
        rewritten.len(),
        untouched.len(),
        rewritten
            .first()
            .map(|k| k.pos.to_string())
            .unwrap_or_default()
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        fs.read(ino).unwrap() == model,
        "the file does not read back as written"
    );
    drop(fs);
    clean(&img);
}

/// Each bucket's dirty sectors, from its alloc_v4 key (docs/clean-room.md,
/// "Allocating space").
fn dirty_sectors(img: &std::path::Path, bucket: u64) -> u64 {
    const ALLOC: u8 = 4;
    const ALLOC_V4: u8 = 27;
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let k = btree::walk(&dev, &sb, ALLOC)
        .unwrap()
        .into_iter()
        .find(|k| k.key_type == ALLOC_V4 && k.pos.inode == 0 && k.pos.offset == bucket)
        .unwrap_or_else(|| panic!("bucket {bucket} has no alloc key"));
    u64::from(u32::from_le_bytes(k.value[16..20].try_into().unwrap()))
}

/// #447: a write into the middle of an extent cuts it, as the reference
/// leaves an extent it overwrote part of (docs/clean-room.md, "Pieces of
/// checksummed regions"). A 10-byte write in the middle of one extent (64
/// sectors, a bucket) of a 1 MiB file on `default` leaves the extent's part
/// before the written block and its part after it where they were: the
/// same pointer and checksum entry, the crc entry's offset and the key's
/// size naming the live part. Only the written block is stored anew, so
/// the old bucket's dirty sectors fall by that block alone. The file reads
/// back and the checker passes the image.
#[test]
fn a_write_into_the_middle_of_an_extent_cuts_it_instead_of_rewriting_it() {
    use fs_bcachefs::extent::DataExtent;
    let img = scratch("cut");
    let (block, bucket) = {
        let dev = FileDevice::open(&img).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let bucket = u64::from(sb.members().unwrap()[0].bucket_size);
        ((u64::from(sb.block_size) * 512).max(512), bucket)
    };
    let mut model: Vec<u8> = (0..MIB as u32).map(|i| (i % 253) as u8 ^ 0xa5).collect();
    let root = Filesystem::open(FileDevice::open(&img).unwrap())
        .unwrap()
        .lookup("/")
        .unwrap();
    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    let ino = w.create_file(root, b"large", &model, 0o644).unwrap();
    drop(w);

    // The extent holding the middle of the file, and a byte in its middle.
    let before = extent_keys(&img, ino);
    let old = before
        .iter()
        .find(|k| k.start_offset() * 512 <= MIB / 2 && k.pos.offset * 512 > MIB / 2)
        .expect("an extent holds the file's middle")
        .clone();
    assert!(
        u64::from(old.size) * 512 >= 4 * block,
        "the middle extent is {} sectors; the test needs one of several blocks",
        old.size
    );
    let old_e = DataExtent::from_key(&old).unwrap();
    let crc = old_e.crc.expect("default's extents carry a checksum entry");
    let at = old.start_offset() * 512 + u64::from(old.size) * 256 + 3;
    let (from, to) = (at - at % block, (at + 10).next_multiple_of(block));
    let old_bucket = old_e.ptr.offset / bucket;
    let dirty_before = dirty_sectors(&img, old_bucket);

    let mut w = Writer::open(FileDevice::open_rw(&img).unwrap()).unwrap();
    w.write_at(ino, at, b"0123456789").unwrap();
    drop(w);
    model[at as usize..at as usize + 10].copy_from_slice(b"0123456789");

    let after = extent_keys(&img, ino);
    let piece = |start: u64, end: u64| {
        after
            .iter()
            .find(|k| k.start_offset() == start && k.pos.offset == end)
            .unwrap_or_else(|| {
                panic!(
                    "a 10-byte write at byte {at} into the extent {}..{} left no key \
                     {start}..{end}: the extent was rewritten whole, not cut; the keys there \
                     are {:?}",
                    old.start_offset(),
                    old.pos.offset,
                    after
                        .iter()
                        .filter(|k| k.start_offset() < old.pos.offset
                            && k.pos.offset > old.start_offset())
                        .map(|k| (k.start_offset(), k.pos.offset))
                        .collect::<Vec<_>>()
                )
            })
    };
    let front = DataExtent::from_key(piece(old.start_offset(), from / 512)).unwrap();
    let back = DataExtent::from_key(piece(to / 512, old.pos.offset)).unwrap();
    for (name, e, offset) in [
        ("front", &front, 0),
        ("back", &back, to / 512 - old.start_offset()),
    ] {
        assert_eq!(e.ptr, old_e.ptr, "the {name} piece moved");
        let c = e.crc.expect("a piece keeps its checksum entry");
        assert_eq!(
            (c.csum_lo, c.csum_hi, c.uncompressed_size, c.compressed_size),
            (
                crc.csum_lo,
                crc.csum_hi,
                crc.uncompressed_size,
                crc.compressed_size
            ),
            "the {name} piece's checksum entry is not the region's"
        );
        assert_eq!(u64::from(c.offset), offset, "the {name} piece's crc offset");
    }
    assert_eq!(
        dirty_sectors(&img, old_bucket),
        dirty_before - (to - from) / 512,
        "the old extent's bucket should lose only the written block's sectors"
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        fs.read(ino).unwrap() == model,
        "the file does not read back as written"
    );
    drop(fs);
    clean(&img);
}
