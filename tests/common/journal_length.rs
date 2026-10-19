//! #262: mutate an old journal header in a reference-made image.
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use fs_bcachefs::{journal, superblock::Superblock};
use fs_core::FileDevice;

pub fn plant(img: &Path, oversized: bool, unknown_version: bool) {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let entries = journal::read_entries(&dev, &sb).unwrap();
    let first = entries
        .first()
        .expect("reference fixture has journal entries");
    let newest = entries.iter().rev().find(|j| j.flush).unwrap();
    assert!(
        first.seq < newest.last_seq,
        "exercise a stale journal entry"
    );
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(img)
        .unwrap();
    let mut b = vec![0; first.bytes];
    f.seek(SeekFrom::Start(first.sector * 512)).unwrap();
    f.read_exact(&mut b).unwrap();
    if unknown_version {
        b[34..36].fill(255);
    }
    if oversized {
        b[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        // Deliberately leave the original checksum: the reference diagnoses
        // the impossible length before checksum validation.
    } else {
        let c = fs_bcachefs::csum::compute(
            (u32::from_le_bytes(b[36..40].try_into().unwrap()) & 15) as u8,
            &b[16..],
        )
        .unwrap();
        b[..16].fill(0);
        b[..8].copy_from_slice(&c.to_le_bytes());
    }
    f.seek(SeekFrom::Start(first.sector * 512)).unwrap();
    f.write_all(&b).unwrap();
    f.sync_all().unwrap();
}
