//! A backpointer's generation, against what the reference checker said of a
//! real image, in the unit tier (#562). The image in `tests/data` is
//! `kernel-written.img` from the fixtures of CI run 41424418346: the module
//! mounted it, and this crate's writer, before #479, wrote two files whose
//! pointers are at generation 1 and whose backpointers are at generation 0.
//! The reference checker exits 0 on it. How the file was packed, and the
//! matrix the rule comes from, are in `docs/clean-room.md`; the guest holds
//! the reference itself to the same rule (`tests/check_oracle.rs`).

use super::*;

/// A raw deflate stream of: the number of blocks, then each block's number,
/// little-endian u32s, then the blocks. Every other block of the image is zero.
const PACKED: &[u8] = include_bytes!("../../tests/data/kernel-written-41424418346.bin");
const IMAGE_BYTES: u64 = 64 << 20;
const BLOCK: usize = 4096;
const WORD: usize = 4;
const UNPACKED_LIMIT: usize = 1 << 20;

/// The image the fixture packs; a block it does not hold reads as zero.
struct Packed(BTreeMap<u64, Vec<u8>>);

impl Packed {
    fn open() -> Packed {
        let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(PACKED, UNPACKED_LIMIT)
            .expect("the fixture is a deflate stream");
        let count = le32(&raw, 0) as usize;
        let blocks = &raw[WORD * (1 + count)..];
        assert_eq!(blocks.len(), count * BLOCK);
        Packed(
            (0..count)
                .map(|i| {
                    let number = u64::from(le32(&raw, WORD * (1 + i)));
                    (number, blocks[i * BLOCK..(i + 1) * BLOCK].to_vec())
                })
                .collect(),
        )
    }
}

impl BlockRead for Packed {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        for (i, byte) in buf.iter_mut().enumerate() {
            let at = offset + i as u64;
            let block = self.0.get(&(at / BLOCK as u64));
            *byte = block.map_or(0, |b| b[(at % BLOCK as u64) as usize]);
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        IMAGE_BYTES
    }
}

/// The image's data extents, each with its backpointer.
fn pairs(dev: &Packed) -> Vec<(Bkey, DataExtent, Bkey)> {
    let sb = Superblock::read(dev).unwrap();
    let backpointers = btree::walk(dev, &sb, BACKPOINTERS).unwrap();
    btree::walk(dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == key_type::EXTENT)
        .map(|k| {
            let e = DataExtent::from_key(&k).unwrap();
            let bp = backpointers
                .iter()
                .find(|b| b.pos.offset == e.ptr.offset << 16)
                .expect("the extent has a backpointer")
                .clone();
            (k, e, bp)
        })
        .collect()
}

#[test]
fn a_bucket_with_no_backpointer_at_its_generation_is_clean_as_the_reference_says() {
    let dev = Packed::open();
    let disagreeing = pairs(&dev)
        .iter()
        .filter(|(_, e, bp)| e.ptr.gen == 1 && bp.value[3] == 0)
        .count();
    assert_eq!(disagreeing, 16, "the shape this fixture exists to hold");
    let report = check(&dev).unwrap();
    assert!(report.clean(), "{:?}", report.problems);
}

/// The extents the checker reports when each backpointer of `pairs` has the
/// generation `gen` gives it.
fn reported_with(
    pairs: &[(Bkey, DataExtent, Bkey)],
    bucket_size: u64,
    gen: impl Fn(usize, &DataExtent, u8) -> u8,
) -> Vec<String> {
    let changed: Vec<Bkey> = pairs
        .iter()
        .enumerate()
        .map(|(i, (_, e, bp))| {
            let mut bp = bp.clone();
            bp.value[3] = gen(i, e, bp.value[3]);
            bp
        })
        .collect();
    let bps: BTreeMap<u64, &Bkey> = changed.iter().map(|k| (k.pos.offset, k)).collect();
    let mut r = Report::default();
    for (k, e, _) in pairs {
        data_backpointer(k, e, Some(bucket_size), &bps, &mut r);
    }
    assert!(r
        .problems
        .iter()
        .all(|p| p.kind == "backpointer_to_missing_ptr"));
    r.problems.into_iter().map(|p| p.detail).collect()
}

/// The verdicts of `docs/clean-room.md`'s matrix, on this image's own keys: a
/// generation stays damage where its bucket has a backpointer at the pointer's.
#[test]
fn a_generation_the_reference_reports_is_still_reported() {
    let dev = Packed::open();
    let sb = Superblock::read(&dev).unwrap();
    let bucket_size = u64::from(sb.members().unwrap()[sb.dev_idx as usize].bucket_size);
    let pairs = pairs(&dev);
    let first = pairs.iter().position(|(_, e, _)| e.ptr.gen == 1).unwrap();
    let bucket = pairs[first].1.ptr.offset / bucket_size;
    let in_bucket = pairs
        .iter()
        .filter(|(_, e, _)| e.ptr.offset / bucket_size == bucket)
        .count();
    assert!(in_bucket >= 2, "the bucket holds several extents");
    let other = 0xff;

    // Each at the pointer's generation; each at another; the fixture as it is.
    assert!(reported_with(&pairs, bucket_size, |_, e, _| e.ptr.gen).is_empty());
    assert!(reported_with(&pairs, bucket_size, |_, _, _| other).is_empty());
    assert!(reported_with(&pairs, bucket_size, |_, _, stored| stored).is_empty());

    // One other beside the rest at the pointer's: that one is reported.
    let one_other = reported_with(&pairs, bucket_size, |i, e, _| {
        if i == first {
            other
        } else {
            e.ptr.gen
        }
    });
    assert_eq!(one_other.len(), 1, "{one_other:?}");
    assert!(one_other[0].contains(&pairs[first].0.pos.to_string()));

    // One at the pointer's beside the rest at another: the rest are reported,
    // as the reference printed the neighbour of the key this changed.
    let one_current = reported_with(&pairs, bucket_size, |i, e, stored| {
        if i == first {
            e.ptr.gen
        } else {
            stored
        }
    });
    assert_eq!(one_current.len(), in_bucket - 1, "{one_current:?}");
    assert!(!one_current
        .iter()
        .any(|detail| detail.contains(&pairs[first].0.pos.to_string())));
}
