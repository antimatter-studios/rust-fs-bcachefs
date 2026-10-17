//! #261's zero-length optional field, located in reference-made images.
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use fs_bcachefs::superblock::{Superblock, SB_HEADER_BYTES};
use fs_core::FileDevice;

pub fn zero_field_length(img: &Path, sector: u64, newer: bool, restamp: bool) {
    let sb = Superblock::read_at(&FileDevice::open(img).unwrap(), sector).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(img)
        .unwrap();
    let mut b = vec![0; SB_HEADER_BYTES + sb.u64s as usize * 8];
    f.seek(SeekFrom::Start(sector * 512)).unwrap();
    f.read_exact(&mut b).unwrap();
    let before: usize = sb
        .fields
        .iter()
        .take_while(|f| f.field_type != 16)
        .map(|f| 8 + f.body.len())
        .sum();
    assert!(sb.field(16).is_some(), "reference fixture has field 16");
    b[SB_HEADER_BYTES + before..SB_HEADER_BYTES + before + 4].fill(0);
    if newer {
        b[0x70..0x78].copy_from_slice(&(sb.seq + 1).to_le_bytes());
    }
    if restamp {
        let c = fs_bcachefs::csum::compute(sb.csum_type(), &b[16..]).unwrap();
        b[..16].fill(0);
        b[..8].copy_from_slice(&c.to_le_bytes());
    }
    f.seek(SeekFrom::Start(sector * 512)).unwrap();
    f.write_all(&b).unwrap();
    f.sync_all().unwrap();
}
