//! The superblock's copies: the reader takes the copy with the highest
//! valid `seq` and falls back to the layout at sector 7 when the primary is
//! gone (S1 9.5.1). Shown on fixture copies this test damages itself: the
//! reference formatter writes three copies with one `seq`, so the cases are
//! made here, byte for byte, and checked against what the untouched image
//! says.

mod common;
#[path = "common/superblock_copy.rs"]
mod superblock_copy;
#[path = "common/superblock_field.rs"]
mod superblock_field;

use common::fixture;
use fs_bcachefs::check;
use fs_bcachefs::csum;
use fs_bcachefs::superblock::{Superblock, SB_HEADER_BYTES, SB_OFFSET};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

fn scratch(test: &str) -> common::ScratchImage {
    let img = common::ScratchDir::new("sb-copies").into_image(format!("{test}.img"));
    std::fs::copy(fixture("default.img"), &img).unwrap();
    img
}

fn patch(img: &std::path::Path, at: u64, bytes: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(img).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(bytes).unwrap();
}

/// The bytes of the superblock copy at `sector`, header and fields.
fn copy_bytes(img: &std::path::Path, sector: u64) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(img).unwrap();
    f.seek(SeekFrom::Start(sector * 512)).unwrap();
    let mut head = vec![0u8; SB_HEADER_BYTES];
    f.read_exact(&mut head).unwrap();
    let u64s = u32::from_le_bytes(head[0x7c..0x80].try_into().unwrap()) as usize;
    let mut b = vec![0u8; SB_HEADER_BYTES + u64s * 8];
    f.seek(SeekFrom::Start(sector * 512)).unwrap();
    f.read_exact(&mut b).unwrap();
    b
}

#[test]
fn the_layout_names_three_copies_with_one_seq() {
    let dev = FileDevice::open(fixture("default.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    assert_eq!(sb.layout.sb_offsets.len(), 3, "{:?}", sb.layout.sb_offsets);
    assert_eq!(sb.layout.sb_offsets[0], SB_OFFSET / 512);
    for &sector in &sb.layout.sb_offsets {
        let copy = Superblock::read_at(&dev, sector).unwrap();
        assert_eq!(copy.seq, sb.seq, "copy at {sector}");
        assert_eq!(
            copy.offset, sector,
            "copy at {sector} records its own offset"
        );
        assert_eq!(copy.fields, sb.fields, "copy at {sector}");
    }
    assert_eq!(Superblock::read_layout(&dev).unwrap(), sb.layout);
}

#[test]
fn a_zeroed_primary_is_read_from_a_copy_and_reported_by_the_checker() {
    let img = scratch("zeroed-primary");
    let before = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let hello = before.lookup("/hello.txt").unwrap();
    let want = before.read(hello).unwrap();
    drop(before);
    patch(&img, SB_OFFSET, &vec![0u8; 8192]);

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    assert_ne!(
        sb.offset,
        SB_OFFSET / 512,
        "the primary is gone; a copy must be read"
    );
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    assert_eq!(fs.read(fs.lookup("/hello.txt").unwrap()).unwrap(), want);

    let report = check::check(&FileDevice::open(&img).unwrap()).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.kind == "superblock_copy" && p.detail.contains("sector 8:")),
        "the checker did not report the dead primary: {:?}",
        report.problems
    );
}

#[test]
fn the_copy_with_the_highest_seq_wins() {
    let img = scratch("newer-copy");
    let dev = FileDevice::open(&img).unwrap();
    let primary = Superblock::read(&dev).unwrap();
    drop(dev);
    let sector = primary.layout.sb_offsets[1];
    let mut b = copy_bytes(&img, sector);
    // seq one higher, a changed label so the winner is unmistakable, and
    // the checksum recomputed as a writer would.
    b[0x70..0x78].copy_from_slice(&(primary.seq + 1).to_le_bytes());
    b[0x48..0x50].copy_from_slice(b"newer\0\0\0");
    let csum = csum::compute(primary.csum_type(), &b[16..]).unwrap();
    b[0..16].fill(0);
    b[0..8].copy_from_slice(&csum.to_le_bytes());
    patch(&img, sector * 512, &b);

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    assert_eq!(sb.seq, primary.seq + 1);
    assert_eq!(sb.offset, sector);
    assert_eq!(sb.label_str(), "newer");
}

#[test]
fn when_no_copy_reads_the_error_is_the_primarys() {
    let img = scratch("all-gone");
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    drop(dev);
    for &sector in &sb.layout.sb_offsets {
        patch(&img, sector * 512, &vec![0u8; 4096]);
    }
    patch(&img, 3584, &vec![0u8; 512]);
    assert!(matches!(
        Superblock::read(&FileDevice::open(&img).unwrap()),
        Err(fs_bcachefs::Error::BadMagic { what: "superblock" })
    ));
}

/// Nightly differential run 38878947701, aged seed 1659384: only the
/// primary's bucket_size changed from 256 to 8448 sectors, with a valid
/// checksum and unchanged sequence. The reference checker stayed clean.
#[test]
fn a_primary_bucket_size_change_at_the_same_sequence_stays_clean() {
    let img = common::ScratchDir::new("sb-copies").into_image("bucket-size-tie.img");
    std::fs::copy(fixture("aged.img"), &img).unwrap();
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    assert_eq!(sb.members().unwrap()[0].bucket_size, 256);
    assert!(check::check(&dev).unwrap().clean(), "unchanged control");

    superblock_copy::change_bucket_size(&img, SB_OFFSET / 512, 8448, false);
    let dev = FileDevice::open(&img).unwrap();
    let primary = Superblock::read_at(&dev, SB_OFFSET / 512).unwrap();
    assert_eq!(primary.seq, sb.seq, "the mutation keeps its sequence");
    assert_eq!(primary.members().unwrap()[0].bucket_size, 8448);
    let report = check::check(&dev).unwrap();
    assert!(
        report.clean(),
        "the reference accepts the primary-only mutation: {:?}",
        report.problems
    );
}

/// Black-box reference 1.39.7: only the sequence-winning copy's optional
/// fields are validated; a bad checksum still diagnoses a damaged copy.
#[test]
fn optional_fields_are_validated_only_in_the_selected_copy() {
    for (name, indexes, newer, restamp, clean) in [
        ("primary", vec![0], false, true, true),
        ("middle", vec![1], false, true, true),
        ("last", vec![2], false, true, false),
        ("newer", vec![0], true, true, false),
        ("all", vec![0, 1, 2], false, true, false),
        ("checksum", vec![0], false, false, false),
    ] {
        let img = common::ScratchDir::new("sb-fields").into_image(format!("{name}.img"));
        std::fs::copy(fixture("aged-unclean.img"), &img).unwrap();
        let sb = Superblock::read(&FileDevice::open(&img).unwrap()).unwrap();
        assert_eq!(sb.layout.sb_offsets.len(), 3);
        assert!(check::check(&FileDevice::open(&img).unwrap())
            .unwrap()
            .clean());
        for index in indexes {
            superblock_field::zero_field_length(&img, sb.layout.sb_offsets[index], newer, restamp);
        }
        let r = check::check(&FileDevice::open(&img).unwrap()).unwrap();
        assert_eq!(r.clean(), clean, "{name}: {:?}", r.problems);
        if !clean {
            assert_eq!(r.problems.len(), 1, "{name}: {:?}", r.problems);
            assert_eq!(
                r.problems[0].kind,
                if restamp {
                    "superblock"
                } else {
                    "superblock_copy"
                }
            );
        }
    }
}
