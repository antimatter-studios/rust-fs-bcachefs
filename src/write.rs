//! Writing, behind the `write` feature: new keys go into the btrees by
//! appending a bset to the leaf that covers them, and each node's new length
//! is carried up to the root, whose key lives in the superblock.
//!
//! Provenance (docs/clean-room.md, "The write path"). Everything here
//! repeats what the reference implementation was observed to do in the
//! write study's before/after pairs (S8): creating a file appended one bset
//! to the dirents leaf, with the node's seq, a newer journal sequence, flags
//! carrying the checksum type in the low 4 bits and the bset's start in
//! sectors from bit 16, and raised the `sectors_written` of the root key in
//! the superblock's clean field. The reference checker judges every image
//! this writes (tests in the guest).
//!
//! NOT CRASH-SAFE: nodes are written, then their parents, then the
//! superblock. An interruption leaves bsets nothing points past; the
//! reference checker would find them. Committing through the journal is
//! issue #25.

use crate::bkey::{self, key_type, Bkey, BkeyFormat, Bpos};
use crate::btree::{self, Node, NodePtr};
use crate::error::{Error, Result};
use crate::superblock::{Superblock, FIELD_CLEAN};
use crate::util::{le16, le32, le64};
use fs_core::BlockDevice;

const SB_OFFSET: u64 = 4096;
const SB_HEADER_BYTES: usize = 0x2f0;
const HEADER_BSET: usize = 136;
const BSET_HEADER: usize = 24;

/// The bytes of an unpacked key: u64s, format 1, type, pad, version_hi u32,
/// version_lo u64, size u32, bpos (snapshot u32, offset u64, inode u64),
/// then the value, which must be a whole number of u64s.
pub fn encode_key(k: &Bkey) -> Result<Vec<u8>> {
    if !k.value.len().is_multiple_of(8) {
        return Err(Error::Corrupt("a key's value must be whole u64s".into()));
    }
    let u64s = 5 + k.value.len() / 8;
    let u64s_byte =
        u8::try_from(u64s).map_err(|_| Error::Unsupported(format!("a key of {u64s} u64s")))?;
    let mut b = Vec::with_capacity(u64s * 8);
    b.extend_from_slice(&[u64s_byte, 1, k.key_type, 0]);
    b.extend_from_slice(&k.version_hi.to_le_bytes());
    b.extend_from_slice(&k.version_lo.to_le_bytes());
    b.extend_from_slice(&k.size.to_le_bytes());
    b.extend_from_slice(&k.pos.snapshot.to_le_bytes());
    b.extend_from_slice(&k.pos.offset.to_le_bytes());
    b.extend_from_slice(&k.pos.inode.to_le_bytes());
    b.extend_from_slice(&k.value);
    Ok(b)
}

const UNPACKED: BkeyFormat = BkeyFormat {
    key_u64s: 5,
    nr_fields: 6,
    bits: [0; 6],
    field_offset: [0; 6],
};

/// A btree_ptr_v2 key with its `sectors_written` replaced (value bytes
/// 16..18, see `NodePtr::parse`).
fn with_sectors_written(k: &Bkey, sectors: u16) -> Result<Bkey> {
    if k.key_type != key_type::BTREE_PTR_V2 || k.value.len() < 40 {
        return Err(Error::Corrupt("not a btree_ptr_v2 key".into()));
    }
    let mut k = k.clone();
    k.value[16..18].copy_from_slice(&sectors.to_le_bytes());
    Ok(k)
}

/// A writer over one clean, single-device, unencrypted filesystem.
pub struct Writer<D: BlockDevice> {
    dev: D,
    sb: Superblock,
    /// The primary superblock's bytes, header and fields.
    sb_raw: Vec<u8>,
    /// The journal sequence number new bsets carry: the one the clean
    /// field records as the last.
    journal_seq: u64,
}

impl<D: BlockDevice> Writer<D> {
    pub fn open(dev: D) -> Result<Self> {
        let sb = Superblock::read(&dev)?;
        if !sb.is_clean() {
            return Err(Error::Unsupported(
                "the filesystem was not cleanly unmounted: writing needs a clean one".into(),
            ));
        }
        if sb.field(2).is_some() || sb.nr_devices != 1 {
            return Err(Error::Unsupported(
                "writing needs a single-device, unencrypted filesystem".into(),
            ));
        }
        let len = SB_HEADER_BYTES + sb.u64s as usize * 8;
        let mut sb_raw = vec![0u8; len];
        dev.read_at(SB_OFFSET, &mut sb_raw)?;
        let clean = sb
            .field(FIELD_CLEAN)
            .ok_or_else(|| Error::Corrupt("a clean filesystem with no clean field".into()))?;
        if clean.body.len() < 16 {
            return Err(Error::Corrupt("clean field shorter than its header".into()));
        }
        let journal_seq = le64(&clean.body, 8);
        Ok(Writer {
            dev,
            sb,
            sb_raw,
            journal_seq,
        })
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// Insert `keys` into btree `id`: each replaces the key at its position
    /// (a key of type `deleted` removes it). Keys going to the same leaf are
    /// appended as one bset; the new lengths are carried up to the root.
    pub fn insert(&mut self, id: u8, mut keys: Vec<Bkey>) -> Result<()> {
        keys.sort_by_key(|k| k.pos);
        let (root_level, root_key) = self.root(id)?;
        let new_root = self.insert_at(&root_key, root_level, &keys, true)?;
        self.set_root(id, &new_root)?;
        self.write_superblock()
    }

    /// Fail, writing nothing, when inserting `keys` into btree `id` would
    /// need a node that has no room for another bset. A transaction checks
    /// every btree it touches before writing any of them, so a refusal
    /// never leaves one btree updated and another not.
    fn check_fits(&self, id: u8, keys: &[Bkey]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let mut keys = keys.to_vec();
        keys.sort_by_key(|k| k.pos);
        let (root_level, root_key) = self.root(id)?;
        self.insert_at(&root_key, root_level, &keys, false)
            .map(|_| ())
    }

    /// Insert into the subtree under `ptr_key` (at `level`); returns the
    /// pointer key with its new `sectors_written`. With `write` false
    /// nothing is written: only whether every node has room is checked.
    fn insert_at(&self, ptr_key: &Bkey, level: u8, keys: &[Bkey], write: bool) -> Result<Bkey> {
        let ptr = NodePtr::from_key(ptr_key)?;
        if level == 0 {
            let written = self.append_bset(&ptr, keys, write)?;
            return with_sectors_written(ptr_key, written);
        }
        let node = btree::read_node(&self.dev, &self.sb, &ptr)?;
        // Each key goes to the first child whose pointer position (its max
        // key) is at or after it.
        let mut updated = Vec::new();
        let mut rest = keys;
        for child in node
            .keys
            .iter()
            .filter(|k| k.key_type == key_type::BTREE_PTR_V2)
        {
            let n = rest.iter().take_while(|k| k.pos <= child.pos).count();
            if n > 0 {
                updated.push(self.insert_at(child, level - 1, &rest[..n], write)?);
                rest = &rest[n..];
            }
        }
        if !rest.is_empty() {
            return Err(Error::Corrupt(format!(
                "key {} is past the last child of an interior node",
                rest[0].pos
            )));
        }
        let written = self.append_bset(&ptr, &updated, write)?;
        with_sectors_written(ptr_key, written)
    }

    /// Append one bset holding `keys` to the node, or with `write` false
    /// only check that it fits; returns the node's new `sectors_written`.
    fn append_bset(&self, ptr: &NodePtr, keys: &[Bkey], write: bool) -> Result<u16> {
        let block = (self.sb.block_size as usize * 512).max(512);
        let node_bytes = self.sb.btree_node_size() as usize * 512;
        let written = ptr.sectors_written as usize * 512;
        let mut node = vec![0u8; written];
        let at = ptr.ptrs[0].offset * 512;
        self.dev.read_at(at, &mut node)?;
        // The node must parse as it stands before anything is added to it.
        Node::parse(
            &node,
            btree::node_magic(&self.sb.uuid),
            block,
            Some(ptr.seq),
        )?;
        let first_flags = le32(&node, HEADER_BSET + 16);
        let version = le16(&node, HEADER_BSET + 20);
        let csum_type = (first_flags & 0xf) as u8;

        let start = written.div_ceil(block) * block;
        let mut body = Vec::new();
        for k in keys {
            body.extend(encode_key(k)?);
        }
        let u64s = u16::try_from(body.len() / 8)
            .map_err(|_| Error::Unsupported("a bset of more than 65535 u64s".into()))?;
        let sector = u32::try_from(start / 512)
            .map_err(|_| Error::Corrupt("node offset overflows".into()))?;
        let mut rec = vec![0u8; 16 + BSET_HEADER];
        rec[16..24].copy_from_slice(&ptr.seq.to_le_bytes());
        rec[24..32].copy_from_slice(&self.journal_seq.to_le_bytes());
        rec[32..36].copy_from_slice(&(u32::from(csum_type) | sector << 16).to_le_bytes());
        rec[36..38].copy_from_slice(&version.to_le_bytes());
        rec[38..40].copy_from_slice(&u64s.to_le_bytes());
        rec.extend_from_slice(&body);
        let csum = crate::csum::compute(csum_type, &rec[16..])?;
        rec[0..8].copy_from_slice(&csum.to_le_bytes());
        let padded = rec.len().div_ceil(block) * block;
        if start + padded > node_bytes {
            return Err(Error::Unsupported(
                "the node is full: splitting a node is not implemented".into(),
            ));
        }
        rec.resize(padded, 0);
        if write {
            self.dev.write_at(at + start as u64, &rec)?;
        }
        u16::try_from((start + padded) / 512)
            .map_err(|_| Error::Corrupt("sectors_written overflows".into()))
    }

    /// The root of btree `id` from the clean field: its level and key.
    fn root(&self, id: u8) -> Result<(u8, Bkey)> {
        let r = self
            .sb
            .btree_roots()?
            .into_iter()
            .find(|r| r.btree_id == id)
            .ok_or_else(|| Error::NotFound(format!("btree {id} has no root")))?;
        Ok((r.level, bkey::decode(&r.key, &UNPACKED)?))
    }

    /// Replace btree `id`'s root key in the clean field (same length).
    fn set_root(&mut self, id: u8, key: &Bkey) -> Result<()> {
        let bytes = encode_key(key)?;
        let (off, len) = self.clean_root_span(id)?;
        if len != bytes.len() {
            return Err(Error::Corrupt("the new root key's length differs".into()));
        }
        self.sb_raw[off..off + len].copy_from_slice(&bytes);
        self.sb = Superblock::parse_unchecked(&self.sb_raw)?;
        Ok(())
    }

    /// Where btree `id`'s root key sits in `sb_raw`.
    fn clean_root_span(&self, id: u8) -> Result<(usize, usize)> {
        let mut p = SB_HEADER_BYTES;
        while p + 8 <= self.sb_raw.len() {
            let u64s = le32(&self.sb_raw, p) as usize;
            let ty = le32(&self.sb_raw, p + 4);
            let end = p + u64s * 8;
            if u64s == 0 || end > self.sb_raw.len() {
                break;
            }
            if ty == FIELD_CLEAN {
                let mut e = p + 8 + 16;
                while e + 8 <= end {
                    let n = le16(&self.sb_raw, e) as usize;
                    let (btree_id, entry_type) = (self.sb_raw[e + 2], self.sb_raw[e + 4]);
                    if entry_type == 1 && btree_id == id && n > 0 {
                        return Ok((e + 8, n * 8));
                    }
                    e += 8 + n * 8;
                }
            }
            p = end;
        }
        Err(Error::NotFound(format!(
            "btree {id}'s root in the clean field"
        )))
    }

    /// Checksum the superblock and write it to every location its layout
    /// names.
    fn write_superblock(&mut self) -> Result<()> {
        let csum = crate::csum::compute(self.sb.csum_type(), &self.sb_raw[16..])?;
        self.sb_raw[0..16].fill(0);
        self.sb_raw[0..8].copy_from_slice(&csum.to_le_bytes());
        self.sb = Superblock::parse(&self.sb_raw)?;
        for &sector in &self.sb.layout.sb_offsets {
            let mut copy = self.sb_raw.clone();
            // Each copy records its own offset.
            copy[0x68..0x70].copy_from_slice(&sector.to_le_bytes());
            let csum = crate::csum::compute(self.sb.csum_type(), &copy[16..])?;
            copy[0..16].fill(0);
            copy[0..8].copy_from_slice(&csum.to_le_bytes());
            self.dev.write_at(sector * 512, &copy)?;
        }
        self.dev.flush()?;
        Ok(())
    }

    /// Give the device back.
    pub fn into_inner(self) -> D {
        self.dev
    }
}

/// Where a key lands: the position of a dirent, an inode, an extent.
pub fn pos(inode: u64, offset: u64) -> Bpos {
    Bpos {
        inode,
        offset,
        snapshot: u32::MAX,
    }
}

/// The btrees and key types the file operations touch.
mod ids {
    pub const EXTENTS: u8 = 0;
    pub const INODES: u8 = 1;
    pub const DIRENTS: u8 = 2;
    pub const LOGGED_OPS: u8 = 17;
    pub const ACCOUNTING: u8 = 20;
    pub const INODE_ALLOC_CURSOR: u8 = 35;
    pub const ACCOUNTING_KEY: u8 = 34;
}

/// The largest file this writer stores inline. The reference
/// implementation was seen storing up to 248 bytes inline (the aged
/// fixture) and nothing of 2024 bytes or more; past what was seen is not
/// guessed.
pub const INLINE_MAX: usize = 248;

/// Varint positions in an `inode_v3`, as the lister orders the fields
/// (the four times take two varints each).
mod field {
    pub const ATIME: usize = 0;
    pub const CTIME: usize = 2;
    pub const MTIME: usize = 4;
    pub const OTIME: usize = 6;
    pub const UID: usize = 8;
    pub const GID: usize = 9;
    pub const DIR: usize = 23;
    pub const DIR_OFFSET: usize = 24;
    /// Fields stored by a regular file the reference created.
    pub const FILE_FIELDS: u64 = 21;
}

/// The accounting key counting the keys and bytes a btree holds in the
/// all-ones snapshot: kind 5 in the top byte, the snapshot id, the btree id
/// at bits 16..24 (S8: `snapshot id=4294967295 btree=NAME` at
/// 0x05ffffffff000000 + id << 16; its value is keys, bytes, 0).
fn btree_counter_pos(id: u8) -> Bpos {
    Bpos {
        inode: 0x05ff_ffff_ff00_0000 | (u64::from(id) << 16),
        offset: 0,
        snapshot: 0,
    }
}

fn unpacked_bytes(k: &Bkey) -> i64 {
    (40 + k.value.len()) as i64
}

impl<D: BlockDevice> Writer<D> {
    /// Every leaf key of btree `id`, as it stands.
    fn keys(&self, id: u8) -> Result<Vec<Bkey>> {
        btree::walk(&self.dev, &self.sb, id)
    }

    fn key_at(&self, id: u8, pos: Bpos) -> Result<Option<Bkey>> {
        Ok(self.keys(id)?.into_iter().find(|k| k.pos == pos))
    }

    /// Nanoseconds since the filesystem's time base, the unit inode times
    /// are stored in (S8: precision 1, base in nanoseconds since the epoch).
    fn now(&self) -> u64 {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        ns.saturating_sub(self.sb.time_base_lo)
    }

    /// Create a regular file named `name` in directory `parent` holding
    /// `data` (at most [`INLINE_MAX`] bytes, stored inline). Returns its
    /// inode number.
    pub fn create_file(&mut self, parent: u64, name: &[u8], data: &[u8], mode: u32) -> Result<u64> {
        if data.len() > INLINE_MAX {
            return Err(Error::Unsupported(format!(
                "files over {INLINE_MAX} bytes need allocation, which is not implemented"
            )));
        }
        if name.is_empty() || name.len() > 255 || name.contains(&b'/') || name.contains(&0) {
            return Err(Error::Corrupt("not a valid file name".into()));
        }
        let parent_key = self
            .key_at(ids::INODES, pos(0, parent))?
            .ok_or_else(|| Error::NotFound(format!("inode {parent}")))?;
        let mut parent_raw = crate::inode::InodeV3Raw::parse(&parent_key.value)?;
        if parent_raw.mode() & 0o170000 != 0o040000 {
            return Err(Error::Corrupt(format!("inode {parent} is not a directory")));
        }
        let offset = crate::inode::dirent_hash(parent_raw.hash_seed, name);
        let dirent_pos = pos(parent, offset);
        if self.key_at(ids::DIRENTS, dirent_pos)?.is_some() {
            return Err(Error::Unsupported(
                "the name's hash slot is taken: hash collisions are not handled".into(),
            ));
        }

        // The next inode number, from the inode allocation cursor (S8: the
        // reference advanced it by one per create).
        let cursor = self
            .keys(ids::LOGGED_OPS)?
            .into_iter()
            .find(|k| k.key_type == ids::INODE_ALLOC_CURSOR)
            .ok_or_else(|| Error::Unsupported("no inode allocation cursor".into()))?;
        if cursor.value.len() < 16 {
            return Err(Error::Corrupt("inode allocation cursor too short".into()));
        }
        let ino = le64(&cursor.value, 8);
        if self.key_at(ids::INODES, pos(0, ino))?.is_some() {
            return Err(Error::Unsupported(format!(
                "inode {ino}, the cursor's next, is in use: searching for a free one is not implemented"
            )));
        }
        let mut new_cursor = cursor.clone();
        new_cursor.value[8..16].copy_from_slice(&(ino + 1).to_le_bytes());

        let now = self.now();
        let sectors = data.len().div_ceil(512) as u64;
        let mut varints = vec![0u64; 25];
        for t in [field::ATIME, field::CTIME, field::MTIME, field::OTIME] {
            varints[t] = now;
        }
        varints[field::UID] = 0;
        varints[field::GID] = 0;
        varints[field::DIR] = parent;
        varints[field::DIR_OFFSET] = offset;
        // The fixed flags bits as the reference set them on a file: its
        // own hash type and the four bits not yet understood come from the
        // parent, which shares them in every fixture.
        let inherited = parent_raw.flags & (0xf << 20 | 0xf << 32);
        let inode = crate::inode::InodeV3Raw {
            journal_seq: self.journal_seq,
            hash_seed: crate::siphash::siphash24(now, ino, name),
            flags: inherited
                | field::FILE_FIELDS << 24
                | u64::from(0o100000 | (mode & 0o7777)) << 36,
            sectors,
            size: data.len() as u64,
            version: 0,
            varints,
        };
        let new_inode = Bkey {
            key_type: key_type::INODE_V3,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: pos(0, ino),
            value: inode.encode(),
        };
        parent_raw.varints[field::CTIME] = now;
        parent_raw.varints[field::MTIME] = now;
        parent_raw.journal_seq = self.journal_seq;
        let new_parent = Bkey {
            value: parent_raw.encode(),
            ..parent_key.clone()
        };
        let dirent = Bkey {
            key_type: key_type::DIRENT,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: dirent_pos,
            value: crate::inode::Dirent {
                dir: parent,
                name: name.to_vec(),
                inum: ino,
                d_type: 8,
            }
            .encode_value(),
        };
        let inline = (!data.is_empty()).then(|| {
            let mut value = data.to_vec();
            value.resize(value.len().div_ceil(8) * 8, 0);
            Bkey {
                key_type: key_type::INLINE_DATA,
                size: sectors as u32,
                version_hi: 0,
                version_lo: 0,
                pos: pos(ino, sectors),
                value,
            }
        });

        // Accounting: one more inode, and each btree's key count and bytes.
        let mut counters: Vec<(u8, i64, i64)> = vec![
            (
                ids::INODES,
                1,
                unpacked_bytes(&new_inode) + unpacked_bytes(&new_parent)
                    - unpacked_bytes(&parent_key),
            ),
            (ids::DIRENTS, 1, unpacked_bytes(&dirent)),
        ];
        if let Some(k) = &inline {
            counters.push((ids::EXTENTS, 1, unpacked_bytes(k)));
        }
        let accounting = self.accounting_deltas(1, &counters)?;

        // Every btree is checked for room before any is written.
        let mut tx = vec![
            (ids::INODES, vec![new_inode, new_parent]),
            (ids::DIRENTS, vec![dirent]),
        ];
        tx.extend(inline.map(|k| (ids::EXTENTS, vec![k])));
        tx.push((ids::ACCOUNTING, accounting));
        tx.push((ids::LOGGED_OPS, vec![new_cursor]));
        for (id, keys) in &tx {
            self.check_fits(*id, keys)?;
        }
        for (id, keys) in tx {
            self.insert(id, keys)?;
        }
        Ok(ino)
    }

    /// The accounting keys after adding `inodes` to the inode count and
    /// `(btree, keys, bytes)` to each btree's counter.
    fn accounting_deltas(&self, inodes: i64, counters: &[(u8, i64, i64)]) -> Result<Vec<Bkey>> {
        let all = self.keys(ids::ACCOUNTING)?;
        let find = |p: Bpos| {
            all.iter()
                .find(|k| k.pos == p && k.key_type == ids::ACCOUNTING_KEY)
                .cloned()
                .ok_or_else(|| Error::Unsupported(format!("no accounting key at {p}")))
        };
        let add = |k: &mut Bkey, i: usize, d: i64| -> Result<()> {
            let at = i * 8;
            if k.value.len() < at + 8 {
                return Err(Error::Corrupt(format!(
                    "accounting key {} too short",
                    k.pos
                )));
            }
            let v = le64(&k.value, at)
                .checked_add_signed(d)
                .ok_or_else(|| Error::Corrupt("an accounting counter would go negative".into()))?;
            k.value[at..at + 8].copy_from_slice(&v.to_le_bytes());
            Ok(())
        };
        let mut out = Vec::new();
        let mut nr = find(Bpos::default())?;
        add(&mut nr, 0, inodes)?;
        out.push(nr);
        for &(id, keys, bytes) in counters {
            let mut k = find(btree_counter_pos(id))?;
            add(&mut k, 0, keys)?;
            add(&mut k, 1, bytes)?;
            out.push(k);
        }
        Ok(out)
    }
}

impl<D: BlockDevice> Writer<D> {
    /// Create a directory named `name` in `parent`. Returns its inode
    /// number.
    pub fn mkdir(&mut self, parent: u64, name: &[u8], mode: u32) -> Result<u64> {
        Err(Error::Unsupported(format!(
            "mkdir is not implemented yet ({parent}, {}, {mode:o})",
            name.len()
        )))
    }

    /// Remove the file or symlink `name` from `parent`.
    pub fn unlink(&mut self, parent: u64, name: &[u8]) -> Result<()> {
        Err(Error::Unsupported(format!(
            "unlink is not implemented yet ({parent}, {})",
            name.len()
        )))
    }

    /// Remove the empty directory `name` from `parent`.
    pub fn rmdir(&mut self, parent: u64, name: &[u8]) -> Result<()> {
        Err(Error::Unsupported(format!(
            "rmdir is not implemented yet ({parent}, {})",
            name.len()
        )))
    }

    /// Move `name` in `from` to `to_name` in `to`.
    pub fn rename(&mut self, from: u64, name: &[u8], to: u64, to_name: &[u8]) -> Result<()> {
        Err(Error::Unsupported(format!(
            "rename is not implemented yet ({from}, {}, {to}, {})",
            name.len(),
            to_name.len()
        )))
    }

    /// Replace a file's whole contents with `data` (inline; at most
    /// [`INLINE_MAX`] bytes; empty truncates it).
    pub fn write_file(&mut self, ino: u64, data: &[u8]) -> Result<()> {
        Err(Error::Unsupported(format!(
            "write_file is not implemented yet ({ino}, {})",
            data.len()
        )))
    }
}
