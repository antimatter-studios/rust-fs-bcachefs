//! The superblock mutation behind #247, located by field rather than by
//! one fixture's byte offset. Only the chosen copy and its checksum change.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use fs_bcachefs::superblock::{Superblock, FIELD_MEMBERS_V2, SB_HEADER_BYTES};
use fs_core::FileDevice;

pub fn change_bucket_size(img: &Path, sector: u64, size: u16, newer: bool) {
    let sb = Superblock::read_at(&FileDevice::open(img).unwrap(), sector).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(img)
        .unwrap();
    let mut bytes = vec![0; SB_HEADER_BYTES + sb.u64s as usize * 8];
    file.seek(SeekFrom::Start(sector * 512)).unwrap();
    file.read_exact(&mut bytes).unwrap();
    let before: usize = sb
        .fields
        .iter()
        .take_while(|f| f.field_type != FIELD_MEMBERS_V2)
        .map(|f| 8 + f.body.len())
        .sum();
    let members = sb.field(FIELD_MEMBERS_V2).expect("members_v2 exists");
    let member_bytes = u16::from_le_bytes(members.body[..2].try_into().unwrap()) as usize;
    // Field header, members_v2 header, this device's member, bucket_size.
    let at = SB_HEADER_BYTES + before + 8 + 8 + usize::from(sb.dev_idx) * member_bytes + 26;
    assert_eq!(
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap()),
        sb.members().unwrap()[usize::from(sb.dev_idx)].bucket_size
    );
    bytes[at..at + 2].copy_from_slice(&size.to_le_bytes());
    if newer {
        bytes[0x70..0x78].copy_from_slice(&(sb.seq + 1).to_le_bytes());
    }
    let checksum = fs_bcachefs::csum::compute(sb.csum_type(), &bytes[16..]).unwrap();
    bytes[..16].fill(0);
    bytes[..8].copy_from_slice(&checksum.to_le_bytes());
    file.seek(SeekFrom::Start(sector * 512)).unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
}
