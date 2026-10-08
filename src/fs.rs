//! A read-only view of a single-device bcachefs filesystem: look up a
//! path, list a directory, read a file.
//!
//! Nothing is loaded at open but the superblock (and a journal replay when
//! one is needed): every lookup, listing and read seeks a btree cursor to
//! its own keys and reads only the nodes on their path.
//!
//! Snapshots are not read. Every key this reader uses must carry the
//! snapshot the root inode carries (U32_MAX on every fixture; inferred,
//! docs/clean-room.md): a key at any other snapshot means a snapshot or
//! subvolume exists, whose visibility rules (S1 9.4) are not implemented,
//! so it is refused rather than misread.

use crate::bkey::{Bkey, Bpos};
use crate::btree::{btree_id, Cursor};
use crate::error::{Error, Result};
use crate::extent::{compression, DataExtent};
use crate::inode::{Dirent, Inode, ROOT_INO};
use crate::journal::Replay;
use crate::superblock::Superblock;
use fs_core::BlockRead;

pub struct Filesystem<D: BlockRead> {
    dev: D,
    sb: Superblock,
    /// What a replay of the journal adds, when the filesystem was not shut
    /// down cleanly.
    replay: Option<Replay>,
    /// The nodes lookups pass through, read once.
    cache: crate::btree::NodeCache,
    /// The one snapshot every key is read at: the root inode's.
    snapshot: u32,
    /// This device's index and bucket size (sectors), for judging pointers.
    dev_idx: u8,
    bucket_size: u64,
}

impl<D: BlockRead> Filesystem<D> {
    pub fn open(dev: D) -> Result<Self> {
        let sb = Superblock::read(&dev)?;
        if sb.is_encrypted() {
            // Its btree nodes and data are encrypted; how the keys and
            // nonces are derived has no clean-room source yet.
            return Err(Error::Unsupported(
                "the filesystem is encrypted: encrypted filesystems are not read".into(),
            ));
        }
        if sb.nr_devices != 1 {
            return Err(Error::Unsupported(format!(
                "{} devices: only single-device filesystems are read",
                sb.nr_devices
            )));
        }
        // The roots in the superblock are as of the last clean shutdown;
        // after an unclean one everything since is in the journal, which
        // is replayed here, in memory.
        let replay = if sb.is_clean() {
            None
        } else {
            Some(crate::journal::replay(&dev, &sb)?)
        };
        // The root inode must be there: a filesystem without it is not
        // one to read. Its snapshot is the one every other key must carry.
        let member = sb
            .members()?
            .into_iter()
            .nth(sb.dev_idx as usize)
            .ok_or_else(|| Error::Corrupt("no member entry for this device".into()))?;
        if member.bucket_size == 0 {
            return Err(Error::Corrupt("bucket size 0".into()));
        }
        let mut fs = Filesystem {
            dev_idx: sb.dev_idx,
            bucket_size: u64::from(member.bucket_size),
            dev,
            sb,
            replay,
            cache: Default::default(),
            snapshot: u32::MAX,
        };
        fs.snapshot = fs.root_snapshot()?;
        Ok(fs)
    }

    /// The snapshot of the root inode's key, and a refusal when the root
    /// exists at more than one.
    fn root_snapshot(&self) -> Result<u32> {
        let mut c = self.cursor(btree_id::INODES)?;
        c.seek(Bpos {
            inode: 0,
            offset: ROOT_INO,
            snapshot: 0,
        })?;
        let mut found: Option<u32> = None;
        while let Some(k) = c.next_key()? {
            if k.pos.inode != 0 || k.pos.offset != ROOT_INO {
                break;
            }
            if k.key_type != crate::bkey::key_type::INODE_V3 {
                continue;
            }
            if let Some(first) = found {
                return Err(snapshots_not_read(&k, first));
            }
            found = Some(k.pos.snapshot);
        }
        found.ok_or_else(|| Error::NotFound(format!("inode {ROOT_INO}")))
    }

    /// A key at any snapshot but the root's is a snapshot this reader
    /// cannot resolve.
    fn same_snapshot(&self, k: &Bkey) -> Result<()> {
        if k.pos.snapshot == self.snapshot {
            Ok(())
        } else {
            Err(snapshots_not_read(k, self.snapshot))
        }
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    fn cursor(&self, id: u8) -> Result<Cursor<'_>> {
        Ok(Cursor::new(&self.dev, &self.sb, id, self.replay.as_ref())?.with_cache(&self.cache))
    }

    /// Every key of btree `id` whose position's inode field is `inode`, in
    /// order, read through a cursor.
    fn keys_of(&self, id: u8, inode: u64) -> Result<Vec<crate::bkey::Bkey>> {
        let mut c = self.cursor(id)?;
        c.seek(Bpos {
            inode,
            offset: 0,
            snapshot: 0,
        })?;
        let mut out = Vec::new();
        while let Some(k) = c.next_key()? {
            if k.pos.inode != inode {
                break;
            }
            self.same_snapshot(&k)?;
            out.push(k);
        }
        Ok(out)
    }

    pub fn inode(&self, ino: u64) -> Result<Inode> {
        let mut c = self.cursor(btree_id::INODES)?;
        c.seek(Bpos {
            inode: 0,
            offset: ino,
            snapshot: 0,
        })?;
        let mut found: Option<Inode> = None;
        while let Some(k) = c.next_key()? {
            if k.pos.inode != 0 || k.pos.offset != ino {
                break;
            }
            self.same_snapshot(&k)?;
            match k.key_type {
                crate::bkey::key_type::INODE_V3 if found.is_none() => {
                    found = Some(Inode::from_key(&k)?);
                }
                // The earlier encodings (S1 11.5, 11.6: `inode` is v1,
                // `inode_v2` is 0.18 to 0.22): a filesystem older than this
                // reader decodes, which is not the same as no inode.
                crate::bkey::key_type::INODE | crate::bkey::key_type::INODE_V2 => {
                    return Err(Error::Unsupported(format!(
                        "inode {ino} is stored in the {} encoding (key type {}), older than \
                         the inode_v3 this reader decodes",
                        if k.key_type == crate::bkey::key_type::INODE {
                            "inode (v1)"
                        } else {
                            "inode_v2"
                        },
                        k.key_type
                    )));
                }
                _ => {}
            }
        }
        found.ok_or_else(|| Error::NotFound(format!("inode {ino}")))
    }

    /// The entries of a directory, by inode, in btree (hash) order.
    pub fn readdir(&self, dir: u64) -> Result<Vec<Dirent>> {
        if !self.inode(dir)?.is_dir() {
            return Err(Error::Corrupt(format!("inode {dir} is not a directory")));
        }
        self.keys_of(btree_id::DIRENTS, dir)?
            .iter()
            .filter(|k| k.key_type == crate::bkey::key_type::DIRENT)
            .map(Dirent::from_key)
            .collect()
    }

    /// The entry named `name` in directory `dir`: from its name's hash slot
    /// up, past slots holding other names or `hash_whiteout`s, until the
    /// name or an empty slot (S8: names that collide take the next free
    /// offset, and a removal inside a run leaves a whiteout). A directory
    /// whose hash is not known (crc64, never seen in a fixture) is scanned.
    ///
    /// A casefolded directory places and finds an entry by its folded name
    /// (S1 2.7; S4: the slot of every entry of the `casefold` set is the
    /// SipHash of its folded name), as the reference mount finds
    /// `HELLO.txt` as `Hello.TXT`. An ASCII name folds to its lowercase
    /// (S3: every ASCII name of the set). Folding any other name takes
    /// Unicode's tables, which this reader does not carry, so such a name
    /// is matched as stored, by a scan.
    fn find(&self, dir: &Inode, name: &[u8]) -> Result<Option<Dirent>> {
        let casefolded = dir.is_casefolded();
        if casefolded && !name.is_ascii() {
            return Ok(self.readdir(dir.ino)?.into_iter().find(|d| d.name == name));
        }
        let key = if casefolded {
            name.to_ascii_lowercase()
        } else {
            name.to_vec()
        };
        let found = |k: &Bkey| -> Result<Option<Dirent>> {
            let d = Dirent::from_key(k)?;
            let hit = match crate::inode::dirent_folded_name(k)? {
                Some(f) if casefolded => f == key,
                _ => d.name == name,
            };
            Ok(hit.then_some(d))
        };
        let Some(mut at) = crate::inode::name_hash(dir.hash_type(), dir.hash_seed, &key) else {
            let keys = self.keys_of(btree_id::DIRENTS, dir.ino)?;
            for k in keys.iter().filter(|k| k.key_type == crate::bkey::key_type::DIRENT) {
                if let Some(d) = found(k)? {
                    return Ok(Some(d));
                }
            }
            return Ok(None);
        };
        let mut c = self.cursor(btree_id::DIRENTS)?;
        c.seek(Bpos {
            inode: dir.ino,
            offset: at,
            snapshot: 0,
        })?;
        while let Some(k) = c.next_key()? {
            if k.pos.inode != dir.ino || k.pos.offset != at {
                break;
            }
            self.same_snapshot(&k)?;
            match k.key_type {
                crate::bkey::key_type::DIRENT => {
                    if let Some(d) = found(&k)? {
                        return Ok(Some(d));
                    }
                }
                crate::bkey::key_type::HASH_WHITEOUT => {}
                _ => break,
            }
            at += 1;
        }
        Ok(None)
    }

    /// Resolve an absolute path to an inode, without following a symlink
    /// in the last component.
    pub fn lookup(&self, path: &str) -> Result<u64> {
        let mut ino = ROOT_INO;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let dir = self.inode(ino)?;
            if !dir.is_dir() {
                return Err(Error::NotFound(path.to_string()));
            }
            ino = self
                .find(&dir, part.as_bytes())?
                .ok_or_else(|| Error::NotFound(path.to_string()))?
                .inum;
        }
        Ok(ino)
    }

    /// The extended attributes of an inode, in btree order.
    pub fn xattrs(&self, ino: u64) -> Result<Vec<crate::xattr::Xattr>> {
        self.inode(ino)?;
        self.keys_of(btree_id::XATTRS, ino)?
            .iter()
            .filter(|k| k.key_type == crate::bkey::key_type::XATTR)
            .map(crate::xattr::Xattr::from_key)
            .collect()
    }

    /// The whole contents of a file (or a symlink's target).
    pub fn read(&self, ino: u64) -> Result<Vec<u8>> {
        let size = self.inode(ino)?.size;
        let len = usize::try_from(size)
            .map_err(|_| Error::Unsupported("file larger than memory".into()))?;
        self.read_range(ino, 0, len)
    }

    /// Up to `len` bytes of a file from byte `offset`: empty at or past the
    /// end, shorter than `len` across it. Only the extents covering the
    /// window are read and decoded, so a reader taking a file in pieces
    /// does work proportional to the pieces, not to the file.
    pub fn read_range(&self, ino: u64, offset: u64, len: usize) -> Result<Vec<u8>> {
        let size = self.inode(ino)?.size;
        if offset >= size {
            return Ok(Vec::new());
        }
        let end = size.min(offset.saturating_add(len as u64));
        let mut out = vec![0u8; (end - offset) as usize];
        // An extent key sits at its END (S1 11.4), so the first key that can
        // cover byte `offset` is the first whose position is past its sector.
        let mut c = self.cursor(btree_id::EXTENTS)?;
        c.seek(Bpos {
            inode: ino,
            offset: offset / 512 + 1,
            snapshot: 0,
        })?;
        // Extents of one file must not overlap (S1's check_extents: "no
        // overlaps"): two keys claiming the same sectors would be written
        // into the buffer in position order, and the later one, not the
        // newer one, would win. Refused instead (issue #60).
        let mut prev_end: Option<u64> = None;
        while let Some(k) = c.next_key()? {
            if k.pos.inode != ino {
                break;
            }
            self.same_snapshot(&k)?;
            let key_start = k.start_offset().saturating_mul(512);
            if key_start >= end {
                break;
            }
            if matches!(
                k.key_type,
                crate::bkey::key_type::EXTENT
                    | crate::bkey::key_type::INLINE_DATA
                    | crate::bkey::key_type::RESERVATION
                    | crate::bkey::key_type::ERROR
                    | crate::bkey::key_type::REFLINK_P
            ) {
                if let Some(prev) = prev_end {
                    if k.start_offset() < prev {
                        return Err(Error::Corrupt(format!(
                            "extent {} starts at sector {} before the extent before it ends at \
                             {prev}: overlapping extents, check the filesystem",
                            k.pos,
                            k.start_offset()
                        )));
                    }
                }
                prev_end = Some(k.pos.offset);
            }
            match k.key_type {
                crate::bkey::key_type::EXTENT => {
                    let e = DataExtent::from_key(&k)?;
                    let data = self.extent_data(&e)?;
                    copy_window(&mut out, offset, end, e.file_start * 512, &data);
                }
                crate::bkey::key_type::INLINE_DATA => {
                    // The value is the data itself, zero-padded to a whole
                    // u64, and the key covers `size` sectors ending at its
                    // position like any extent (S3: the lister prints the
                    // bytes and a `datalen` equal to the value length; S4:
                    // the bytes match the file the mount wrote).
                    let n = k.value.len().min(k.size as usize * 512);
                    copy_window(&mut out, offset, end, key_start, &k.value[..n]);
                }
                // A reservation is space with no data yet, a whiteout hides
                // nothing on a filesystem without snapshots: both read as
                // zeros.
                crate::bkey::key_type::RESERVATION
                | crate::bkey::key_type::WHITEOUT
                | crate::bkey::key_type::EXTENT_WHITEOUT => {}
                // "Reads to these ranges return IO errors" (S1 9.1.2.1).
                crate::bkey::key_type::ERROR => {
                    return Err(Error::Io(format!(
                        "inode {ino}, sectors {}..{}: the data was permanently lost (an `error` \
                         extent)",
                        k.start_offset(),
                        k.pos.offset
                    )))
                }
                // Reflinked data is in the reflink btree, behind a pointer
                // whose layout no reference image has shown (S1 9.1.6; open
                // question 6): refused by name rather than misread (#7).
                crate::bkey::key_type::REFLINK_P => {
                    return Err(Error::Unsupported(format!(
                        "inode {ino}, sectors {}..{}: reflinked data (a `reflink_p` into the \
                         reflink btree), which this reader does not follow yet (#7)",
                        k.start_offset(),
                        k.pos.offset
                    )))
                }
                t => return Err(Error::Unsupported(format!("extent key type {t}"))),
            }
        }
        Ok(out)
    }

    /// The generation the allocator records for the bucket holding
    /// `sector`: byte 4 of the second word of the `alloc_v4` key at
    /// `dev:bucket` (docs/clean-room.md, "Allocating space"), or `None`
    /// when the bucket has no alloc key.
    fn bucket_gen(&self, sector: u64) -> Result<Option<u8>> {
        let bucket = sector / self.bucket_size;
        let mut c = self.cursor(btree_id::ALLOC)?;
        c.seek(Bpos {
            inode: u64::from(self.dev_idx),
            offset: bucket,
            snapshot: 0,
        })?;
        while let Some(k) = c.next_key()? {
            if k.pos.inode != u64::from(self.dev_idx) || k.pos.offset != bucket {
                break;
            }
            if k.key_type == crate::bkey::key_type::ALLOC_V4 {
                return Ok(crate::extent::alloc_v4_gen(&k.value));
            }
        }
        Ok(None)
    }

    /// The `len` live sectors of one extent, decompressed and checked.
    fn extent_data(&self, e: &DataExtent) -> Result<Vec<u8>> {
        e.ptr.check(self.dev_idx, self.bucket_gen(e.ptr.offset)?)?;
        let (stored_sectors, crc) = match e.crc {
            Some(c) => (c.compressed_size as u64, c),
            None => {
                let mut b = vec![0u8; e.len as usize * 512];
                self.dev.read_at(e.ptr.offset * 512, &mut b)?;
                return Ok(b);
            }
        };
        let mut raw = vec![0u8; stored_sectors as usize * 512];
        self.dev.read_at(e.ptr.offset * 512, &mut raw)?;
        if !crate::csum::is_known(crc.csum_type) {
            return Err(Error::Unsupported(format!(
                "data checksum type {}",
                crc.csum_type
            )));
        }
        // 32- and 64-bit checksums live entirely in the low word; the
        // high bits are only used by 128-bit MACs, which are not read here.
        let stored = crc.csum_lo;
        crate::csum::verify(crc.csum_type, &raw, stored).map_err(|computed| {
            Error::BadChecksum {
                what: "extent data",
                stored,
                computed,
            }
        })?;
        let plain = match crc.compression_type {
            compression::NONE | compression::INCOMPRESSIBLE => raw,
            t => crate::compress::decompress(t, &raw, crc.uncompressed_size as usize * 512)?,
        };
        let from = e.skip as usize * 512;
        let to = from + e.len as usize * 512;
        if to > plain.len() {
            return Err(Error::Corrupt(
                "extent's live range exceeds its data".into(),
            ));
        }
        Ok(plain[from..to].to_vec())
    }
}

/// The refusal for a key at a snapshot other than the root's.
fn snapshots_not_read(k: &Bkey, root_snapshot: u32) -> Error {
    Error::Unsupported(format!(
        "key {} is at snapshot {}, the root inode at {root_snapshot}: snapshots and subvolumes \
         are not read (issue #12)",
        k.pos, k.pos.snapshot
    ))
}

/// Copy the part of `data` (which starts at file byte `data_start`) that
/// falls inside the window `[win_start, win_end)` into `out`, which holds
/// that window.
fn copy_window(out: &mut [u8], win_start: u64, win_end: u64, data_start: u64, data: &[u8]) {
    let data_end = data_start.saturating_add(data.len() as u64);
    let from = data_start.max(win_start);
    let to = data_end.min(win_end);
    if from >= to {
        return;
    }
    out[(from - win_start) as usize..(to - win_start) as usize]
        .copy_from_slice(&data[(from - data_start) as usize..(to - data_start) as usize]);
}
