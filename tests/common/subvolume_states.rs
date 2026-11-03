use crate::common::{copy_sparse, fixture, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::Bpos;
use fs_bcachefs::btree;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

pub const SETS: [&str; 4] = ["default", "aged", "kernel", "kernel-rootsnap"];
pub const CHILDREN: [(&str, u64); 3] = [("kernel", 2), ("kernel", 3), ("kernel-rootsnap", 2)];
pub const LIVE: u32 = 0x4ad5_447e;
pub const UNLINKED: u32 = 0x2d35_8e8f;
pub const DELETED: u32 = 0x3c6b_2d4c;

pub fn plant(set: &str, id: u64, state: u32, unreferenced: bool) -> (ScratchImage, Bpos) {
    let source = fixture(&format!("{set}.img"));
    let image = ScratchDir::new("subvolume-state").into_image("changed.img");
    copy_sparse(&source, &image);
    let device = FileDevice::open(&source).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let mut key = btree::walk(&device, &superblock, 8)
        .unwrap()
        .into_iter()
        .find(|key| key.key_type == 21 && key.pos.offset == id)
        .unwrap_or_else(|| panic!("{set} has no reference-made subvolume {id}"));
    key.value[40..44].copy_from_slice(&state.to_le_bytes());
    if unreferenced {
        key.pos.offset = 4096;
        key.value[16..24].fill(0);
    }
    let position = key.pos;
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(8, vec![key]).unwrap();
    drop(writer);
    (image, position)
}
