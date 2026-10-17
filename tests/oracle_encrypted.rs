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

mod common;

use common::{fixture, read_text, ScratchDir};
use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{self, NodePtr};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::Error;
use fs_core::FileDevice;

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
