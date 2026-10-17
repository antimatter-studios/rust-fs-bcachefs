//! An encrypted filesystem's btrees, read with its master key (#271).
//!
//! `encrypted.img` was formatted with `--no_passphrase`, so its master key
//! is stored unsealed in the superblock's crypt field. Every btree node is
//! encrypted with ChaCha20 under that key and authenticated with a 128-bit
//! Poly1305 MAC stored where a checksum would be (S1 9.1.4); which bytes,
//! and with what nonce, is S4 (docs/clean-room.md, "Encryption: btree
//! nodes"). The reference lister decrypted and listed every btree of it;
//! this crate must read the same keys, and refuse a node whose MAC does not
//! verify.
//!
//! Its data extents are encrypted too (#272), each authenticated with an
//! 80-bit Poly1305 MAC in its crc entry (S1 9.1.4.3). Every file must read
//! back as the tree the reference formatter was given, which the reference
//! implementation also reads from the image, and an extent whose MAC does
//! not verify must be refused.

mod common;

use common::{fixture, fixtures_dir, manifest, read_text, ScratchDir};
use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{self, NodePtr};
use fs_bcachefs::extent::DataExtent;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::{Error, Filesystem};
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

/// Btree names in S1's order (11.3), each id its index, as the lister
/// takes them; the fixture build lists every one.
const BTREES: &[&str] = &[
    "extents",
    "inodes",
    "dirents",
    "xattrs",
    "alloc",
    "quotas",
    "stripes",
    "reflink",
    "subvolumes",
    "snapshots",
    "lru",
    "freespace",
    "need_discard",
    "backpointers",
    "bucket_gens",
    "snapshot_trees",
    "deleted_inodes",
    "logged_ops",
    "reconcile_work",
    "subvolume_children",
    "accounting",
];

/// Key type names in S1's order (11.5), each type number its index, as the
/// lister prints them.
const KEY_TYPES: &[&str] = &[
    "deleted",
    "whiteout",
    "error",
    "cookie",
    "hash_whiteout",
    "btree_ptr",
    "extent",
    "reservation",
    "inode",
    "inode_generation",
    "dirent",
    "xattr",
    "alloc",
    "quota",
    "stripe",
    "reflink_p",
    "reflink_v",
    "inline_data",
    "btree_ptr_v2",
    "indirect_inline_data",
    "alloc_v2",
    "subvolume",
    "snapshot",
    "inode_v2",
    "alloc_v3",
    "set",
    "lru",
    "alloc_v4",
    "backpointer",
    "inode_v3",
    "bucket_gens",
    "snapshot_tree",
    "logged_op_truncate",
    "logged_op_finsert",
    "accounting",
    "inode_alloc_cursor",
    "extent_whiteout",
    "logged_op_stripe_update",
    "damage",
    "logged_op_inode_opt_propagate",
];

fn type_name(t: u8) -> String {
    KEY_TYPES
        .get(t as usize)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("type{t}"))
}

/// `type pos len size` of every key line the lister printed.
fn lister_keys(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with("u64s "))
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            // u64s N type T POS len L ver V ...
            format!("{} {} len {}", w[3], w[4], w[6])
        })
        .collect()
}

#[test]
fn every_key_of_every_btree_matches_the_reference_lister() {
    let dev = FileDevice::open(fixture("encrypted.img")).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let mut listed = 0;
    for (id, name) in BTREES.iter().enumerate() {
        let text = read_text(&format!("encrypted.{name}.txt"));
        assert!(
            !text.contains("list: exit"),
            "the reference lister did not read {name}:\n{text}"
        );
        let theirs = lister_keys(&text);
        let ours: Vec<String> = btree::walk(&dev, &sb, id as u8)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .iter()
            .map(|k| format!("{} {} len {}", type_name(k.key_type), k.pos, k.size))
            .collect();
        if let Some(i) = (0..ours.len().max(theirs.len())).find(|&i| ours.get(i) != theirs.get(i)) {
            panic!(
                "{name}: key {i} of {} (the lister's {}): ours {:?}, the lister's {:?}",
                ours.len(),
                theirs.len(),
                ours.get(i),
                theirs.get(i)
            );
        }
        listed += theirs.len();
    }
    assert!(listed > 0, "the lister printed no keys for any btree");
    // Every btree the superblock has a root for was compared.
    for root in sb.btree_roots().unwrap() {
        assert!(
            (root.btree_id as usize) < BTREES.len(),
            "a root for btree {}, which no listing covers",
            root.btree_id
        );
    }
}

/// The inodes btree's root node: where it is on the device, and how many
/// bytes of it are written.
fn inodes_root(sb: &Superblock) -> (u64, usize) {
    let root = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == btree::btree_id::INODES)
        .expect("an inodes root");
    let key = bkey::decode(
        &root.key,
        &BkeyFormat {
            key_u64s: 5,
            nr_fields: 6,
            bits: [0; 6],
            field_offset: [0; 6],
        },
    )
    .unwrap();
    let ptr = NodePtr::from_key(&key).unwrap();
    (ptr.ptrs[0].offset * 512, ptr.sectors_written as usize * 512)
}

/// One byte of the inodes root changed, at an offset into the node: the
/// walk is refused as a bad checksum, naming the node's MAC.
fn refused_with_one_byte_changed(at: usize) -> String {
    let dir = ScratchDir::new("encrypted-mac");
    let img = dir.copy_fixture("encrypted.img");
    let (node, len) = inodes_root(&Superblock::read(&FileDevice::open(&img).unwrap()).unwrap());
    assert!(at < len, "byte {at} is past the node's {len} written bytes");
    {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&img)
            .unwrap();
        let mut b = [0u8; 1];
        f.seek(SeekFrom::Start(node + at as u64)).unwrap();
        f.read_exact(&mut b).unwrap();
        b[0] ^= 0x01;
        f.seek(SeekFrom::Start(node + at as u64)).unwrap();
        f.write_all(&b).unwrap();
    }
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    match btree::walk(&dev, &sb, btree::btree_id::INODES) {
        Err(Error::BadChecksum { what, .. }) => what.to_string(),
        Err(e) => panic!("byte {at}: refused, but not as a bad checksum: {e}"),
        Ok(keys) => panic!("byte {at}: read {} keys from a changed node", keys.len()),
    }
}

/// A node whose MAC does not verify is refused by name: a byte of its
/// encrypted keys, of its encrypted header (the key format), and of its
/// bset header, which is stored in the clear but covered by the MAC.
#[test]
fn a_node_whose_mac_does_not_verify_is_refused_as_a_bad_checksum() {
    for (what, at) in [("a key", 200), ("the header", 90), ("the bset header", 144)] {
        let refused = refused_with_one_byte_changed(at);
        assert!(
            refused.contains("MAC"),
            "{what} (byte {at}) changed: refused as a bad {refused}, not naming the MAC"
        );
    }
}

/// S1 9.1.4.3's default data MAC, Poly1305 truncated to 80 bits, by the
/// name the reference lister prints for a checksum type. Every crc entry of
/// `encrypted.img` carries checksum type 3 (S4), and the lister names it.
const DATA_MAC: &str = "chacha20_poly1305_80";
const CSUM_DATA_MAC: u8 = 3;

fn open(img: &std::path::Path) -> Filesystem<FileDevice> {
    Filesystem::open(FileDevice::open(img).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", img.display()))
}

/// Every data extent of `img`: its key, and its first pointer with the crc
/// entry before it.
fn data_extents(img: &std::path::Path) -> Vec<(bkey::Bkey, DataExtent)> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, btree::btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == bkey::key_type::EXTENT)
        .map(|k| {
            let e = DataExtent::from_key(&k).unwrap_or_else(|e| panic!("{}: {e}", k.pos));
            (k, e)
        })
        .collect()
}

/// The data is encrypted, as the reference lister sees it: every extent
/// carries the 80-bit MAC in its crc entry, and the lister names that
/// checksum on every crc line it prints.
#[test]
fn every_data_extent_carries_an_80_bit_mac() {
    let ours = data_extents(&fixture("encrypted.img"));
    assert!(!ours.is_empty(), "encrypted.img has no data extents");
    for (k, e) in &ours {
        let csum = e.crc.map(|c| c.csum_type);
        assert_eq!(csum, Some(CSUM_DATA_MAC), "extent {}: checksum type", k.pos);
    }
    let text = read_text("encrypted.extents.txt");
    let crc_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("crc32:") || l.starts_with("crc64:") || l.starts_with("crc128:"))
        .collect();
    assert_eq!(
        crc_lines.len(),
        ours.len(),
        "crc entries: the lister printed {}, this crate read {}",
        crc_lines.len(),
        ours.len()
    );
    for l in crc_lines {
        assert!(
            l.contains(&format!("csum {DATA_MAC} ")),
            "the lister's crc entry does not name {DATA_MAC}: {l}"
        );
    }
}

/// Every path of the tree the reference formatter was given reads back
/// from the encrypted image: each file byte for byte (its SHA-256), each
/// symlink's target, each directory as a directory.
#[test]
fn every_file_reads_back_as_the_source_tree() {
    let fs = open(&fixture("encrypted.img"));
    let mut files = 0;
    for e in manifest("encrypted") {
        let ino = fs
            .lookup(&e.path)
            .unwrap_or_else(|err| panic!("{}: {err}", e.path));
        let i = fs.inode(ino).unwrap();
        assert_eq!(i.mode & 0o7777, e.mode, "{}: permissions", e.path);
        match e.kind.as_str() {
            "dir" => assert!(i.is_dir(), "{}: not a directory", e.path),
            "symlink" => {
                let target = fs
                    .read(ino)
                    .unwrap_or_else(|err| panic!("{}: {err}", e.path));
                assert_eq!(
                    target,
                    e.target.as_deref().unwrap().as_bytes(),
                    "{}: symlink target",
                    e.path
                );
            }
            "file" => {
                let data = fs
                    .read(ino)
                    .unwrap_or_else(|err| panic!("{}: {err}", e.path));
                assert_eq!(Some(data.len() as u64), e.size, "{}: size", e.path);
                assert_eq!(
                    Some(format!("{:x}", Sha256::digest(&data))),
                    e.sha256,
                    "{}: contents",
                    e.path
                );
                files += 1;
            }
            k => panic!("{}: kind {k}", e.path),
        }
    }
    assert!(files > 0, "the manifest names no files");
}

/// The reference implementation, mounted over a copy of the image in the
/// test VM, reads the same tree from it, and the reference checker passes
/// it: the source tree is what the image holds, not only what it was made
/// from.
#[test]
fn the_reference_implementation_reads_the_source_tree_from_the_image() {
    let fsck = read_text("encrypted.fsck.txt");
    assert!(
        !fsck.contains("fsck: exit"),
        "the reference checker did not pass encrypted.img:\n{fsck}"
    );
    let record = read_text("encrypted.mounted.txt");
    assert!(
        fixtures_dir().join("encrypted.mounted.json").exists(),
        "the reference implementation did not read encrypted.img:\n{record}"
    );
    let view = |set: &str| -> Vec<String> {
        manifest(set)
            .into_iter()
            .map(|e| {
                format!(
                    "{} {} {:o} {:?} {:?} {:?}",
                    e.path, e.kind, e.mode, e.size, e.sha256, e.target
                )
            })
            .collect()
    };
    let (want, theirs) = (view("encrypted"), view("encrypted.mounted"));
    if let Some(i) = (0..want.len().max(theirs.len())).find(|&i| want.get(i) != theirs.get(i)) {
        panic!(
            "entry {i}: the source tree's {:?}, the reference mount's {:?}",
            want.get(i),
            theirs.get(i)
        );
    }
}

/// One byte of a file's stored data changed, at `at` bytes into its first
/// extent: reading that file is refused as a bad checksum naming the MAC,
/// and every other file still reads.
fn read_with_one_data_byte_changed(at: u64) -> String {
    let dir = ScratchDir::new("encrypted-data-mac");
    let img = dir.copy_fixture("encrypted.img");
    let (k, e) = data_extents(&img)
        .into_iter()
        .find(|(_, e)| {
            e.crc
                .is_some_and(|c| u64::from(c.compressed_size) * 512 > at)
        })
        .expect("an extent that long");
    {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&img)
            .unwrap();
        let mut b = [0u8; 1];
        f.seek(SeekFrom::Start(e.ptr.offset * 512 + at)).unwrap();
        f.read_exact(&mut b).unwrap();
        b[0] ^= 0x01;
        f.seek(SeekFrom::Start(e.ptr.offset * 512 + at)).unwrap();
        f.write_all(&b).unwrap();
    }
    let fs = open(&img);
    let mut refused = None;
    for m in manifest("encrypted").iter().filter(|m| m.kind == "file") {
        let ino = fs.lookup(&m.path).unwrap();
        match fs.read(ino) {
            Ok(_) => assert_ne!(ino, k.pos.inode, "{}: read with a byte changed", m.path),
            Err(Error::BadChecksum { what, .. }) if ino == k.pos.inode => {
                refused = Some(what.to_string())
            }
            Err(err) => panic!("{}: {err}", m.path),
        }
    }
    refused.unwrap_or_else(|| panic!("no file holds the extent at {}", k.pos))
}

/// A data extent whose MAC does not verify is refused by name: a byte of
/// its first sector changed, and of its last.
#[test]
fn a_data_extent_whose_mac_does_not_verify_is_refused_as_a_bad_checksum() {
    for at in [0, 64 * 512 - 1] {
        let refused = read_with_one_data_byte_changed(at);
        assert!(
            refused.contains("MAC"),
            "byte {at} of an extent changed: refused as a bad {refused}, not naming the MAC"
        );
    }
}
