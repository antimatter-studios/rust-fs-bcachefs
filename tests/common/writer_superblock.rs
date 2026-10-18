//! Writer regressions over copies of reference-made images. The header
//! offsets and checksums are the black-box observations recorded in S4.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use fs_bcachefs::csum;
use fs_bcachefs::superblock::{Superblock, SB_HEADER_BYTES, SB_OFFSET};
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;

pub const NAME: &str = "selected-copy.txt";
pub const DATA: &[u8] = b"written from the selected superblock\n";

/// Give one backup a newer sequence and a distinct label. Optionally erase
/// the primary, forcing selection through the standalone layout instead.
pub fn prepare(img: &Path, backup: usize, erase_primary: bool) -> Superblock {
    let dev = FileDevice::open(img).unwrap();
    let primary = Superblock::read_at(&dev, SB_OFFSET / 512).unwrap();
    let sector = primary.layout.sb_offsets[backup];
    assert_ne!(sector, SB_OFFSET / 512);
    let copy = Superblock::read_at(&dev, sector).unwrap();
    let mut bytes = vec![0; SB_HEADER_BYTES + copy.u64s as usize * 8];
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(img)
        .unwrap();
    file.seek(SeekFrom::Start(sector * 512)).unwrap();
    file.read_exact(&mut bytes).unwrap();
    bytes[0x70..0x78].copy_from_slice(&(primary.seq + 1).to_le_bytes());
    bytes[0x48..0x68].fill(0);
    bytes[0x48..0x5c].copy_from_slice(b"writer-selected-copy");
    let checksum = csum::compute(copy.csum_type(), &bytes[16..]).unwrap();
    bytes[..16].fill(0);
    bytes[..8].copy_from_slice(&checksum.to_le_bytes());
    file.seek(SeekFrom::Start(sector * 512)).unwrap();
    file.write_all(&bytes).unwrap();
    if erase_primary {
        file.seek(SeekFrom::Start(SB_OFFSET)).unwrap();
        file.write_all(&vec![0; SB_HEADER_BYTES + primary.u64s as usize * 8])
            .unwrap();
    }
    file.sync_all().unwrap();
    let selected = Superblock::read(&dev).unwrap();
    assert_eq!(selected.offset, sector);
    assert_eq!(selected.seq, primary.seq + 1);
    assert_eq!(selected.label_str(), "writer-selected-copy");
    selected
}

/// Perform a supported write before checking the metadata, so a writer
/// that silently publishes the stale primary fails after its operation.
pub fn create_and_check(img: &Path, selected: &Superblock) {
    let dir = Filesystem::open(FileDevice::open(img).unwrap())
        .unwrap()
        .lookup("/d")
        .unwrap();
    let mut writer = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    assert_eq!(writer.superblock().offset, selected.offset);
    assert_eq!(writer.superblock().label, selected.label);
    writer
        .create_file(dir, NAME.as_bytes(), DATA, 0o644)
        .expect("creating a file from the selected superblock copy");
    drop(writer);

    let dev = FileDevice::open(img).unwrap();
    for &sector in &selected.layout.sb_offsets {
        let copy = Superblock::read_at(&dev, sector).unwrap();
        assert_eq!(copy.offset, sector, "copy at sector {sector}");
        assert_eq!(
            copy.seq, selected.seq,
            "copy at sector {sector} reverted to the stale primary's sequence"
        );
        assert_eq!(
            copy.label, selected.label,
            "copy at sector {sector} lost the selected backup's label"
        );
    }
    let fs = Filesystem::open(dev).unwrap();
    assert_eq!(
        fs.read(fs.lookup(&format!("/d/{NAME}")).unwrap()).unwrap(),
        DATA
    );
    assert_eq!(
        fs.read(fs.lookup("/d/existing").unwrap()).unwrap(),
        b"an existing file\n"
    );
}
