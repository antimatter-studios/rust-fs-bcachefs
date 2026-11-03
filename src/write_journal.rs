//! Committing through the journal: each transaction is written as one
//! journal entry (`jset`) carrying its keys and every btree root, and the
//! superblock is marked as not cleanly shut down, so the reference
//! replays the entries when it next opens the filesystem -- and so does
//! this crate's reader (src/journal.rs). Btree nodes are not touched.
//!
//! Provenance (docs/clean-room.md, "Committing through the journal"): the
//! jset layout is the one this crate reads and checked against the
//! reference's `list_journal`; that accounting travels as signed deltas
//! applied once by version is from the reference's own journal (S8: the
//! dirty window of aged-unclean); that a journal written this way is
//! replayed and passes the reference checker is judged in the guest.
//!
//! CRASH SAFETY. Data goes to free buckets first, then the jset in one
//! write, then the superblock's clean bit is cleared. Interrupted before the
//! jset, the image is unchanged; before the superblock, the jset is ignored
//! on a clean filesystem; after it, the entry is replayed whole. A reclaim
//! changes only free buckets until the one superblock write that ends it
//! (`Writer::reclaim`), so interrupted before that the journal is replayed
//! as if it had not started.

use std::collections::{BTreeMap, BTreeSet};

use super::{ids, Txn, Writer};
use crate::bkey::Bkey;
use crate::error::{Error, Result};
use crate::journal::{self, Replay};
use crate::util::le64;
use fs_core::BlockDevice;

const JSET_HEADER: usize = 56;
/// Flags of a flush jset: the checksum type, and bit 6, set on every flush
/// entry the reference wrote (S4: 0x41, against 0x61 on a non-flush one).
const JSET_FLAG_BIT6: u32 = 1 << 6;

/// What one entry holds besides a transaction's own keys, at most: the header
/// and every btree's root. Measured with the keys below: on the 32 KiB journal
/// buckets of the write study's base image, a 6 MiB write's entry is 46592
/// bytes for 192 extents, an 8 MiB write's 61440 for 256, a 16 MiB write's
/// 120832 for 512 (#485), and freeing a 4 MiB file's 128 extents 69120 (#486).
const ENTRY_OVERHEAD: usize = 8192;
/// An entry's bytes per extent a write adds, above the 242 measured.
const ENTRY_BYTES_PER_WRITTEN_EXTENT: usize = 320;
/// An entry's bytes per extent a removal frees, above the 540 measured.
const ENTRY_BYTES_PER_FREED_EXTENT: usize = 640;
/// An entry's bytes per emptied bucket made free: its alloc key, the
/// need_discard key gone and a freespace run, each one key.
const ENTRY_BYTES_PER_DISCARDED_BUCKET: usize = 256;

/// The state of a journalled session.
pub(super) struct Session {
    /// The roots every entry records, by btree: level and key.
    pub roots: BTreeMap<u8, (u8, Bkey)>,
    /// Everything committed so far, as a replay over the nodes on disk.
    pub replay: Replay,
    /// The first sequence number this session wrote: every entry needs it.
    pub first_seq: u64,
    pub next_seq: u64,
    /// The journal's buckets in ring order, and where the next entry goes:
    /// an index into them and a byte offset inside the bucket.
    pub buckets: Vec<u64>,
    pub at: (usize, usize),
    /// The bucket holding the oldest entry a replay needs, once there is
    /// one: nothing is reclaimed, so the ring may not come round into it
    /// again (#101).
    pub window_bucket: Option<usize>,
    /// Whether the superblock already says "replay me".
    pub marked: bool,
    /// Test hook: stop each commit after its jset, before the superblock.
    pub crash_before_superblock: bool,
}

impl<D: BlockDevice> Writer<D> {
    /// Switch to committing through the journal. The filesystem must be
    /// clean; from here on nothing but free buckets, the journal and the
    /// superblock's clean bit are written.
    pub fn journal_commits(&mut self) -> Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        if !self.sb.is_clean() {
            return Err(Error::Unsupported(
                "journalled commits start on a clean filesystem".into(),
            ));
        }
        let mut replay_roots = Vec::new();
        for r in self.sb.btree_roots()? {
            let k = crate::bkey::decode(&r.key, &super::UNPACKED)?;
            replay_roots.push((r.btree_id, r.level, k));
        }
        let entries = journal::read_entries(&self.dev, &self.sb)?;
        let newest = entries.iter().map(|j| j.seq).max().unwrap_or(0);
        let first = newest.max(self.journal_seq) + 1;
        let fresh = Replay {
            seq: u64::MAX,
            roots: replay_roots,
            ..Replay::default()
        };
        self.journal_commits_from(fresh, &entries, first, first)?;
        if let Some(s) = self.session.as_mut() {
            s.marked = false;
        }
        Ok(())
    }

    /// Start a session over `replay` (what is already committed): new
    /// entries need everything from `first_seq` and start at `next_seq`.
    fn journal_commits_from(
        &mut self,
        replay: Replay,
        entries: &[journal::Jset],
        first_seq: u64,
        next_seq: u64,
    ) -> Result<()> {
        let roots: BTreeMap<u8, (u8, Bkey)> = replay
            .roots
            .iter()
            .map(|(id, level, k)| (*id, (*level, k.clone())))
            .collect();
        let members = self.sb.members()?;
        let bucket_bytes = members
            .first()
            .map(|m| u64::from(m.bucket_size) * 512)
            .ok_or_else(|| Error::Corrupt("no member device".into()))?
            as usize;
        let mut buckets = Vec::new();
        let f = self
            .sb
            .field(journal::FIELD_JOURNAL_V2)
            .ok_or_else(|| Error::Unsupported("no journal_v2 field".into()))?;
        for c in f.body.chunks_exact(16) {
            let (start, nr) = (le64(c, 0), le64(c, 8));
            buckets.extend(start..start + nr);
        }
        if buckets.is_empty() {
            return Err(Error::Corrupt("the journal has no buckets".into()));
        }
        // After the newest entry there is, in its bucket or the next.
        let block = (self.sb.block_size as usize * 512).max(512);
        let at = match entries.iter().max_by_key(|j| j.seq) {
            Some(j) => {
                let byte = j.sector * 512;
                let idx = buckets
                    .iter()
                    .position(|&b| byte / bucket_bytes as u64 == b)
                    .ok_or_else(|| {
                        Error::Corrupt("the newest jset is outside the journal".into())
                    })?;
                let off = (byte % bucket_bytes as u64) as usize + j.bytes.div_ceil(block) * block;
                (idx, off)
            }
            None => (0, 0),
        };
        let window_bucket = entries
            .iter()
            .filter(|j| j.seq >= first_seq)
            .min_by_key(|j| j.seq)
            .and_then(|j| {
                let bucket = j.sector * 512 / bucket_bytes as u64;
                buckets.iter().position(|&b| b == bucket)
            });
        self.session = Some(Session {
            roots,
            replay,
            first_seq,
            next_seq,
            buckets,
            at,
            window_bucket,
            // An unclean filesystem already says "replay me".
            marked: true,
            crash_before_superblock: false,
        });
        Ok(())
    }

    /// A writer that commits through the journal, on a clean filesystem
    /// ([`Writer::journal_commits`]) or on one whose journal holds entries
    /// not yet replayed -- this writer's own from an earlier session, or
    /// the reference's: the new entries continue that journal, so one
    /// replay applies them all.
    pub fn open_journalled(dev: D) -> Result<Self> {
        let sb = crate::superblock::Superblock::read(&dev)?;
        if sb.is_clean() {
            let mut w = Self::open(dev)?;
            w.journal_commits()?;
            return Ok(w);
        }
        let entries = journal::read_entries(&dev, &sb)?;
        let rp = journal::replay(&dev, &sb)?;
        let newest = entries
            .iter()
            .rev()
            .find(|j| j.flush)
            .ok_or_else(|| Error::Corrupt("the journal holds no flush entry".into()))?;
        if entries.iter().any(|j| j.seq > newest.seq) {
            // Entries after the last flush are not committed; a flush entry
            // written after them would commit them.
            return Err(Error::Unsupported(
                "the journal ends in entries that were never flushed".into(),
            ));
        }
        let (window_start, newest_seq) = (newest.last_seq, newest.seq);
        let mut w = Self::open_any(dev, sb)?;
        w.journal_seq = newest_seq;
        w.journal_commits_from(rp, &entries, window_start, newest_seq + 1)?;
        Ok(w)
    }

    /// Write one transaction as a journal entry.
    pub(super) fn commit_journal(&mut self, t: Txn) -> Result<()> {
        // Kept whole in case the journal is full: it is then reclaimed and
        // this transaction goes into the fresh session's first entry.
        let retry = Txn {
            keys: t.keys.clone(),
            acct: t.acct.clone(),
            inflight: None,
        };
        let seq = self.session.as_ref().expect("journalled").next_seq;
        // Accounting as deltas, each with a version above any it adds to:
        // this entry's sequence number, then its place in the entry.
        let mut acct = Vec::new();
        for (i, (&at, d)) in t.acct.iter().enumerate() {
            if d.iter().all(|&x| x == 0) {
                continue;
            }
            let mut value = Vec::with_capacity(d.len() * 8);
            for &x in d {
                value.extend_from_slice(&(x as u64).to_le_bytes());
            }
            acct.push(Bkey {
                key_type: ids::ACCOUNTING_KEY,
                size: 0,
                // The journal sequence, then the delta's index below it.
                version_hi: (seq >> 32) as u32,
                version_lo: (seq << 32) | i as u64,
                pos: at,
                value,
            });
        }
        let mut keys: BTreeMap<u8, Vec<Bkey>> = t.keys;
        if !acct.is_empty() {
            keys.entry(ids::ACCOUNTING).or_default().extend(acct);
        }

        let csum_type = match self.sb.metadata_checksum_opt() {
            0 => 0u8,
            1 => 1,
            2 => 2,
            3 => 7,
            o => return Err(Error::Unsupported(format!("metadata checksum option {o}"))),
        };
        let s = self.session.as_ref().expect("journalled");
        let mut body = Vec::new();
        for (&id, ks) in &keys {
            let mut entry_keys = Vec::new();
            for k in ks {
                entry_keys.extend(super::encode_key(k)?);
            }
            for chunk in entry_keys.chunks(0xffff * 8) {
                push_entry(&mut body, journal::entry_type::BTREE_KEYS, id, 0, chunk);
            }
        }
        for (&id, (level, k)) in &s.roots {
            push_entry(
                &mut body,
                journal::entry_type::BTREE_ROOT,
                id,
                *level,
                &super::encode_key(k)?,
            );
        }
        let mut jset = vec![0u8; JSET_HEADER];
        jset[16..24].copy_from_slice(&journal::jset_magic(&self.sb.uuid).to_le_bytes());
        jset[24..32].copy_from_slice(&seq.to_le_bytes());
        jset[32..36].copy_from_slice(&u32::from(self.sb.version.0).to_le_bytes());
        jset[36..40].copy_from_slice(&(u32::from(csum_type) | JSET_FLAG_BIT6).to_le_bytes());
        let u64s = u32::try_from(body.len() / 8)
            .map_err(|_| Error::Unsupported("a journal entry too large".into()))?;
        jset[40..44].copy_from_slice(&u64s.to_le_bytes());
        jset[48..56].copy_from_slice(&s.first_seq.to_le_bytes());
        jset.extend_from_slice(&body);
        let csum = crate::csum::compute(csum_type, &jset[16..])?;
        jset[0..8].copy_from_slice(&csum.to_le_bytes());

        let block = (self.sb.block_size as usize * 512).max(512);
        let bucket_bytes = self.bucket_sectors()? as usize * 512;
        let len = jset.len().div_ceil(block) * block;
        if len > bucket_bytes {
            return Err(Error::Unsupported(format!(
                "a journal entry of {len} bytes does not fit a {bucket_bytes}-byte journal bucket"
            )));
        }
        jset.resize(len, 0);
        let (mut idx, mut off) = s.at;
        if off + len > bucket_bytes {
            idx = (idx + 1) % s.buckets.len();
            off = 0;
            // Every entry from the window's first on is still needed, and
            // this writer reclaims none: going on into the bucket that
            // holds the first would write over it (#101). Nor may the window
            // take the whole ring: a replay needs journal space of its own
            // to run in (#486).
            let n = s.buckets.len();
            let held = s.window_bucket.map(|w| (idx + n - w) % n + 1);
            if s.window_bucket == Some(idx) || held.is_some_and(|held| held > window_limit(n)) {
                let took = self.reclaim()?;
                let mut retry = retry;
                without_buckets(&mut retry, &took);
                return self.commit_journal(retry);
            }
        }
        let where_ = s.buckets[idx] * bucket_bytes as u64 + off as u64;
        self.dev.write_at(where_, &jset)?;
        self.dev.flush()?;

        let s = self.session.as_mut().expect("journalled");
        if s.window_bucket.is_none() {
            s.window_bucket = Some(idx);
        }
        s.at = (idx, off + len);
        s.next_seq = seq + 1;
        for (id, ks) in keys {
            s.replay.keys.entry((id, 0)).or_default().extend(ks);
        }
        if s.crash_before_superblock {
            return Ok(());
        }
        if !s.marked {
            s.marked = true;
            self.mark_unclean()?;
        }
        Ok(())
    }

    /// Reclaim the whole journal (#113, S1 9.7.2-9.7.3): every key the
    /// session's replay holds -- what this session committed, and what a
    /// continued journal held before it -- goes into the btrees, the
    /// accounting deltas summed onto the accounting keys; one superblock
    /// write then names the new roots and marks the filesystem clean at the
    /// newest sequence, so no entry is needed any more, and a fresh session
    /// starts after it.
    ///
    /// Returns the buckets the reclaim's nodes took, which the transaction
    /// waiting for it may show as still free.
    ///
    /// Nothing the journal's roots reach is changed (#486). Any node the keys
    /// reach is written again in a free bucket, as a full node is, and only
    /// those new nodes take further keys. So until the superblock write the
    /// image is what it was: free buckets aside, a stop at any point leaves
    /// the old nodes and the whole journal, and a replay as if nothing had
    /// started. No new node may take a bucket the replay's alloc keys name,
    /// whichever writer took it: the nodes do not show that yet, and it may
    /// hold the data an extent points at. After a stop this writer commits
    /// nothing more, because the buckets it has taken are unaccounted.
    fn reclaim(&mut self) -> Result<BTreeSet<u64>> {
        let free = self.free_buckets()?;
        let s = self.session.take().expect("journalled");
        let refuse = |w: &mut Self, s: Session, why: String| -> Result<BTreeSet<u64>> {
            let first = s.first_seq;
            w.session = Some(s);
            Err(Error::Unsupported(format!(
                "the journal is full: its buckets hold entries a replay still needs, from \
                 sequence {first} on, and they cannot be reclaimed: {why}; replay the journal \
                 before writing more"
            )))
        };
        let pending = match pending_from(&s.replay) {
            Ok(t) => t,
            Err(why) => return refuse(self, s, why),
        };
        let needed = self.reclaim_needs(&pending);
        if free < needed {
            return refuse(
                self,
                s,
                format!("{needed} free buckets are needed for its nodes and only {free} are free"),
            );
        }
        // Roots the session moved must be where the superblock names them.
        let on_disk: Result<BTreeMap<u8, (u8, Bkey)>> = self.sb.btree_roots().and_then(|rs| {
            rs.into_iter()
                .map(|r| {
                    Ok((
                        r.btree_id,
                        (r.level, crate::bkey::decode(&r.key, &super::UNPACKED)?),
                    ))
                })
                .collect()
        });
        let on_disk = match on_disk {
            Ok(r) => r,
            Err(e) => return refuse(self, s, e.to_string()),
        };
        let moved: Vec<(u8, u8, Bkey)> = s
            .roots
            .iter()
            .filter(|(id, root)| on_disk.get(id) != Some(root))
            .map(|(&id, (level, k))| (id, *level, k.clone()))
            .collect();
        for (id, level, k) in moved {
            if let Err(e) = self.set_root(id, level, &k) {
                return refuse(self, s, format!("btree {id}'s root: {e}"));
            }
        }
        let newest = s.next_seq.saturating_sub(1);
        let (journal_seq, sb_raw) = (self.journal_seq, self.sb_raw.clone());
        self.journal_seq = newest;
        let held = self.reserved.clone();
        self.reserved.extend(
            pending
                .keys
                .get(&super::alloc::aids::ALLOC)
                .into_iter()
                .flatten()
                .filter(|k| k.pos.inode == 0)
                .map(|k| k.pos.offset),
        );
        // The in-place path reads the btrees from their nodes, which it does
        // only on a clean filesystem (CI run 37925455364: "its journal must
        // be replayed first"). The clean bit is set in memory for it, and on
        // disk by mark_clean with everything else.
        let f = le64(&self.sb_raw, 0x90) | 0b10;
        self.sb_raw[0x90..0x98].copy_from_slice(&f.to_le_bytes());
        self.sb = crate::superblock::Superblock::parse_unchecked(&self.sb_raw)?;
        self.reclaiming = Some(BTreeSet::new());
        let in_nodes = self.commit_in_place(pending);
        let took = self.reclaiming.take().unwrap_or_default();
        if let Err(e) = in_nodes.and_then(|()| self.mark_clean(newest)) {
            self.sb_raw = sb_raw;
            self.sb = crate::superblock::Superblock::parse_unchecked(&self.sb_raw)?;
            self.journal_seq = journal_seq;
            self.session = Some(s);
            self.reclaim_failed = Some(e.to_string());
            return Err(e);
        }
        // What the nodes now show needs no holding back; the buckets the
        // nodes took do, for the transaction this reclaim was for.
        self.reserved = held;
        self.reserved.extend(took.iter().copied());
        self.journal_commits()?;
        Ok(took)
    }

    /// Free buckets, as this session sees them.
    fn free_buckets(&self) -> Result<u64> {
        let mut free = 0;
        for run in self.keys(super::alloc::aids::FREESPACE)? {
            if run.key_type == super::alloc::aids::SET && run.pos.inode == 0 {
                let start = run.pos.offset.saturating_sub(u64::from(run.size));
                free += (start..run.pos.offset)
                    .filter(|b| !self.reserved.contains(b))
                    .count() as u64;
            }
        }
        Ok(free)
    }

    /// How much of a node a reclaim's keys may fill: as `rewrite_node` fills
    /// one, 2/3 of it past the header.
    fn node_room(&self) -> u64 {
        (u64::from(self.sb.btree_node_size()) * 512).saturating_sub(160) * 2 / 3
    }

    /// The buckets the nodes of a reclaim of `pending` take: its keys laid
    /// into nodes as `rewrite_node` lays them, and one more per btree for
    /// the part node or new root that does not fill.
    fn reclaim_needs(&self, pending: &Txn) -> u64 {
        let room = self.node_room().max(1);
        pending
            .keys
            .values()
            .filter(|ks| !ks.is_empty())
            .map(|ks| {
                let bytes: u64 = ks.iter().map(|k| 40 + k.value.len() as u64).sum();
                bytes.div_ceil(room) + 1
            })
            .sum()
    }

    /// The buckets data leaves free for a reclaim of the whole journal, past
    /// `NODE_RESERVE`: without a session, none.
    pub(super) fn reclaim_reserve(&self) -> Result<u64> {
        let Some(s) = &self.session else {
            return Ok(0);
        };
        let journal = s.buckets.len() as u64 * self.bucket_sectors()? * 512;
        Ok(journal.div_ceil(self.node_room().max(1)))
    }

    /// How many buckets to make free in one commit: as many as one journal
    /// entry holds, in a session; all of them without one.
    pub(super) fn discard_batch(&self) -> Result<usize> {
        if self.session.is_none() {
            return Ok(usize::MAX);
        }
        Ok(self.entry_room()? / ENTRY_BYTES_PER_DISCARDED_BUCKET)
    }

    /// What one journal entry has room for besides what every entry holds,
    /// never less than one key's share of it.
    fn entry_room(&self) -> Result<usize> {
        let bucket = self.bucket_sectors()? as usize * 512;
        Ok(bucket
            .saturating_sub(ENTRY_OVERHEAD)
            .max(ENTRY_BYTES_PER_FREED_EXTENT))
    }

    /// The most bytes one [`Writer::write_at`] of new data into a journalled
    /// file is sure to fit one journal entry (#485), a whole number of
    /// extents, so whole blocks. A longer write is refused: write it in
    /// runs of this length, each committed whole.
    pub fn largest_journalled_write(&self) -> Result<usize> {
        let bucket = self.bucket_sectors()?;
        let extent = super::alloc::MAX_EXTENT_SECTORS.min(bucket) as usize * 512;
        Ok(extent * (self.entry_room()? / ENTRY_BYTES_PER_WRITTEN_EXTENT).max(1))
    }

    /// Remove the file `name` from `parent` however large it is (#486): its
    /// extents go from the end, as many in each commit as one journal entry
    /// holds, and its name and inode in the last. Each commit is whole, but
    /// a stop part way leaves the file shorter under its name, so this is for
    /// a file nothing relies on yet, such as a staged import; a file another
    /// name links keeps its data, and only that name goes.
    pub fn unlink_in_steps(&mut self, parent: u64, name: &[u8]) -> Result<()> {
        let p = self.dir(parent)?;
        let (_, dirent, _) = self.dirent(&p, name)?;
        let dirent = dirent
            .ok_or_else(|| Error::NotFound(format!("{:?}", String::from_utf8_lossy(name))))?;
        let ino = crate::inode::Dirent::from_key(&dirent)?.inum;
        let linked = self.inode(ino)?.raw.varints[super::field::NLINK] > 0;
        let step = (self.entry_room()? / ENTRY_BYTES_PER_FREED_EXTENT).max(1);
        loop {
            use crate::bkey::key_type::{EXTENT_WHITEOUT, WHITEOUT};
            let ends: Vec<u64> = self
                .extents_of(ino)?
                .iter()
                .filter(|k| k.size > 0 && !matches!(k.key_type, WHITEOUT | EXTENT_WHITEOUT))
                .map(|k| k.pos.offset)
                .collect();
            if linked || ends.len() <= step {
                break;
            }
            // Space the last step freed comes back, so a reclaim on the way
            // always has buckets for its nodes.
            self.discard_freed()?;
            self.truncate(ino, ends[ends.len() - step - 1] * 512)?;
        }
        self.discard_freed()?;
        self.unlink(parent, name)
    }

    /// Set the superblock's clean bit and its clean field's journal
    /// sequence, and write every copy.
    fn mark_clean(&mut self, seq: u64) -> Result<()> {
        let mut p = crate::superblock::SB_HEADER_BYTES;
        let mut found = false;
        while p + 8 <= self.sb_raw.len() {
            let u64s = crate::util::le32(&self.sb_raw, p) as usize;
            let ty = crate::util::le32(&self.sb_raw, p + 4);
            if u64s == 0 || p + u64s * 8 > self.sb_raw.len() {
                break;
            }
            if ty == crate::superblock::FIELD_CLEAN && u64s >= 3 {
                // The field's body: flags u32, two u16 clocks, journal_seq.
                self.sb_raw[p + 16..p + 24].copy_from_slice(&seq.to_le_bytes());
                found = true;
            }
            p += u64s * 8;
        }
        if !found {
            return Err(Error::Corrupt("no clean field to mark".into()));
        }
        let f = le64(&self.sb_raw, 0x90) | 0b10;
        self.sb_raw[0x90..0x98].copy_from_slice(&f.to_le_bytes());
        self.sb = crate::superblock::Superblock::parse_unchecked(&self.sb_raw)?;
        self.write_superblock()
    }

    /// Clear the superblock's clean bit (flags[0] bit 1, as the reference
    /// leaves an image it did not shut down) and write every copy.
    fn mark_unclean(&mut self) -> Result<()> {
        self.ensure_journal_replicas()?;
        let f = le64(&self.sb_raw, 0x90) & !0b10;
        self.sb_raw[0x90..0x98].copy_from_slice(&f.to_le_bytes());
        self.sb = crate::superblock::Superblock::parse_unchecked(&self.sb_raw)?;
        self.write_superblock()
    }

    /// The superblock's replicas_v0 field (type 3) must list the journal on
    /// device 0 before an entry on it is replayed: entries are data type,
    /// device count, devices, packed and zero-padded (S4: `03 01 00` on a
    /// clean image, `03 01 00 02 01 00` on the aged one the reference left
    /// unclean; the checker's "superblock not marked as containing replicas
    /// for journal entry").
    fn ensure_journal_replicas(&mut self) -> Result<()> {
        const FIELD_REPLICAS_V0: u32 = 3;
        const JOURNAL: u8 = 2;
        let mut p = super::SB_HEADER_BYTES;
        while p + 8 <= self.sb_raw.len() {
            let u64s = crate::util::le32(&self.sb_raw, p) as usize;
            let ty = crate::util::le32(&self.sb_raw, p + 4);
            if u64s == 0 {
                break;
            }
            let end = p + u64s * 8;
            if ty == FIELD_REPLICAS_V0 {
                let body = p + 8;
                let mut q = body;
                while q < end && self.sb_raw[q] != 0 {
                    let (dt, n) = (self.sb_raw[q], self.sb_raw[q + 1] as usize);
                    if dt == JOURNAL && n == 1 && self.sb_raw[q + 2] == 0 {
                        return Ok(());
                    }
                    q += 2 + n;
                }
                let entry = [JOURNAL, 1, 0];
                if q + entry.len() > end {
                    self.sb_raw.splice(end..end, [0u8; 8]);
                    self.sb_raw[p..p + 4].copy_from_slice(&((u64s + 1) as u32).to_le_bytes());
                    let total = crate::util::le32(&self.sb_raw, 0x7c) + 1;
                    self.sb_raw[0x7c..0x80].copy_from_slice(&total.to_le_bytes());
                }
                self.sb_raw[q..q + entry.len()].copy_from_slice(&entry);
                self.sb = crate::superblock::Superblock::parse_unchecked(&self.sb_raw)?;
                return Ok(());
            }
            p = end;
        }
        Err(Error::Unsupported("no replicas_v0 field".into()))
    }

    /// Test hook: from now on, each commit stops after writing its jset, as
    /// if the machine stopped before the superblock was written.
    #[doc(hidden)]
    pub fn crash_before_superblock(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.crash_before_superblock = true;
        }
    }

    /// Record a new root in the session (journalled mode).
    pub(super) fn session_add_root(&mut self, id: u8, level: u8, key: Bkey) {
        if let Some(s) = self.session.as_mut() {
            s.replay.roots.retain(|(b, _, _)| *b != id);
            s.replay.roots.push((id, level, key.clone()));
            s.roots.insert(id, (level, key));
        }
    }
}

/// The most of a journal of `n` buckets a replay window may hold: a quarter
/// of them, and at least two, stay free.
fn window_limit(n: usize) -> usize {
    n.saturating_sub((n / 4).max(2)).max(1)
}

/// `t`'s freespace keys with the buckets `took` gone from them, as a reclaim
/// took them while `t` waited for it (#486). A run only ever shrinks from its
/// start, so those are the buckets just before what `t` left of the run; left
/// as they were, its key would put them back in the run.
fn without_buckets(t: &mut Txn, took: &BTreeSet<u64>) {
    let Some(runs) = t.keys.get_mut(&super::alloc::aids::FREESPACE) else {
        return;
    };
    for run in runs
        .iter_mut()
        .filter(|k| k.key_type == super::alloc::aids::SET)
    {
        let end = run.pos.offset;
        let mut start = end.saturating_sub(u64::from(run.size));
        while start < end && took.contains(&start) {
            start += 1;
        }
        run.size = (end - start) as u32;
        if start == end {
            run.key_type = crate::bkey::key_type::DELETED;
        }
    }
}

/// The transaction a replay amounts to: its leaf keys in order, and its
/// accounting keys' deltas summed per position (#113). Interior node keys
/// cannot be written this way.
fn pending_from(replay: &Replay) -> std::result::Result<Txn, String> {
    let mut pending = Txn::default();
    for (&(id, level), ks) in &replay.keys {
        if ks.is_empty() {
            continue;
        }
        if level != 0 {
            return Err(format!("it holds interior node keys of btree {id}"));
        }
        for k in ks {
            if id == ids::ACCOUNTING && k.key_type == ids::ACCOUNTING_KEY {
                let n = k.value.len() / 8;
                let d = pending.acct.entry(k.pos).or_insert_with(|| vec![0; n]);
                if d.len() < n {
                    d.resize(n, 0);
                }
                for (i, c) in k.value.chunks_exact(8).enumerate() {
                    d[i] = d[i].wrapping_add(le64(c, 0) as i64);
                }
            } else {
                pending.keys.entry(id).or_default().push(k.clone());
            }
        }
    }
    Ok(pending)
}

fn push_entry(out: &mut Vec<u8>, ty: u8, btree: u8, level: u8, payload: &[u8]) {
    out.extend_from_slice(&((payload.len() / 8) as u16).to_le_bytes());
    out.extend_from_slice(&[btree, level, ty, 0, 0, 0]);
    out.extend_from_slice(payload);
}
