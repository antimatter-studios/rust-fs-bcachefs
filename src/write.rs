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
    pub const NLINK: usize = 10;
    pub const DEPTH: usize = 28;
    /// Fields stored by a directory the reference created.
    pub const DIR_FIELDS: u64 = 25;
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

/// What one operation changes: keys per btree (a key of type `deleted`
/// removes the one at its position), and the accounting deltas they imply.
#[derive(Default)]
struct Txn {
    keys: std::collections::BTreeMap<u8, Vec<Bkey>>,
    /// Per btree: (keys added, bytes added), negative for removals.
    counters: std::collections::BTreeMap<u8, (i64, i64)>,
    inodes: i64,
}

impl Txn {
    /// Add `new` where `old` was (either may be absent).
    fn put(&mut self, id: u8, old: Option<&Bkey>, new: Bkey) {
        let c = self.counters.entry(id).or_default();
        if let Some(o) = old {
            c.0 -= 1;
            c.1 -= unpacked_bytes(o);
        }
        c.0 += 1;
        c.1 += unpacked_bytes(&new);
        self.keys.entry(id).or_default().push(new);
    }

    /// Remove `old`.
    fn delete(&mut self, id: u8, old: &Bkey) {
        let c = self.counters.entry(id).or_default();
        c.0 -= 1;
        c.1 -= unpacked_bytes(old);
        self.keys.entry(id).or_default().push(Bkey {
            key_type: key_type::DELETED,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: old.pos,
            value: Vec::new(),
        });
    }
}

/// An inode as the writer holds it: its key and its decoded value.
struct InodeRef {
    key: Bkey,
    raw: crate::inode::InodeV3Raw,
}

impl InodeRef {
    fn is_dir(&self) -> bool {
        self.raw.mode() & 0o170000 == 0o040000
    }

    fn rekey(&self) -> Bkey {
        Bkey {
            value: self.raw.encode(),
            ..self.key.clone()
        }
    }
}

fn valid_name(name: &[u8]) -> Result<()> {
    if name.is_empty()
        || name.len() > 255
        || name.contains(&b'/')
        || name.contains(&0)
        || name == b"."
        || name == b".."
    {
        return Err(Error::Corrupt(format!(
            "not a valid name: {:?}",
            String::from_utf8_lossy(name)
        )));
    }
    Ok(())
}

impl<D: BlockDevice> Writer<D> {
    /// Every leaf key of btree `id`, as it stands.
    fn keys(&self, id: u8) -> Result<Vec<Bkey>> {
        btree::walk(&self.dev, &self.sb, id)
    }

    fn key_at(&self, id: u8, pos: Bpos) -> Result<Option<Bkey>> {
        Ok(self.keys(id)?.into_iter().find(|k| k.pos == pos))
    }

    fn inode(&self, ino: u64) -> Result<InodeRef> {
        let key = self
            .key_at(ids::INODES, pos(0, ino))?
            .filter(|k| k.key_type == key_type::INODE_V3)
            .ok_or_else(|| Error::NotFound(format!("inode {ino}")))?;
        let raw = crate::inode::InodeV3Raw::parse(&key.value)?;
        Ok(InodeRef { key, raw })
    }

    fn dir(&self, ino: u64) -> Result<InodeRef> {
        let i = self.inode(ino)?;
        if !i.is_dir() {
            return Err(Error::Corrupt(format!("inode {ino} is not a directory")));
        }
        Ok(i)
    }

    /// The dirent named `name` in `dir`, if there is one. Names live at
    /// their hash; a different name there is a collision, which is refused.
    fn dirent(&self, dir: &InodeRef, name: &[u8]) -> Result<(Bpos, Option<Bkey>)> {
        let at = pos(
            dir.key.pos.offset,
            crate::inode::dirent_hash(dir.raw.hash_seed, name),
        );
        match self.key_at(ids::DIRENTS, at)? {
            Some(k) if k.key_type == key_type::DIRENT => {
                let d = crate::inode::Dirent::from_key(&k)?;
                if d.name != name {
                    return Err(Error::Unsupported(
                        "another name holds this name's hash slot: collisions are not handled"
                            .into(),
                    ));
                }
                Ok((at, Some(k)))
            }
            _ => Ok((at, None)),
        }
    }

    /// The extents of an inode, all of which must be inline: freeing
    /// allocated space is not implemented.
    fn inline_extents(&self, ino: u64) -> Result<Vec<Bkey>> {
        let keys: Vec<Bkey> = self
            .keys(ids::EXTENTS)?
            .into_iter()
            .filter(|k| k.pos.inode == ino && k.key_type != key_type::DELETED)
            .collect();
        if let Some(k) = keys.iter().find(|k| k.key_type != key_type::INLINE_DATA) {
            return Err(Error::Unsupported(format!(
                "inode {ino} has an allocated extent (type {}): freeing space is not implemented",
                k.key_type
            )));
        }
        Ok(keys)
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

    /// The next inode number from the allocation cursor, and the cursor
    /// advanced past it (S8: the reference advanced it by one per create).
    fn next_ino(&self) -> Result<(u64, Bkey)> {
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
        let mut next = cursor;
        next.value[8..16].copy_from_slice(&(ino + 1).to_le_bytes());
        Ok((ino, next))
    }

    fn touch(&self, i: &mut InodeRef, now: u64) {
        i.raw.varints[field::CTIME] = now;
        i.raw.varints[field::MTIME] = now;
        i.raw.journal_seq = self.journal_seq;
    }

    /// A new inode's value, in the shape the reference gave a new file or
    /// directory: the parent's hash type and the four flags bits not yet
    /// understood, every time `now`, `dir`/`dir_offset` naming its dirent.
    #[allow(clippy::too_many_arguments)]
    fn new_inode(
        &self,
        ino: u64,
        parent: &InodeRef,
        offset: u64,
        mode: u32,
        is_dir: bool,
        size: u64,
        now: u64,
        name: &[u8],
    ) -> Bkey {
        let fields = if is_dir {
            field::DIR_FIELDS
        } else {
            field::FILE_FIELDS
        };
        let mut varints = vec![0u64; fields as usize + 4];
        for t in [field::ATIME, field::CTIME, field::MTIME, field::OTIME] {
            varints[t] = now;
        }
        varints[field::DIR] = parent.key.pos.offset;
        varints[field::DIR_OFFSET] = offset;
        if is_dir {
            varints[field::DEPTH] = parent.raw.varints.get(field::DEPTH).copied().unwrap_or(0) + 1;
        }
        let kind = if is_dir { 0o040000 } else { 0o100000 };
        let inherited = parent.raw.flags & (0xf << 20 | 0xf << 32);
        let raw = crate::inode::InodeV3Raw {
            journal_seq: self.journal_seq,
            hash_seed: crate::siphash::siphash24(now, ino, name),
            flags: inherited | fields << 24 | u64::from(kind | (mode & 0o7777)) << 36,
            sectors: size.div_ceil(512),
            size,
            version: 0,
            varints,
        };
        Bkey {
            key_type: key_type::INODE_V3,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: pos(0, ino),
            value: raw.encode(),
        }
    }

    fn inline_key(ino: u64, data: &[u8]) -> Option<Bkey> {
        (!data.is_empty()).then(|| {
            let sectors = data.len().div_ceil(512) as u64;
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
        })
    }

    fn dirent_key(at: Bpos, name: &[u8], ino: u64, d_type: u8) -> Bkey {
        Bkey {
            key_type: key_type::DIRENT,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: at,
            value: crate::inode::Dirent {
                dir: at.inode,
                name: name.to_vec(),
                inum: ino,
                d_type,
            }
            .encode_value(),
        }
    }

    /// Write a transaction: the accounting it implies first computed from
    /// the keys as they stand, every btree checked for room, then each
    /// btree's keys, then accounting.
    fn commit(&mut self, mut t: Txn) -> Result<()> {
        let counters: Vec<(u8, i64, i64)> = t
            .counters
            .iter()
            .filter(|(_, (k, b))| *k != 0 || *b != 0)
            .map(|(&id, &(k, b))| (id, k, b))
            .collect();
        let accounting = self.accounting_deltas(t.inodes, &counters)?;
        // Every btree is checked for room before any is written.
        for (id, keys) in &t.keys {
            self.check_fits(*id, keys)?;
        }
        self.check_fits(ids::ACCOUNTING, &accounting)?;
        for (id, keys) in std::mem::take(&mut t.keys) {
            self.insert(id, keys)?;
        }
        self.insert(ids::ACCOUNTING, accounting)
    }

    fn create(
        &mut self,
        parent: u64,
        name: &[u8],
        data: &[u8],
        mode: u32,
        is_dir: bool,
    ) -> Result<u64> {
        valid_name(name)?;
        if data.len() > INLINE_MAX {
            return Err(Error::Unsupported(format!(
                "files over {INLINE_MAX} bytes need allocation, which is not implemented"
            )));
        }
        let mut p = self.dir(parent)?;
        let (at, existing) = self.dirent(&p, name)?;
        if existing.is_some() {
            return Err(Error::Corrupt(format!(
                "{:?} already exists",
                String::from_utf8_lossy(name)
            )));
        }
        let (ino, cursor) = self.next_ino()?;
        let now = self.now();
        let mut t = Txn {
            inodes: 1,
            ..Txn::default()
        };
        let new = self.new_inode(
            ino,
            &p,
            at.offset,
            mode,
            is_dir,
            data.len() as u64,
            now,
            name,
        );
        t.put(ids::INODES, None, new);
        let old_parent = p.key.clone();
        self.touch(&mut p, now);
        if is_dir {
            // The parent stores its number of subdirectories (S8: mkdir
            // raised it by one).
            p.raw.varints[field::NLINK] += 1;
        }
        t.put(ids::INODES, Some(&old_parent), p.rekey());
        t.put(
            ids::DIRENTS,
            None,
            Self::dirent_key(at, name, ino, if is_dir { 4 } else { 8 }),
        );
        if let Some(k) = Self::inline_key(ino, data) {
            t.put(ids::EXTENTS, None, k);
        }
        // The cursor replaces itself; logged_ops has no counter.
        t.keys.entry(ids::LOGGED_OPS).or_default().push(cursor);
        self.commit(t)?;
        Ok(ino)
    }

    /// Create a regular file named `name` in directory `parent` holding
    /// `data` (at most [`INLINE_MAX`] bytes, stored inline). Returns its
    /// inode number.
    pub fn create_file(&mut self, parent: u64, name: &[u8], data: &[u8], mode: u32) -> Result<u64> {
        self.create(parent, name, data, mode, false)
    }

    /// Create a directory named `name` in `parent`. Returns its inode
    /// number.
    pub fn mkdir(&mut self, parent: u64, name: &[u8], mode: u32) -> Result<u64> {
        self.create(parent, name, &[], mode, true)
    }

    fn remove(&mut self, parent: u64, name: &[u8], want_dir: bool) -> Result<()> {
        valid_name(name)?;
        let mut p = self.dir(parent)?;
        let (_, dirent) = self.dirent(&p, name)?;
        let dirent = dirent
            .ok_or_else(|| Error::NotFound(format!("{:?}", String::from_utf8_lossy(name))))?;
        let target = crate::inode::Dirent::from_key(&dirent)?.inum;
        let mut i = self.inode(target)?;
        if i.is_dir() != want_dir {
            return Err(Error::Corrupt(format!(
                "{:?} is {}a directory",
                String::from_utf8_lossy(name),
                if want_dir { "not " } else { "" }
            )));
        }
        let now = self.now();
        let mut t = Txn::default();
        if want_dir {
            let has_entries = self
                .keys(ids::DIRENTS)?
                .iter()
                .any(|k| k.pos.inode == target && k.key_type == key_type::DIRENT);
            if has_entries {
                return Err(Error::Corrupt("the directory is not empty".into()));
            }
        }
        t.delete(ids::DIRENTS, &dirent);
        if !want_dir && i.raw.varints[field::NLINK] > 0 {
            // Another name still links it: one link fewer.
            let old = i.key.clone();
            i.raw.varints[field::NLINK] -= 1;
            i.raw.varints[field::CTIME] = now;
            i.raw.journal_seq = self.journal_seq;
            t.put(ids::INODES, Some(&old), i.rekey());
        } else {
            for k in self.inline_extents(target)? {
                t.delete(ids::EXTENTS, &k);
            }
            t.delete(ids::INODES, &i.key);
            t.inodes = -1;
        }
        let old_parent = p.key.clone();
        self.touch(&mut p, now);
        if want_dir {
            p.raw.varints[field::NLINK] = p.raw.varints[field::NLINK].saturating_sub(1);
        }
        t.put(ids::INODES, Some(&old_parent), p.rekey());
        self.commit(t)
    }

    /// Remove the file or symlink `name` from `parent`.
    pub fn unlink(&mut self, parent: u64, name: &[u8]) -> Result<()> {
        self.remove(parent, name, false)
    }

    /// Remove the empty directory `name` from `parent`.
    pub fn rmdir(&mut self, parent: u64, name: &[u8]) -> Result<()> {
        self.remove(parent, name, true)
    }

    /// Move `name` in `from` to `to_name` in `to`, which must not exist.
    pub fn rename(&mut self, from: u64, name: &[u8], to: u64, to_name: &[u8]) -> Result<()> {
        valid_name(name)?;
        valid_name(to_name)?;
        let src = self.dir(from)?;
        let (_, dirent) = self.dirent(&src, name)?;
        let dirent = dirent
            .ok_or_else(|| Error::NotFound(format!("{:?}", String::from_utf8_lossy(name))))?;
        let d = crate::inode::Dirent::from_key(&dirent)?;
        let dst = self.dir(to)?;
        let (at, existing) = self.dirent(&dst, to_name)?;
        if existing.is_some() {
            return Err(Error::Unsupported(
                "renaming over an existing name is not implemented".into(),
            ));
        }
        let mut i = self.inode(d.inum)?;
        if i.is_dir() && from != to {
            return Err(Error::Unsupported(
                "moving a directory to another directory is not implemented".into(),
            ));
        }
        let now = self.now();
        let mut t = Txn::default();
        t.delete(ids::DIRENTS, &dirent);
        t.put(
            ids::DIRENTS,
            None,
            Self::dirent_key(at, to_name, d.inum, d.d_type),
        );
        // The inode names its dirent (S8: rename changed bi_dir_offset and
        // the inode's ctime).
        let old = i.key.clone();
        i.raw.varints[field::DIR] = to;
        i.raw.varints[field::DIR_OFFSET] = at.offset;
        i.raw.varints[field::CTIME] = now;
        i.raw.journal_seq = self.journal_seq;
        t.put(ids::INODES, Some(&old), i.rekey());
        for dir in if from == to {
            vec![from]
        } else {
            vec![from, to]
        } {
            let mut p = self.dir(dir)?;
            let old = p.key.clone();
            self.touch(&mut p, now);
            t.put(ids::INODES, Some(&old), p.rekey());
        }
        self.commit(t)
    }

    /// Replace a file's whole contents with `data` (inline; at most
    /// [`INLINE_MAX`] bytes; empty truncates it).
    pub fn write_file(&mut self, ino: u64, data: &[u8]) -> Result<()> {
        if data.len() > INLINE_MAX {
            return Err(Error::Unsupported(format!(
                "files over {INLINE_MAX} bytes need allocation, which is not implemented"
            )));
        }
        let mut i = self.inode(ino)?;
        if i.raw.mode() & 0o170000 != 0o100000 {
            return Err(Error::Corrupt(format!("inode {ino} is not a regular file")));
        }
        let now = self.now();
        let mut t = Txn::default();
        let old_extents = self.inline_extents(ino)?;
        let new = Self::inline_key(ino, data);
        for k in &old_extents {
            if new.as_ref().map(|n| n.pos) != Some(k.pos) {
                t.delete(ids::EXTENTS, k);
            }
        }
        if let Some(n) = new {
            let old = old_extents.iter().find(|k| k.pos == n.pos);
            t.put(ids::EXTENTS, old, n);
        }
        let old = i.key.clone();
        i.raw.size = data.len() as u64;
        i.raw.sectors = (data.len() as u64).div_ceil(512);
        self.touch(&mut i, now);
        t.put(ids::INODES, Some(&old), i.rekey());
        self.commit(t)
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
        if inodes != 0 {
            let mut nr = find(Bpos::default())?;
            add(&mut nr, 0, inodes)?;
            out.push(nr);
        }
        for &(id, keys, bytes) in counters {
            let mut k = find(btree_counter_pos(id))?;
            add(&mut k, 0, keys)?;
            add(&mut k, 1, bytes)?;
            out.push(k);
        }
        Ok(out)
    }
}
