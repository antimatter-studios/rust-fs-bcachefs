//! Planting keys where they do not belong (#163): a root for a btree that
//! has none, and the names of the key types, for tests that put a key of
//! one type into a btree that may not hold it.

use std::path::Path;

#[cfg(feature = "write")]
use fs_bcachefs::btree::{flags_id_and_level, flags_with_id_and_level};
use fs_bcachefs::btree::{walk, NodePtr};
use fs_bcachefs::superblock::Superblock;
#[cfg(feature = "write")]
use fs_bcachefs::superblock::{FIELD_CLEAN, SB_HEADER_BYTES};
#[cfg(feature = "write")]
use fs_core::BlockRead;
use fs_core::FileDevice;
use std::io::{Seek, SeekFrom, Write};

mod helpers {
    #![allow(dead_code, unused_imports)]
    include!("../../fuzz/shared/helpers.rs");
}
pub use helpers::UNPACKED;
#[cfg(feature = "write")]
pub use helpers::{restamp_node, restamp_superblock};

/// The btrees by id, in the Principles of Operation's list order (S1
/// 11.3); the reference tools print these names.
pub const BTREES: &[&str] = &[
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
    "reconcile_hipri",
    "reconcile_pending",
    "reconcile_scan",
    "reconcile_work_phys",
    "reconcile_hipri_phys",
    "bucket_to_stripe",
    "stripe_backpointers",
    "damage",
];

/// The key types by number, in the Principles of Operation's list order
/// (S1 11.5); the reference tools print these names.
pub const KEY_TYPES: &[&str] = &[
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

pub const DELETED_INODES: u8 = 16;
pub const LOGGED_OPS: u8 = 17;
pub const ACCOUNTING: u8 = 20;
pub const RECONCILE_WORK_PHYS: u8 = 24;

/// A key type's name, or its number when S1 names none.
pub fn type_name(t: u8) -> String {
    KEY_TYPES
        .get(usize::from(t))
        .map_or_else(|| format!("type {t}"), |n| n.to_string())
}

/// Write `image` to `dst`, leaving its zero blocks as holes, so a copy of a
/// mostly empty 64 MiB fixture costs what it holds.
pub fn write_sparse(image: &[u8], dst: &Path) {
    const BLOCK: usize = 4096;
    let mut f = std::fs::File::create(dst).unwrap();
    f.set_len(image.len() as u64).unwrap();
    for (i, b) in image.chunks(BLOCK).enumerate() {
        if b.iter().any(|&x| x != 0) {
            f.seek(SeekFrom::Start((i * BLOCK) as u64)).unwrap();
            f.write_all(b).unwrap();
        }
    }
    f.sync_all().unwrap();
}

/// A btree of `img` that records a root yet holds no keys, other than
/// `except`: the donor `move_root` takes a root from. Which btrees that is
/// depends on what the reference formatter wrote, so it is found, not
/// assumed. `default` records a root for no btree that holds nothing (CI:
/// "no empty btree with a root to give btree 24") and none for xattrs, so
/// when none is found the xattrs btree is given one: the writer makes its
/// root for the first xattr, and removing that xattr, its only key, leaves
/// the root holding nothing.
#[cfg(feature = "write")]
pub fn empty_root(img: &Path, except: u8) -> u8 {
    use fs_bcachefs::btree::btree_id::XATTRS;
    use fs_bcachefs::inode::ROOT_INO;
    use fs_bcachefs::write::Writer;

    if let Some(id) = find_empty_root(img, except) {
        compact(img, id);
        return id;
    }
    assert_ne!(
        except, XATTRS,
        "no empty btree with a root to give the xattrs btree"
    );
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    w.set_xattr(ROOT_INO, b"user.donor", b"x").unwrap();
    w.remove_xattr(ROOT_INO, b"user.donor").unwrap();
    drop(w);
    let id = find_empty_root(img, except)
        .unwrap_or_else(|| panic!("no empty btree with a root to give btree {except}"));
    compact(img, id);
    id
}

/// Rewrite btree `id`'s root through the writer so it physically holds
/// only its live keys: an empty btree's root then holds none at all.
#[cfg(feature = "write")]
fn compact(img: &Path, id: u8) {
    use fs_bcachefs::write::Writer;
    let mut w = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    w.compact_root(id).unwrap();
    drop(w);
    assert_eq!(
        physical_keys(img, id),
        0,
        "btree {id}'s compacted root holds keys"
    );
}

fn find_empty_root(img: &Path, except: u8) -> Option<u8> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let roots = sb.btree_roots().unwrap();
    roots
        .iter()
        .map(|r| r.btree_id)
        .find(|&id| id != except && walk(&dev, &sb, id).unwrap().is_empty())
}

/// The keys btree `id`'s root node holds as written, before any merge:
/// a removed key and its whiteout both count, as the reference validates
/// each one on its own.
pub fn physical_keys(img: &Path, id: u8) -> usize {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let roots = sb.btree_roots().unwrap();
    let root = roots
        .iter()
        .find(|r| r.btree_id == id)
        .unwrap_or_else(|| panic!("btree {id} has no root"));
    let key = fs_bcachefs::bkey::decode(&root.key, &UNPACKED).unwrap();
    let ptr = NodePtr::from_key(&key).unwrap();
    fs_bcachefs::btree::read_node(&dev, &sb, &ptr)
        .unwrap()
        .keys
        .len()
}

/// Give btree `to`, which records no root, the root of btree `from`: its
/// clean-field entry in every superblock copy, and the btree id in its
/// node's header, are renumbered, and each is re-stamped. Its node usage
/// and allocation backpointer name the receiving btree too. `from` is left
/// with no root, which the reference reads as an empty btree (the xattrs
/// btree of every formatter-made fixture has none).
#[cfg(feature = "write")]
pub fn move_root(img: &Path, from: u8, to: u8) {
    use fs_bcachefs::bkey::Bpos;
    use fs_bcachefs::write::Writer;

    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let roots = sb.btree_roots().unwrap();
    assert!(
        roots.iter().all(|r| r.btree_id != to),
        "btree {to} already has a root"
    );
    let root = roots
        .iter()
        .find(|r| r.btree_id == from)
        .unwrap_or_else(|| panic!("btree {from} has no root to give"));
    let key = fs_bcachefs::bkey::decode(&root.key, &UNPACKED).unwrap();
    let ptr = NodePtr::from_key(&key).unwrap();
    assert_eq!(ptr.ptrs.len(), 1, "the donor must have one device pointer");
    assert_eq!(root.level, 0, "the donor must be a leaf root");
    assert!(walk(&dev, &sb, from).unwrap().is_empty());

    // S8's kind-6 node usage is indexed by btree id (write_alloc.rs).
    // The reference moved-root control reports this node's sectors and
    // count against the old id unless both records are transferred.
    let usage_pos = |id| Bpos {
        inode: 6 << 56 | u64::from(id) << 48,
        offset: 0,
        snapshot: 0,
    };
    let usage = walk(&dev, &sb, ACCOUNTING).unwrap();
    let mut old_usage = usage
        .iter()
        .find(|k| k.pos == usage_pos(from) && k.key_type == 34)
        .expect("the donor's node usage is recorded")
        .clone();
    let expected_usage: Vec<u8> = [u64::from(sb.btree_node_size()), 1, 0]
        .into_iter()
        .flat_map(u64::to_le_bytes)
        .collect();
    assert_eq!(old_usage.value, expected_usage);
    assert!(usage.iter().filter(|k| k.pos == usage_pos(to)).all(|k| {
        k.key_type == 34 && k.value.len() == 24 && k.value.iter().all(|&byte| byte == 0)
    }));
    let mut new_usage = old_usage.clone();
    new_usage.pos = usage_pos(to);
    old_usage.value.fill(0);

    // The root's allocation backpointer stays at its physical address;
    // only its owning btree changes (write_nodes.rs).
    let mut backpointer = walk(&dev, &sb, 13)
        .unwrap()
        .into_iter()
        .find(|k| {
            k.key_type == 28
                && k.pos.inode == u64::from(ptr.ptrs[0].dev)
                && k.pos.offset == ptr.ptrs[0].offset << 16
                && k.pos.snapshot == 0
        })
        .expect("the donor's allocation backpointer is recorded");
    assert_eq!(backpointer.value.len(), 32);
    assert_eq!(&backpointer.value[..3], &[from, 1, 3]);
    backpointer.value[0] = to;
    let node_at = ptr.ptrs[0].offset * 512;
    let mut node = vec![0u8; usize::from(ptr.sectors_written) * 512];
    dev.read_at(node_at, &mut node).unwrap();
    let mut copies = Vec::new();
    for &sector in &sb.layout.sb_offsets {
        let mut b = vec![0u8; SB_HEADER_BYTES + sb.u64s as usize * 8];
        dev.read_at(sector * 512, &mut b).unwrap();
        copies.push((sector, b));
    }
    drop(dev);

    // The node: flags at byte 24 carry its btree id and level.
    let flags = u64::from_le_bytes(node[24..32].try_into().unwrap());
    let (id, level) = flags_id_and_level(flags);
    assert_eq!(id, from, "the root node's header names btree {id}");
    node[24..32].copy_from_slice(&flags_with_id_and_level(flags, to, level).to_le_bytes());
    restamp_node(&mut node, usize::from(sb.block_size) * 512);

    // Each superblock copy: in the clean field, entries of u16 u64s, u8
    // btree id, u8 level, u8 type (1 a btree root), 3 bytes pad, payload.
    for (_, b) in &mut copies {
        let mut p = SB_HEADER_BYTES;
        let mut moved = false;
        while p + 8 <= b.len() {
            let u64s = u32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as usize;
            let ty = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap());
            let end = p + u64s * 8;
            if u64s == 0 || end > b.len() {
                break;
            }
            if ty == FIELD_CLEAN {
                let mut e = p + 8 + 16;
                while e + 8 <= end {
                    let n = usize::from(u16::from_le_bytes([b[e], b[e + 1]]));
                    if b[e + 4] == 1 && b[e + 2] == from && n > 0 {
                        b[e + 2] = to;
                        moved = true;
                    }
                    e += 8 + n * 8;
                }
            }
            p = end;
        }
        assert!(moved, "no clean-field root entry for btree {from}");
        restamp_superblock(b);
    }

    let mut f = std::fs::OpenOptions::new().write(true).open(img).unwrap();
    f.seek(SeekFrom::Start(node_at)).unwrap();
    f.write_all(&node).unwrap();
    for (sector, b) in &copies {
        f.seek(SeekFrom::Start(sector * 512)).unwrap();
        f.write_all(b).unwrap();
    }
    f.sync_all().unwrap();
    drop(f);

    let mut writer = Writer::open(FileDevice::open_rw(img).unwrap()).unwrap();
    writer.insert(13, vec![backpointer]).unwrap();
    writer
        .insert(ACCOUNTING, vec![old_usage, new_usage])
        .unwrap();
}
