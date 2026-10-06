//! A read-only view of a single-device bcachefs filesystem: look up a
//! path, list a directory, read a file.
//!
//! The spike loads the inodes and dirents btrees whole at open, and walks
//! the extents btree per file read. Snapshots are ignored: every key is
//! taken at the snapshot it was found at, which is right for a filesystem
//! that has never had a snapshot taken and wrong otherwise.

use std::collections::BTreeMap;

use crate::btree::{self, btree_id};
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
    inodes: BTreeMap<u64, Inode>,
    /// directory inode -> entries, in btree (hash) order
    dirents: BTreeMap<u64, Vec<Dirent>>,
}

impl<D: BlockRead> Filesystem<D> {
    pub fn open(dev: D) -> Result<Self> {
        let sb = Superblock::read(&dev)?;
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
        let mut inodes = BTreeMap::new();
        for k in btree::walk_replayed(&dev, &sb, btree_id::INODES, replay.as_ref())? {
            if k.key_type == crate::bkey::key_type::INODE_V3 {
                let i = Inode::from_key(&k)?;
                inodes.insert(i.ino, i);
            }
        }
        let mut dirents: BTreeMap<u64, Vec<Dirent>> = BTreeMap::new();
        for k in btree::walk_replayed(&dev, &sb, btree_id::DIRENTS, replay.as_ref())? {
            if k.key_type == crate::bkey::key_type::DIRENT {
                let d = Dirent::from_key(&k)?;
                dirents.entry(d.dir).or_default().push(d);
            }
        }
        Ok(Filesystem {
            dev,
            sb,
            replay,
            inodes,
            dirents,
        })
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    pub fn inode(&self, ino: u64) -> Result<&Inode> {
        self.inodes
            .get(&ino)
            .ok_or_else(|| Error::NotFound(format!("inode {ino}")))
    }

    /// The entries of a directory, by inode.
    pub fn readdir(&self, dir: u64) -> Result<&[Dirent]> {
        if !self.inode(dir)?.is_dir() {
            return Err(Error::Corrupt(format!("inode {dir} is not a directory")));
        }
        Ok(self.dirents.get(&dir).map(|v| v.as_slice()).unwrap_or(&[]))
    }

    /// Resolve an absolute path to an inode, without following a symlink
    /// in the last component.
    pub fn lookup(&self, path: &str) -> Result<u64> {
        let mut ino = ROOT_INO;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let d = self
                .readdir(ino)?
                .iter()
                .find(|d| d.name == part.as_bytes())
                .ok_or_else(|| Error::NotFound(path.to_string()))?;
            ino = d.inum;
        }
        Ok(ino)
    }

    /// The extended attributes of an inode, in btree order.
    pub fn xattrs(&self, ino: u64) -> Result<Vec<crate::xattr::Xattr>> {
        self.inode(ino)?;
        btree::walk_replayed(&self.dev, &self.sb, btree_id::XATTRS, self.replay.as_ref())?
            .iter()
            .filter(|k| k.pos.inode == ino && k.key_type == crate::bkey::key_type::XATTR)
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
        for k in btree::walk_replayed(&self.dev, &self.sb, btree_id::EXTENTS, self.replay.as_ref())?
        {
            if k.pos.inode != ino {
                continue;
            }
            match k.key_type {
                crate::bkey::key_type::EXTENT => {}
                crate::bkey::key_type::RESERVATION | crate::bkey::key_type::WHITEOUT => continue,
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

    /// The `len` live sectors of one extent, decompressed and checked.
    fn extent_data(&self, e: &DataExtent) -> Result<Vec<u8>> {
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
