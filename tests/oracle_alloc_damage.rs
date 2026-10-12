//! One damaged alloc node does not make every file unreadable (#167).
//! Every data read checks its pointer's generation against the bucket's
//! alloc key, so a single torn alloc node failed every file on the
//! filesystem, though the data and its checksums were intact. To the
//! reference, allocation information is derived data: it reports the node
//! and reads on (the differential run's `default-13`). Here a byte of
//! `default`'s alloc root is flipped without recomputing its checksum, and
//! every file must still read as the reference recorded it.

mod common;

use common::{fixture, manifest};
use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{btree_id, NodePtr};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::Filesystem;
use fs_core::{BlockRead, FileDevice};
use sha2::{Digest, Sha256};
use std::io::{Seek, SeekFrom, Write};

const UNPACKED: BkeyFormat = BkeyFormat {
    key_u64s: 5,
    nr_fields: 6,
    bits: [0; 6],
    field_offset: [0; 6],
};

#[test]
fn every_file_reads_with_its_alloc_root_torn() {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-bcachefs-alloc-damage-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("alloc-torn.img");
    std::fs::copy(fixture("default.img"), &img).unwrap();

    // Flip one byte inside the alloc root's first bset, checksum left as it
    // was: a torn write.
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let root = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == btree_id::ALLOC)
        .expect("default records an alloc root");
    let ptr = NodePtr::from_key(&bkey::decode(&root.key, &UNPACKED).unwrap()).unwrap();
    let at = ptr.ptrs[0].offset * 512 + 200;
    let mut byte = [0u8; 1];
    dev.read_at(at, &mut byte).unwrap();
    drop(dev);
    let mut f = std::fs::OpenOptions::new().write(true).open(&img).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(&[byte[0] ^ 0x5a]).unwrap();
    drop(f);

    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let mut read = 0;
    for e in manifest("default") {
        let Some(want) = e.sha256 else { continue };
        let ino = fs.lookup(&e.path).unwrap();
        let data = fs.read(ino).unwrap_or_else(|err| {
            panic!("{} is unreadable with the alloc root torn: {err}", e.path)
        });
        assert_eq!(
            format!("{:x}", Sha256::digest(&data)),
            want,
            "{} read wrong with the alloc root torn",
            e.path
        );
        read += 1;
    }
    assert!(read > 0, "default's manifest lists no files");
}
