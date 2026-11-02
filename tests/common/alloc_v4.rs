use super::common::{fixture, plant::write_sparse, ScratchDir, ScratchImage};
use fs_bcachefs::bkey::{key_type, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

pub type Case = (u32, u32, usize, Option<&'static str>);

pub const CASES: &[Case] = &[
    (8, 0, 64, None),
    (0, 0, 48, None),
    (6, 0, 48, None),
    (7, 0, 48, Some("bad val size (7 > 6)")),
    (7, 0, 56, None),
    (8, 0, 56, Some("bad val size (8 > 7)")),
    (9, 0, 64, Some("bad val size (9 > 8)")),
    (9, 0, 72, None),
    (8, 1, 88, Some("bad val size (12 > 11)")),
    (8, 1, 96, None),
    (8, 1, 104, None),
    (0, 1, 48, Some("bad val size (10 > 6)")),
    (0, 2, 64, Some("bad val size (14 > 8)")),
    (6, 1, 72, Some("bad val size (10 > 9)")),
    (6, 1, 80, None),
    (3, 1, 48, Some("bad val size (7 > 6)")),
    (3, 1, 64, None),
    (4, 1, 64, None),
    (5, 1, 64, Some("bad val size (9 > 8)")),
    (63, 0, 64, Some("bad val size (63 > 8)")),
    (8, 63, 64, Some("bad val size (260 > 8)")),
    (63, 63, 64, Some("bad val size (315 > 8)")),
    (8, 0, 72, None),
    (8, 0, 40, Some("bad val size (40 < 48)")),
];

pub fn plant(set: &str, (start, backpointers, bytes, _): Case) -> (ScratchImage, Bpos) {
    let source = fixture(&format!("{set}.img"));
    let image = ScratchDir::new("alloc-v4-size").into_image("planted.img");
    write_sparse(&std::fs::read(&source).unwrap(), &image);
    let device = FileDevice::open(&source).unwrap();
    let superblock = Superblock::read(&device).unwrap();
    let mut key = btree::walk(&device, &superblock, btree_id::ALLOC)
        .unwrap()
        .into_iter()
        .find(|key| key.key_type == key_type::ALLOC_V4 && key.value.get(14) == Some(&4))
        .expect("the reference-made fixture has a user alloc_v4 key");
    key.value.resize(bytes, 0);
    let flags = u32::from_le_bytes(key.value[8..12].try_into().unwrap());
    let flags = (flags & !0x3ffc) | (start << 2) | (backpointers << 8);
    key.value[8..12].copy_from_slice(&flags.to_le_bytes());
    let position = key.pos;
    let mut writer = Writer::open(FileDevice::open_rw(&image).unwrap()).unwrap();
    writer.insert(btree_id::ALLOC, vec![key]).unwrap();
    drop(writer);
    (image, position)
}
