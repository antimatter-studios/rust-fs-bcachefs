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

    /// The entry named `name` in directory `dir`: at its name's hash, or
    /// (a collision moved it) anywhere in the directory.
    fn find(&self, dir: &Inode, name: &[u8]) -> Result<Option<Dirent>> {
        let at = crate::inode::dirent_hash(dir.hash_seed, name);
        let mut c = self.cursor(btree_id::DIRENTS)?;
        c.seek(Bpos {
            inode: dir.ino,
            offset: at,
            snapshot: 0,
        })?;
        if let Some(k) = c.next_key()? {
            if k.pos.inode == dir.ino {
                self.same_snapshot(&k)?;
            }
            if k.pos.inode == dir.ino && k.key_type == crate::bkey::key_type::DIRENT {
                let d = Dirent::from_key(&k)?;
                if d.name == name {
                    return Ok(Some(d));
                }
            }
        }
        Ok(self.readdir(dir.ino)?.into_iter().find(|d| d.name == name))
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
        let inode = self.inode(ino)?;
        let size = inode.size;
        let mut out = vec![
            0u8;
            usize::try_from(size)
                .map_err(|_| Error::Unsupported("file larger than memory".into()))?
        ];
        // Extents of one file must not overlap (S1's check_extents: "no
        // overlaps"): two keys claiming the same sectors would be written
        // into the buffer in position order, and the later one, not the
        // newer one, would win. Refused instead (issue #60).
        let mut prev_end: Option<u64> = None;
        for k in self.keys_of(btree_id::EXTENTS, ino)? {
            if matches!(
                k.key_type,
                crate::bkey::key_type::EXTENT
                    | crate::bkey::key_type::INLINE_DATA
                    | crate::bkey::key_type::RESERVATION
                    | crate::bkey::key_type::ERROR
            ) {
                if let Some(end) = prev_end {
                    if k.start_offset() < end {
                        return Err(Error::Corrupt(format!(
                            "extent {} starts at sector {} before the extent before it ends at \
                             {end}: overlapping extents, check the filesystem",
                            k.pos,
                            k.start_offset()
                        )));
                    }
                }
                prev_end = Some(k.pos.offset);
            }
            match k.key_type {
                crate::bkey::key_type::EXTENT => {}
                // A reservation is space with no data yet, a whiteout hides
                // nothing on a filesystem without snapshots: both read as
                // zeros.
                crate::bkey::key_type::RESERVATION
                | crate::bkey::key_type::WHITEOUT
                | crate::bkey::key_type::EXTENT_WHITEOUT => continue,
                // "Reads to these ranges return IO errors" (S1 9.1.2.1).
                crate::bkey::key_type::ERROR => {
                    return Err(Error::Io(format!(
                        "inode {ino}, sectors {}..{}: the data was permanently lost (an `error` \
                         extent)",
                        k.start_offset(),
                        k.pos.offset
                    )))
                }
                crate::bkey::key_type::INLINE_DATA => {
                    // The value is the data itself, zero-padded to a whole
                    // u64, and the key covers `size` sectors ending at its
                    // position like any extent (S3: the lister prints the
                    // bytes and a `datalen` equal to the value length; S4:
                    // the bytes match the file the mount wrote).
                    let start = k
                        .pos
                        .offset
                        .checked_sub(k.size as u64)
                        .ok_or_else(|| Error::Corrupt("inline extent before offset 0".into()))?
                        .checked_mul(512)
                        .ok_or_else(|| Error::Corrupt("inline extent offset overflows".into()))?;
                    if start >= size {
                        continue;
                    }
                    let n = ((size - start) as usize)
                        .min(k.value.len())
                        .min(k.size as usize * 512);
                    out[start as usize..start as usize + n].copy_from_slice(&k.value[..n]);
                    continue;
                }
                t => return Err(Error::Unsupported(format!("extent key type {t}"))),
            }
            let e = DataExtent::from_key(&k)?;
            let data = self.extent_data(&e)?;
            let file_off = e.file_start * 512;
            if file_off >= size {
                continue;
            }
            let n = ((size - file_off) as usize).min(data.len());
            out[file_off as usize..file_off as usize + n].copy_from_slice(&data[..n]);
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
