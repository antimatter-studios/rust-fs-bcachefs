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
        if !keys.is_empty() {
            return Err(Error::Unsupported(
                "inserting keys is not implemented yet".into(),
            ));
        }
        keys.sort_by_key(|k| k.pos);
        let (root_level, root_key) = self.root(id)?;
        let new_root = self.insert_at(&root_key, root_level, &keys)?;
        self.set_root(id, &new_root)?;
        self.write_superblock()
    }

    /// Insert into the subtree under `ptr_key` (at `level`); returns the
    /// pointer key with its new `sectors_written`.
    fn insert_at(&self, ptr_key: &Bkey, level: u8, keys: &[Bkey]) -> Result<Bkey> {
        let ptr = NodePtr::from_key(ptr_key)?;
        if level == 0 {
            let written = self.append_bset(&ptr, keys)?;
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
                updated.push(self.insert_at(child, level - 1, &rest[..n])?);
                rest = &rest[n..];
            }
        }
        if !rest.is_empty() {
            return Err(Error::Corrupt(format!(
                "key {} is past the last child of an interior node",
                rest[0].pos
            )));
        }
        let written = self.append_bset(&ptr, &updated)?;
        with_sectors_written(ptr_key, written)
    }

    /// Append one bset holding `keys` to the node; returns the node's new
    /// `sectors_written`.
    fn append_bset(&self, ptr: &NodePtr, keys: &[Bkey]) -> Result<u16> {
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
        self.dev.write_at(at + start as u64, &rec)?;
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
