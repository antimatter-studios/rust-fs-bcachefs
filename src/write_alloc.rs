//! Allocation, for files too large to store inline: whole free buckets are
//! taken from the freespace btree, the data written into them with
//! checksummed extents, and every key the reference implementation was
//! seen to write for a large file is written with them (the write study's
//! create-large pair, S8; docs/clean-room.md "Allocating space").

use super::{ids, pos, Txn, Writer};
use crate::bkey::{key_type, Bkey, Bpos};
use crate::error::{Error, Result};
use crate::util::le64;
use fs_core::BlockDevice;

/// Btrees and key types allocation touches (S1's orders; checked against
/// the reference lister on the write study's images).
pub(super) mod aids {
    pub const ALLOC: u8 = 4;
    pub const LRU: u8 = 10;
    pub const FREESPACE: u8 = 11;
    pub const BACKPOINTERS: u8 = 13;
    pub const BUCKET_GENS: u8 = 14;
    pub const SET: u8 = 25;
    pub const ALLOC_V4: u8 = 27;
    pub const BACKPOINTER: u8 = 28;
    pub const BUCKET_GENS_KEY: u8 = 30;
}

/// The largest extent one crc32 entry describes: its sizes are 7 bits,
/// stored minus one (src/extent.rs).
const MAX_EXTENT_SECTORS: u64 = 128;

/// Data types in alloc keys, accounting and backpointers (S1's order; the
/// lister names 1 sb, 2 journal, 3 btree, 4 user).
const DATA_USER: u8 = 4;

/// A bucket's data type sits in byte 6 of the alloc_v4 value's second
/// word; the low byte of that word held 0x23 in every bucket the reference
/// filled with data (S8, every user bucket of every fixture).
const ALLOC_FLAGS_USER: u64 = (DATA_USER as u64) << 48 | 0x23;

/// The accounting key kinds and their positions (S8): the first byte of the
/// position's inode field is the kind, and the fields follow it byte by
/// byte.
fn acct_replicas_user(dev: u8) -> Bpos {
    Bpos {
        inode: 0x0204_0101_0000_0000 | u64::from(dev) << 24,
        offset: 0,
        snapshot: 0,
    }
}

fn acct_dev_data_type(dev: u8, data_type: u8) -> Bpos {
    Bpos {
        inode: 0x0300_0000_0000_0000 | u64::from(dev) << 48 | u64::from(data_type) << 40,
        offset: 0,
        snapshot: 0,
    }
}

/// The per-inode accounting key: kind 8, then the inode number's bytes in
/// little-endian order, read as a big-endian position (S8: inode
/// 0x80000003 at 0x0803000080000000).
fn acct_inum(ino: u64) -> Bpos {
    let mut b = [0u8; 16];
    b[0] = 8;
    b[1..9].copy_from_slice(&ino.to_le_bytes());
    Bpos {
        inode: u64::from_be_bytes(b[0..8].try_into().expect("8 bytes")),
        offset: u64::from_be_bytes(b[8..16].try_into().expect("8 bytes")),
        snapshot: 0,
    }
}

/// One extent this writer is about to write.
pub(super) struct Planned {
    pub file_sector: u64,
    pub sectors: u64,
    pub dev_sector: u64,
    pub bucket: u64,
    pub gen: u8,
}

impl<D: BlockDevice> Writer<D> {
    pub(super) fn bucket_sectors(&self) -> Result<u64> {
        let members = self.sb.members()?;
        let m = members
            .first()
            .ok_or_else(|| Error::Corrupt("no member device".into()))?;
        Ok(u64::from(m.bucket_size))
    }

    /// Take `n` whole free buckets, lowest first, from the freespace btree's
    /// runs (keys of type `set` at `dev:end`, `size` buckets long, S8).
    /// Returns the buckets and the keys that shrink the runs.
    fn take_buckets(&self, n: u64, t: &mut Txn) -> Result<Vec<u64>> {
        let runs: Vec<Bkey> = self
            .keys(aids::FREESPACE)?
            .into_iter()
            .filter(|k| k.key_type == aids::SET && k.pos.inode == 0)
            .collect();
        // Buckets with an alloc key of any data type but free (byte 6 of
        // the second word, 0) are not free, whatever freespace says.
        let used: std::collections::BTreeSet<u64> = self
            .keys(aids::ALLOC)?
            .iter()
            .filter(|k| k.pos.inode == 0 && k.value.len() >= 16 && k.value[14] != 0)
            .map(|k| k.pos.offset)
            .collect();
        let mut out = Vec::new();
        for run in runs {
            if out.len() as u64 == n {
                break;
            }
            let end = run.pos.offset;
            let start = end
                .checked_sub(u64::from(run.size))
                .ok_or_else(|| Error::Corrupt("a freespace run before bucket 0".into()))?;
            let mut first_unused = start;
            while first_unused < end && (out.len() as u64) < n {
                if used.contains(&first_unused) {
                    return Err(Error::Corrupt(format!(
                        "bucket {first_unused} is free and has an alloc key"
                    )));
                }
                out.push(first_unused);
                first_unused += 1;
            }
            // The run now starts later: the same end, fewer buckets; or it
            // is gone.
            if first_unused == end {
                t.delete_uncounted(aids::FREESPACE, run.pos);
            } else if first_unused > start {
                t.put_uncounted(
                    aids::FREESPACE,
                    Bkey {
                        size: (end - first_unused) as u32,
                        ..run.clone()
                    },
                );
            }
        }
        if (out.len() as u64) < n {
            return Err(Error::Unsupported(format!(
                "{n} free buckets are needed and only {} are free",
                out.len()
            )));
        }
        Ok(out)
    }

    /// A bucket's generation: byte `bucket % 256` of the bucket_gens key at
    /// `dev:bucket / 256` (type 30, 256 one-byte generations; S3 + S4: the
    /// lister prints them in order and the alloc keys of reused buckets
    /// carry the same), 0 when there is none.
    fn bucket_gen(&self, bucket: u64) -> Result<u8> {
        let gens = match self.keys(aids::BUCKET_GENS) {
            Err(Error::NotFound(_)) => return Ok(0),
            r => r?,
        };
        Ok(gens
            .iter()
            .find(|k| {
                k.key_type == aids::BUCKET_GENS_KEY
                    && k.pos.inode == 0
                    && k.pos.offset == bucket >> 8
            })
            .and_then(|k| k.value.get((bucket & 0xff) as usize).copied())
            .unwrap_or(0))
    }

    /// The largest write clock any bucket recorded (alloc_v4 word 4), for
    /// the buckets this writer fills.
    fn write_clock(&self) -> Result<u64> {
        Ok(self
            .keys(aids::ALLOC)?
            .iter()
            .filter(|k| k.key_type == aids::ALLOC_V4 && k.value.len() >= 40)
            .map(|k| le64(&k.value, 32))
            .max()
            .unwrap_or(1))
    }

    /// Write `data` for inode `ino` into fresh buckets and add every key
    /// that goes with it to `t`. Returns the sectors allocated.
    pub(super) fn allocate_data(&mut self, ino: u64, data: &[u8], t: &mut Txn) -> Result<u64> {
        let bucket = self.bucket_sectors()?;
        if bucket == 0 {
            return Err(Error::Corrupt("bucket size 0".into()));
        }
        let csum_type = match self.sb.data_checksum_opt() {
            1 => 5u8, // crc32c, from zero, not inverted (data checksums, S4)
            o => {
                return Err(Error::Unsupported(format!(
                    "data checksum option {o}: only crc32c is written"
                )))
            }
        };
        if self.sb.compression_opt() != 0 {
            return Err(Error::Unsupported(
                "writing compressed data is not implemented".into(),
            ));
        }
        let sectors = (data.len() as u64).div_ceil(512);
        let data_buckets = sectors.div_ceil(bucket);
        // A bucket left partly empty needs an lru entry, and a filesystem
        // that never had one has no lru btree: its root is made here, in one
        // more bucket (S8: the reference made exactly that for its first
        // large file).
        let lru_root = !sectors.is_multiple_of(bucket) && self.root(aids::LRU).is_err();
        let mut buckets = self.take_buckets(data_buckets + u64::from(lru_root), t)?;
        if lru_root {
            let node_bucket = buckets.pop().expect("one more was taken");
            self.create_root(aids::LRU, node_bucket, t)?;
        }
        let clock = self.write_clock()?;

        // Plan the extents: each within one bucket, at most 128 sectors.
        let mut plan = Vec::new();
        let mut done = 0u64;
        for &b in &buckets {
            let gen = self.bucket_gen(b)?;
            let mut in_bucket = 0;
            while in_bucket < bucket && done < sectors {
                let n = (bucket - in_bucket)
                    .min(MAX_EXTENT_SECTORS)
                    .min(sectors - done);
                plan.push(Planned {
                    file_sector: done,
                    sectors: n,
                    dev_sector: b * bucket + in_bucket,
                    bucket: b,
                    gen,
                });
                in_bucket += n;
                done += n;
            }
        }

        for p in &plan {
            let from = (p.file_sector * 512) as usize;
            let to = (from + (p.sectors * 512) as usize).min(data.len());
            let mut buf = data[from..to].to_vec();
            buf.resize((p.sectors * 512) as usize, 0);
            self.dev.write_at(p.dev_sector * 512, &buf)?;
            let csum = crate::csum::compute(csum_type, &buf)?;
            // crc32 entry (type bit 1), then the pointer (type bit 0).
            let crc: u64 = 0b10
                | (p.sectors - 1) << 2
                | (p.sectors - 1) << 9
                | u64::from(csum_type) << 24
                | (csum & 0xffff_ffff) << 32;
            let ptr: u64 = 1 | p.dev_sector << 4 | u64::from(p.gen) << 56;
            let mut value = crc.to_le_bytes().to_vec();
            value.extend_from_slice(&ptr.to_le_bytes());
            let extent = Bkey {
                key_type: key_type::EXTENT,
                size: p.sectors as u32,
                version_hi: 0,
                version_lo: 0,
                pos: pos(ino, p.file_sector + p.sectors),
                value,
            };
            t.put(ids::EXTENTS, None, extent.clone());
            t.count(
                super::btree_counter_pos(ids::EXTENTS),
                3,
                2,
                p.sectors as i64,
            );

            // The backpointer: at the data's device sector << 16, naming
            // the extent (S8: btree, level, data type, bucket_len, pos).
            let mut bp = Vec::with_capacity(32);
            bp.extend_from_slice(&[ids::EXTENTS, 0, DATA_USER, 0, 0, 0, 0, 0]);
            bp.extend_from_slice(&(p.sectors as u32).to_le_bytes());
            bp.extend_from_slice(&extent.pos.snapshot.to_le_bytes());
            bp.extend_from_slice(&extent.pos.offset.to_le_bytes());
            bp.extend_from_slice(&extent.pos.inode.to_le_bytes());
            t.put_uncounted(
                aids::BACKPOINTERS,
                Bkey {
                    key_type: aids::BACKPOINTER,
                    size: 0,
                    version_hi: 0,
                    version_lo: 0,
                    pos: Bpos {
                        inode: 0,
                        offset: p.dev_sector << 16,
                        snapshot: 0,
                    },
                    value: bp,
                },
            );
        }

        // One alloc key per bucket, and an lru entry for one not full.
        for &b in &buckets {
            let dirty: u64 = plan
                .iter()
                .filter(|p| p.bucket == b)
                .map(|p| p.sectors)
                .sum();
            let mut v = Vec::with_capacity(64);
            let gen = u64::from(
                plan.iter()
                    .find(|p| p.bucket == b)
                    .map(|p| p.gen)
                    .unwrap_or(0),
            );
            for w in [
                self.journal_seq,
                ALLOC_FLAGS_USER | gen << 40 | gen << 32,
                dirty,
                1,
                clock,
                0,
                0,
                0,
            ] {
                v.extend_from_slice(&w.to_le_bytes());
            }
            t.put_uncounted(
                aids::ALLOC,
                Bkey {
                    key_type: aids::ALLOC_V4,
                    size: 0,
                    version_hi: 0,
                    version_lo: 0,
                    pos: Bpos {
                        inode: 0,
                        offset: b,
                        snapshot: 0,
                    },
                    value: v,
                },
            );
            if dirty < bucket {
                // Fragmentation lru (S8): (1 << 61) | dirty/bucket in 2^31ths.
                let frag = (dirty << 31) / bucket;
                t.put_uncounted(
                    aids::LRU,
                    Bkey {
                        key_type: aids::SET,
                        size: 0,
                        version_hi: 0,
                        version_lo: 0,
                        pos: Bpos {
                            inode: 1 << 61 | frag,
                            offset: b,
                            snapshot: 0,
                        },
                        value: Vec::new(),
                    },
                );
            }
        }

        // Accounting (S8): user sectors on the device, the device's user
        // buckets, sectors and the unused part of them, one free bucket
        // fewer each, and the inode's extents and sectors.
        let n = buckets.len() as i64;
        let s = sectors as i64;
        t.count(acct_replicas_user(0), 1, 0, s);
        let user = acct_dev_data_type(0, DATA_USER);
        t.count(user, 3, 0, n);
        t.count(user, 3, 1, s);
        t.count(user, 3, 2, n * bucket as i64 - s);
        t.count(acct_dev_data_type(0, 0), 3, 0, -n);
        let inum = acct_inum(ino);
        t.count(inum, 3, 0, plan.len() as i64);
        t.count(inum, 3, 1, s);
        t.count(inum, 3, 2, s);
        Ok(sectors)
    }
}

impl<D: BlockDevice> Writer<D> {
    /// Make an empty root node for btree `id` in `bucket`: a header and one
    /// empty bset, as the reference wrote for the lru btree it created
    /// (S8): the btree id and level in the header's flags, the other flag
    /// bits as an existing node of this filesystem carries them, min key
    /// POS_MIN, max key SPOS_MAX, the 64/64/32 unpacked key format. The
    /// root key goes into the superblock's clean field; the bucket's alloc
    /// key, its backpointer and the accounting go into `t`.
    fn create_root(&mut self, id: u8, bucket: u64, t: &mut Txn) -> Result<()> {
        if id >= 16 {
            return Err(Error::Unsupported(format!(
                "a root for btree {id}: where ids of 16 and more go in a node's flags is not known"
            )));
        }
        let bucket_sectors = self.bucket_sectors()?;
        let node_sectors = u64::from(self.sb.btree_node_size());
        if node_sectors > bucket_sectors {
            return Err(Error::Unsupported(
                "btree nodes larger than a bucket".into(),
            ));
        }
        let block = (self.sb.block_size as usize * 512).max(512);
        // A template: the extents btree's root node.
        let (_, ext_root) = self.root(ids::EXTENTS)?;
        let ext_ptr = crate::btree::NodePtr::from_key(&ext_root)?;
        let mut tmpl = vec![0u8; 160];
        self.dev.read_at(ext_ptr.ptrs[0].offset * 512, &mut tmpl)?;
        let tmpl_flags = le64(&tmpl, 24);
        let first_bset_flags = crate::util::le32(&tmpl, 136 + 16);
        let version = crate::util::le16(&tmpl, 136 + 20);

        let gen = u64::from(self.bucket_gen(bucket)?);
        let seq =
            crate::siphash::siphash24(self.journal_seq, bucket, b"rust-fs-bcachefs node seq") | 1;
        let mut node = vec![0u8; 160];
        node[16..24].copy_from_slice(&crate::btree::node_magic(&self.sb.uuid).to_le_bytes());
        node[24..32].copy_from_slice(&((tmpl_flags & !0xff) | u64::from(id)).to_le_bytes());
        // min_key 32..52 stays POS_MIN; max_key 52..72 is SPOS_MAX.
        node[52..72].fill(0xff);
        // The key format: 3 u64s, 6 fields, 64/64/32 bits, no offsets.
        node[80..88].copy_from_slice(&[3, 6, 64, 64, 32, 0, 0, 0]);
        node[136..144].copy_from_slice(&seq.to_le_bytes());
        node[152..156].copy_from_slice(&(first_bset_flags & 0xf).to_le_bytes());
        node[156..158].copy_from_slice(&version.to_le_bytes());
        let csum = crate::csum::compute((first_bset_flags & 0xf) as u8, &node[16..160])?;
        node[0..8].copy_from_slice(&csum.to_le_bytes());
        node.resize(160usize.div_ceil(block) * block, 0);
        let dev_sector = bucket * bucket_sectors;
        self.dev.write_at(dev_sector * 512, &node)?;

        // The root key: btree_ptr_v2 at SPOS_MAX.
        let mut v = Vec::with_capacity(48);
        v.extend_from_slice(&0u64.to_le_bytes());
        v.extend_from_slice(&seq.to_le_bytes());
        v.extend_from_slice(&((node.len() / 512) as u16).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&[0u8; 20]);
        v.extend_from_slice(&(1u64 | dev_sector << 4 | gen << 56).to_le_bytes());
        let root = Bkey {
            key_type: key_type::BTREE_PTR_V2,
            size: 0,
            version_hi: 0,
            version_lo: 0,
            pos: SPOS_MAX,
            value: v,
        };
        if self.session.is_some() {
            // Journalled: every entry from now on records the new root.
            self.session_add_root(id, 0, root);
            self.mark_btree_bitmap(dev_sector, node_sectors)?;
            self.write_superblock()?;
        } else {
            self.add_root_entry(id, &super::encode_key(&root)?)?;
            self.mark_btree_bitmap(dev_sector, node_sectors)?;
        }

        let clock = self.write_clock()?;
        let mut a = Vec::with_capacity(64);
        for w in [
            self.journal_seq,
            u64::from(DATA_BTREE) << 48 | gen << 40 | gen << 32 | 0x23,
            node_sectors,
            1,
            clock,
            0,
            0,
            0,
        ] {
            a.extend_from_slice(&w.to_le_bytes());
        }
        t.put_uncounted(
            aids::ALLOC,
            Bkey {
                key_type: aids::ALLOC_V4,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: Bpos {
                    inode: 0,
                    offset: bucket,
                    snapshot: 0,
                },
                value: a,
            },
        );
        // The node's backpointer names the key pointing at it: the root key,
        // one level up (S8: btree=lru level=1 pos=SPOS_MAX).
        let mut bp = Vec::with_capacity(32);
        bp.extend_from_slice(&[id, 1, DATA_BTREE, 0, 0, 0, 0, 0]);
        bp.extend_from_slice(&(node_sectors as u32).to_le_bytes());
        bp.extend_from_slice(&SPOS_MAX.snapshot.to_le_bytes());
        bp.extend_from_slice(&SPOS_MAX.offset.to_le_bytes());
        bp.extend_from_slice(&SPOS_MAX.inode.to_le_bytes());
        t.put_uncounted(
            aids::BACKPOINTERS,
            Bkey {
                key_type: aids::BACKPOINTER,
                size: 0,
                version_hi: 0,
                version_lo: 0,
                pos: Bpos {
                    inode: 0,
                    offset: dev_sector << 16,
                    snapshot: 0,
                },
                value: bp,
            },
        );
        let n = node_sectors as i64;
        t.count(acct_replicas(DATA_BTREE, 0), 1, 0, n);
        let btree = acct_dev_data_type(0, DATA_BTREE);
        t.count(btree, 3, 0, 1);
        t.count(btree, 3, 1, n);
        t.count(btree, 3, 2, bucket_sectors as i64 - n);
        t.count(acct_dev_data_type(0, 0), 3, 0, -1);
        let per_btree = Bpos {
            inode: 6 << 56 | u64::from(id) << 48,
            offset: 0,
            snapshot: 0,
        };
        t.count(per_btree, 3, 0, n);
        t.count(per_btree, 3, 1, 1);
        Ok(())
    }

    /// Mark the device range of a btree node in the member's btree
    /// allocated bitmap: a u64 at member offset 128, one bit per 2^shift
    /// sectors, the shift a byte at member offset 28 (S3 + S4: the printer's
    /// "Btree allocated bitmap" read most significant bit first, blocksize
    /// 128, against the bytes; the reference set bit 48 for a node at sector
    /// 6144). The reference checker names a node outside it.
    fn mark_btree_bitmap(&mut self, dev_sector: u64, sectors: u64) -> Result<()> {
        let mut p = super::SB_HEADER_BYTES;
        let raw = &mut self.sb_raw;
        while p + 8 <= raw.len() {
            let u64s = crate::util::le32(raw, p) as usize;
            let ty = crate::util::le32(raw, p + 4);
            if u64s == 0 {
                break;
            }
            if ty == crate::superblock::FIELD_MEMBERS_V2 {
                let body = p + 8;
                let member_size = crate::util::le16(raw, body) as usize;
                let m = body + 8; // device 0
                if member_size < 136 || m + 136 > p + u64s * 8 {
                    return Err(Error::Corrupt(
                        "members_v2 too short for the btree bitmap".into(),
                    ));
                }
                let shift = u32::from(raw[m + 28]);
                let first = dev_sector >> shift;
                let last = (dev_sector + sectors - 1) >> shift;
                if last >= 64 {
                    return Err(Error::Unsupported(
                        "a btree node past the btree bitmap's 64 regions".into(),
                    ));
                }
                let mut bits = le64(raw, m + 128);
                for b in first..=last {
                    bits |= 1 << b;
                }
                raw[m + 128..m + 136].copy_from_slice(&bits.to_le_bytes());
                self.sb = crate::superblock::Superblock::parse_unchecked(raw)?;
                return Ok(());
            }
            p += u64s * 8;
        }
        Err(Error::Corrupt("no members_v2 field".into()))
    }

    /// Add a `btree_root` entry for btree `id` at the end of the clean
    /// field, growing the field and the superblock.
    fn add_root_entry(&mut self, id: u8, key: &[u8]) -> Result<()> {
        let mut p = super::SB_HEADER_BYTES;
        let raw = &mut self.sb_raw;
        while p + 8 <= raw.len() {
            let u64s = crate::util::le32(raw, p) as usize;
            let ty = crate::util::le32(raw, p + 4);
            if u64s == 0 {
                break;
            }
            let end = p + u64s * 8;
            if ty == crate::superblock::FIELD_CLEAN {
                let mut entry = Vec::with_capacity(8 + key.len());
                entry.extend_from_slice(&((key.len() / 8) as u16).to_le_bytes());
                entry.extend_from_slice(&[id, 0, 1, 0, 0, 0]);
                entry.extend_from_slice(key);
                let grow = entry.len() / 8;
                raw.splice(end..end, entry);
                raw[p..p + 4].copy_from_slice(&((u64s + grow) as u32).to_le_bytes());
                let total = crate::util::le32(raw, 0x7c) as usize + grow;
                raw[0x7c..0x80].copy_from_slice(&(total as u32).to_le_bytes());
                self.sb = crate::superblock::Superblock::parse_unchecked(raw)?;
                return Ok(());
            }
            p = end;
        }
        Err(Error::Corrupt("no clean field to add a root to".into()))
    }
}

const SPOS_MAX: Bpos = Bpos {
    inode: u64::MAX,
    offset: u64::MAX,
    snapshot: u32::MAX,
};

const DATA_BTREE: u8 = 3;

fn acct_replicas(data_type: u8, dev: u8) -> Bpos {
    Bpos {
        inode: 0x0200_0101_0000_0000 | u64::from(data_type) << 48 | u64::from(dev) << 24,
        offset: 0,
        snapshot: 0,
    }
}

/// Data type of a bucket emptied and waiting for a discard (S3: the lister
/// names 9 need_discard).
const DATA_NEED_DISCARD: u8 = 9;
const NEED_DISCARD: u8 = 12;

impl<D: BlockDevice> Writer<D> {
    /// Remove an inode's extents -- inline ones and allocated ones -- and
    /// give allocated space back the way the reference does (the write
    /// study's unlink-large pair, S8): each bucket loses the extent's
    /// sectors; an emptied bucket becomes need_discard with its generation
    /// and oldest generation one higher, its journal_seq_empty set and
    /// need_inc_gen cleared, gets a need_discard key at
    /// `journal_seq_empty:bucket`, and its generation is recorded in
    /// bucket_gens; backpointers and lru entries go; accounting moves the
    /// buckets from user to need_discard.
    pub(super) fn free_extents(&mut self, ino: u64, extents: &[Bkey], t: &mut Txn) -> Result<()> {
        let bucket = self.bucket_sectors()?;
        let mut freed: std::collections::BTreeMap<u64, u64> = std::collections::BTreeMap::new();
        let mut nr = 0i64;
        for k in extents {
            t.delete(ids::EXTENTS, k);
            if k.key_type == key_type::INLINE_DATA {
                continue;
            }
            if k.key_type != key_type::EXTENT {
                return Err(Error::Unsupported(format!(
                    "freeing an extent of key type {}",
                    k.key_type
                )));
            }
            let e = crate::extent::DataExtent::from_key(k)?;
            if e.ptr.dev != 0 {
                return Err(Error::Unsupported("extents on another device".into()));
            }
            let sectors = e.crc.map(|c| u64::from(c.compressed_size)).unwrap_or(e.len);
            if e.crc
                .is_some_and(|c| c.offset != 0 || u64::from(c.uncompressed_size) != e.len)
            {
                return Err(Error::Unsupported(
                    "freeing part of a checksummed extent is not implemented".into(),
                ));
            }
            t.delete_uncounted(
                aids::BACKPOINTERS,
                Bpos {
                    inode: 0,
                    offset: e.ptr.offset << 16,
                    snapshot: 0,
                },
            );
            *freed.entry(e.ptr.offset / bucket).or_default() += sectors;
            t.count(
                super::btree_counter_pos(ids::EXTENTS),
                3,
                2,
                -(sectors as i64),
            );
            nr += 1;
        }
        if freed.is_empty() {
            return Ok(());
        }
        let total: u64 = freed.values().sum();

        let allocs: std::collections::BTreeMap<u64, Bkey> = self
            .keys(aids::ALLOC)?
            .into_iter()
            .filter(|k| k.pos.inode == 0 && freed.contains_key(&k.pos.offset))
            .map(|k| (k.pos.offset, k))
            .collect();
        let emptied: Vec<u64> = freed
            .iter()
            .filter(|(b, s)| {
                allocs
                    .get(b)
                    .is_some_and(|a| le64(&a.value, 16) as u32 as u64 == **s)
            })
            .map(|(b, _)| *b)
            .collect();
        // Roots the emptied buckets need, made in buckets taken now.
        let mut need_roots = Vec::new();
        if !emptied.is_empty() {
            for id in [NEED_DISCARD, aids::BUCKET_GENS] {
                if self.root(id).is_err() {
                    need_roots.push(id);
                }
            }
        }
        let mut node_buckets = self.take_buckets(need_roots.len() as u64, t)?;
        for id in need_roots {
            let b = node_buckets.pop().expect("taken");
            self.create_root(id, b, t)?;
        }

        let mut frag_delta = 0i64;
        let mut gens_updates: std::collections::BTreeMap<u64, Vec<(usize, u8)>> =
            std::collections::BTreeMap::new();
        for (&b, &s) in &freed {
            let a = allocs
                .get(&b)
                .ok_or_else(|| Error::Corrupt(format!("bucket {b} has data and no alloc key")))?;
            let mut v = a.value.clone();
            let old = le64(&v, 16) & 0xffff_ffff;
            let new = old
                .checked_sub(s)
                .ok_or_else(|| Error::Corrupt(format!("bucket {b} frees more than it holds")))?;
            if old < bucket {
                t.delete_uncounted(
                    aids::LRU,
                    Bpos {
                        inode: (1 << 61) | ((old << 31) / bucket),
                        offset: b,
                        snapshot: 0,
                    },
                );
            }
            v[16..20].copy_from_slice(&(new as u32).to_le_bytes());
            if new == 0 {
                let w1 = le64(&v, 8);
                let gen = ((w1 >> 32) as u8).wrapping_add(1);
                let flags = (w1 & 0xff) & !0b10;
                let w1 = u64::from(DATA_NEED_DISCARD) << 48
                    | u64::from(gen) << 40
                    | u64::from(gen) << 32
                    | flags;
                v[8..16].copy_from_slice(&w1.to_le_bytes());
                v[48..56].copy_from_slice(&self.journal_seq.to_le_bytes());
                t.put_uncounted(
                    NEED_DISCARD,
                    Bkey {
                        key_type: aids::SET,
                        size: 0,
                        version_hi: 0,
                        version_lo: 0,
                        pos: Bpos {
                            inode: self.journal_seq,
                            offset: b,
                            snapshot: 0,
                        },
                        value: Vec::new(),
                    },
                );
                gens_updates
                    .entry(b >> 8)
                    .or_default()
                    .push(((b & 0xff) as usize, gen));
                frag_delta -= (bucket - old) as i64;
            } else {
                t.put_uncounted(
                    aids::LRU,
                    Bkey {
                        key_type: aids::SET,
                        size: 0,
                        version_hi: 0,
                        version_lo: 0,
                        pos: Bpos {
                            inode: (1 << 61) | ((new << 31) / bucket),
                            offset: b,
                            snapshot: 0,
                        },
                        value: Vec::new(),
                    },
                );
                frag_delta += (old - new) as i64;
            }
            t.put_uncounted(
                aids::ALLOC,
                Bkey {
                    value: v,
                    ..a.clone()
                },
            );
        }
        if !gens_updates.is_empty() {
            let existing = match self.keys(aids::BUCKET_GENS) {
                Err(Error::NotFound(_)) => Vec::new(),
                r => r?,
            };
            for (group, sets) in gens_updates {
                let mut k = existing
                    .iter()
                    .find(|k| {
                        k.key_type == aids::BUCKET_GENS_KEY
                            && k.pos.inode == 0
                            && k.pos.offset == group
                    })
                    .cloned()
                    .unwrap_or(Bkey {
                        key_type: aids::BUCKET_GENS_KEY,
                        size: 0,
                        version_hi: 0,
                        version_lo: 0,
                        pos: Bpos {
                            inode: 0,
                            offset: group,
                            snapshot: 0,
                        },
                        value: vec![0; 256],
                    });
                for (i, g) in sets {
                    k.value[i] = g;
                }
                t.put_uncounted(aids::BUCKET_GENS, k);
            }
        }

        let n_emptied = emptied.len() as i64;
        t.count(acct_replicas_user(0), 1, 0, -(total as i64));
        let user = acct_dev_data_type(0, DATA_USER);
        t.count(user, 3, 0, -n_emptied);
        t.count(user, 3, 1, -(total as i64));
        t.count(user, 3, 2, frag_delta);
        t.count(acct_dev_data_type(0, DATA_NEED_DISCARD), 3, 0, n_emptied);
        let inum = acct_inum(ino);
        t.count(inum, 3, 0, -nr);
        t.count(inum, 3, 1, -(total as i64));
        t.count(inum, 3, 2, -(total as i64));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_positions_are_the_observed_ones() {
        assert_eq!(acct_inum(0x8000_0003).inode, 0x0803_0000_8000_0000);
        assert_eq!(acct_dev_data_type(0, 4).inode, 0x0300_0400_0000_0000);
        assert_eq!(acct_dev_data_type(0, 0).inode, 0x0300_0000_0000_0000);
        assert_eq!(acct_replicas_user(0).inode, 0x0204_0101_0000_0000);
        assert_eq!(acct_replicas(DATA_BTREE, 0).inode, 0x0203_0101_0000_0000);
    }
}
