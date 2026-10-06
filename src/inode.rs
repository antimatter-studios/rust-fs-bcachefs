//! Inodes (`inode_v3`) and directory entries.
//!
//! Provenance (docs/clean-room.md): that inodes live in the inodes btree,
//! that v3 is the current "compact encoding", and the names and order of
//! the inode's fields come from the documentation and the reference
//! lister's printout of each inode (S1, S3). The encoding itself was found
//! by hexdump against those printed values (S4):
//!
//! * the inodes btree keys an inode by its number in the OFFSET field of
//!   the position (the lister prints `0:4096:U32_MAX` for inode 4096);
//! * the value starts with journal_seq u64, hash_seed u64, and a u64 whose
//!   bits 0..31 are flags (bits 24..31 hold the number of varint fields
//!   that follow, bits 20..23 the string hash type) and bits 36..51 the
//!   mode; then bi_sectors, bi_size and bi_version as u64s (in that
//!   order: sectors first, unlike the lister's printing order); then the
//!   remaining fields as varints, in the lister's order, each time field
//!   taking two varints;
//! * a varint's length is one more than the number of trailing one bits of
//!   its first byte; a 9-byte varint is the next eight bytes verbatim;
//!   otherwise the value is the little-endian bytes shifted right by the
//!   length.
//!
//! A dirent is keyed (directory inode, name hash); its value is the target
//! inode (u64), the type (u8, `DT_*` numbering) and the name, NUL-padded.

use crate::bkey::{key_type, Bkey};
use crate::error::{Error, Result};
use crate::util::le64;

/// The root directory's inode number (the lister lists `/`'s entries
/// under it).
pub const ROOT_INO: u64 = 4096;

/// The varint fields of an `inode_v3`, after the fixed part, in order.
/// Times take two varints each.
const FIELDS: &[(&str, usize)] = &[
    ("atime", 2),
    ("ctime", 2),
    ("mtime", 2),
    ("otime", 2),
    ("uid", 1),
    ("gid", 1),
    ("nlink", 1),
    ("generation", 1),
    ("dev", 1),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inode {
    pub ino: u64,
    pub mode: u32,
    pub flags: u32,
    pub size: u64,
    pub sectors: u64,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub atime: u64,
    pub ctime: u64,
    pub mtime: u64,
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & 0o170000 == 0o040000
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & 0o170000 == 0o120000
    }
    pub fn is_file(&self) -> bool {
        self.mode & 0o170000 == 0o100000
    }

    /// The link count a mount reports for this inode (`st_nlink`).
    /// INFERRED from the aged fixture, where every inode's stored count
    /// was compared with what the mount reported: a file or symlink stores
    /// one less than its links (a file with three names stores 2), and a
    /// directory stores its number of subdirectories, to which the mount
    /// adds 2 for its own entry and `.`.
    pub fn link_count(&self) -> u32 {
        if self.is_dir() {
            self.nlink.saturating_add(2)
        } else {
            self.nlink.saturating_add(1)
        }
    }

    pub fn from_key(k: &Bkey) -> Result<Self> {
        if k.key_type != key_type::INODE_V3 {
            return Err(Error::Unsupported(format!("inode key type {}", k.key_type)));
        }
        let v = &k.value;
        if v.len() < 48 {
            return Err(Error::Corrupt(
                "inode_v3 shorter than its fixed part".into(),
            ));
        }
        let fm = le64(v, 16);
        let nr_fields = ((fm >> 24) & 0xff) as usize;
        let mut ino = Inode {
            ino: k.pos.offset,
            mode: ((fm >> 36) & 0xffff) as u32,
            flags: fm as u32,
            sectors: le64(v, 24),
            size: le64(v, 32),
            uid: 0,
            gid: 0,
            nlink: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
        };
        let mut p = 48;
        for (i, (name, count)) in FIELDS.iter().enumerate() {
            if i >= nr_fields {
                break;
            }
            let mut first = 0;
            for j in 0..*count {
                let (val, n) = varint(&v[p.min(v.len())..])?;
                if j == 0 {
                    first = val;
                }
                p += n;
            }
            match *name {
                "atime" => ino.atime = first,
                "ctime" => ino.ctime = first,
                "mtime" => ino.mtime = first,
                "uid" => ino.uid = first as u32,
                "gid" => ino.gid = first as u32,
                "nlink" => ino.nlink = first as u32,
                _ => {}
            }
        }
        Ok(ino)
    }
}

/// Decode one varint; returns the value and its length in bytes.
pub fn varint(b: &[u8]) -> Result<(u64, usize)> {
    let first = *b
        .first()
        .ok_or_else(|| Error::Corrupt("varint past the end of the inode".into()))?;
    let len = first.trailing_ones() as usize + 1;
    if len >= 9 {
        if b.len() < 9 {
            return Err(Error::Corrupt("9-byte varint truncated".into()));
        }
        return Ok((le64(b, 1), 9));
    }
    if b.len() < len {
        return Err(Error::Corrupt("varint truncated".into()));
    }
    let mut raw = [0u8; 8];
    raw[..len].copy_from_slice(&b[..len]);
    Ok((u64::from_le_bytes(raw) >> len, len))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirent {
    pub dir: u64,
    pub name: Vec<u8>,
    pub inum: u64,
    pub d_type: u8,
}

impl Dirent {
    pub fn from_key(k: &Bkey) -> Result<Self> {
        if k.key_type != key_type::DIRENT {
            return Err(Error::Unsupported(format!(
                "dirent key type {}",
                k.key_type
            )));
        }
        let v = &k.value;
        if v.len() < 10 {
            return Err(Error::Corrupt("dirent shorter than its fixed part".into()));
        }
        let raw = &v[9..];
        let n = raw
            .iter()
            .rposition(|&c| c != 0)
            .map(|i| i + 1)
            .unwrap_or(0);
        if n == 0 {
            return Err(Error::Corrupt("dirent with an empty name".into()));
        }
        Ok(Dirent {
            dir: k.pos.inode,
            name: raw[..n].to_vec(),
            inum: le64(v, 0),
            d_type: v[8],
        })
    }

    /// The type's name as the reference lister prints it.
    pub fn type_name(&self) -> &'static str {
        match self.d_type {
            1 => "fifo",
            2 => "chr",
            4 => "dir",
            6 => "blk",
            8 => "reg",
            10 => "lnk",
            12 => "sock",
            _ => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_decode_as_measured() {
        // atime 273717049 was stored as 2f e7 12 0a 02.
        assert_eq!(
            varint(&[0x2f, 0xe7, 0x12, 0x0a, 0x02]).unwrap(),
            (273_717_049, 5)
        );
        // nlink 2 as 04; inode 4096 as 01 40.
        assert_eq!(varint(&[0x04]).unwrap(), (2, 1));
        assert_eq!(varint(&[0x01, 0x40]).unwrap(), (4096, 2));
        let mut nine = vec![0xff];
        nine.extend_from_slice(&2710689797173725757u64.to_le_bytes());
        assert_eq!(varint(&nine).unwrap(), (2710689797173725757, 9));
        assert!(varint(&[]).is_err());
        assert!(varint(&[0xff, 1]).is_err());
        assert!(varint(&[0x03]).is_err());
    }
}
