//! A read-only checker: what a filesystem's own structures say about each
//! other, without repairing anything.
//!
//! Checked: the superblock and its checksum; every btree this reader knows,
//! node by node (magic, sequence, checksums, key order, keys inside each
//! node's range); every directory entry against the inode it names (it
//! exists, its type agrees, its directory is a directory); every inode's
//! link count against the entries that name it; every extent against its
//! inode (it exists, the extent does not start past the end of the file,
//! it does not overlap the extent before it); every data extent's checksum;
//! and that every key is at the root inode's snapshot, since this checker
//! reads one snapshot and knows nothing of the others. A filesystem not
//! shut down cleanly is checked through the replay of its journal, as it
//! would be read.
//!
//! It is a subset of what the reference checker checks (allocation,
//! accounting, backpointers and lru are not), and the tests hold it to
//! that: wherever it reports a problem, so does the reference (tests in the
//! guest), and on every clean fixture it reports none.

use std::collections::BTreeMap;

use crate::bkey::{key_type, Bkey, Bpos};
use crate::btree::{self, btree_id};
use crate::error::{Error, Result};
use crate::extent::DataExtent;
use crate::inode::{Dirent, Inode, ROOT_INO};
use crate::journal::{self, Replay};
use crate::superblock::Superblock;
use fs_core::BlockRead;

/// One thing found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// A short, stable name for the kind of problem.
    pub kind: &'static str,
    /// What and where.
    pub detail: String,
}

/// What the checker found.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub problems: Vec<Problem>,
    /// Inodes, directory entries and extents looked at.
    pub inodes: u64,
    pub dirents: u64,
    pub extents: u64,
    /// Whether the journal was replayed to check the filesystem.
    pub replayed: bool,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.problems.is_empty()
    }

    fn add(&mut self, kind: &'static str, detail: impl Into<String>) {
        self.problems.push(Problem {
            kind,
            detail: detail.into(),
        });
    }
}

/// Check the filesystem on `dev`. An `Err` is a filesystem this checker
/// cannot check at all (not bcachefs, encrypted, several devices); every
/// problem it can name is in the report.
pub fn check(dev: &dyn BlockRead) -> Result<Report> {
    let mut r = Report::default();
    let sb = match Superblock::read_copies(dev) {
        Ok((sb, failed)) => {
            // A copy that does not read is damage even when another copy
            // does: the next torn write has one fewer to fall back on.
            for (sector, e) in failed {
                r.add("superblock_copy", format!("copy at sector {sector}: {e}"));
            }
            sb
        }
        Err(e @ (Error::BadChecksum { .. } | Error::BadMagic { .. })) => {
            r.add(
                "superblock",
                format!("no copy of the superblock reads: {e}"),
            );
            return Ok(r);
        }
        Err(e) => return Err(e),
    };
    if sb.field(2).is_some() {
        return Err(Error::Unsupported("the filesystem is encrypted".into()));
    }
    if sb.nr_devices != 1 {
        return Err(Error::Unsupported(format!("{} devices", sb.nr_devices)));
    }
    let replay = if sb.is_clean() {
        None
    } else {
        r.replayed = true;
        match journal::replay(dev, &sb) {
            Ok(rp) => Some(rp),
            Err(e) => {
                r.add("journal", e.to_string());
                return Ok(r);
            }
        }
    };

    // Every node of every btree with a root.
    let roots: Vec<u8> = match &replay {
        Some(rp) => rp.roots.iter().map(|(id, _, _)| *id).collect(),
        None => sb.btree_roots()?.iter().map(|x| x.btree_id).collect(),
    };
    let mut trees: BTreeMap<u8, Vec<Bkey>> = BTreeMap::new();
    for id in roots {
        match walk_checked(dev, &sb, id, replay.as_ref(), &mut r) {
            Some(keys) => {
                trees.insert(id, keys);
            }
            None => continue,
        }
    }

    // Snapshots: this checker reads at one snapshot, the root inode's. A
    // key at another belongs to a snapshot or subvolume whose visibility
    // it does not implement, so everything below is said of one snapshot
    // only, and the report says so (issue #53).
    let root_snapshot = trees
        .get(&btree_id::INODES)
        .into_iter()
        .flatten()
        .find(|k| k.pos.inode == 0 && k.pos.offset == ROOT_INO && k.key_type == key_type::INODE_V3)
        .map(|k| k.pos.snapshot);
    if let Some(snap) = root_snapshot {
        let elsewhere = [
            btree_id::EXTENTS,
            btree_id::INODES,
            btree_id::DIRENTS,
            btree_id::XATTRS,
        ]
        .iter()
        .filter_map(|id| trees.get(id).map(|keys| (*id, keys)))
        .flat_map(|(id, keys)| keys.iter().map(move |k| (id, k)))
        .find(|(_, k)| k.pos.snapshot != snap);
        if let Some((id, k)) = elsewhere {
            r.add(
                "snapshots",
                format!(
                    "btree {}: key {} is at snapshot {}, the root inode at {snap}; snapshots are \
                     not checked, so nothing below speaks for them",
                    journal::btree_name(id),
                    k.pos,
                    k.pos.snapshot
                ),
            );
        }
    }

    // Inodes, dirents, link counts.
    let mut inodes: BTreeMap<u64, Inode> = BTreeMap::new();
    for k in trees.get(&btree_id::INODES).into_iter().flatten() {
        if k.key_type != key_type::INODE_V3 {
            continue;
        }
        match Inode::from_key(k) {
            Ok(i) => {
                inodes.insert(i.ino, i);
            }
            Err(e) => r.add("inode", format!("{}: {e}", k.pos)),
        }
    }
    r.inodes = inodes.len() as u64;
    if !inodes.contains_key(&ROOT_INO) {
        r.add("root_inode", "the root directory's inode is missing");
    }
    let mut links: BTreeMap<u64, u32> = BTreeMap::new();
    let mut subdirs: BTreeMap<u64, u32> = BTreeMap::new();
    for k in trees.get(&btree_id::DIRENTS).into_iter().flatten() {
        if k.key_type != key_type::DIRENT {
            continue;
        }
        r.dirents += 1;
        let d = match Dirent::from_key(k) {
            Ok(d) => d,
            Err(e) => {
                r.add("dirent", format!("{}: {e}", k.pos));
                continue;
            }
        };
        let name = String::from_utf8_lossy(&d.name);
        match inodes.get(&d.dir) {
            Some(dir) if dir.is_dir() => {}
            Some(_) => r.add(
                "dirent_in_non_directory",
                format!("{name:?} in inode {}", d.dir),
            ),
            None => r.add(
                "dirent_in_missing_directory",
                format!("{name:?} in inode {}", d.dir),
            ),
        }
        match inodes.get(&d.inum) {
            None => r.add(
                "dirent_to_missing_inode",
                format!("{name:?} in {} names inode {}", d.dir, d.inum),
            ),
            Some(i) => {
                let want = match i.mode & 0o170000 {
                    0o040000 => 4,
                    0o100000 => 8,
                    0o120000 => 10,
                    0o020000 => 2,
                    0o060000 => 6,
                    0o010000 => 1,
                    0o140000 => 12,
                    _ => 0,
                };
                if want != d.d_type {
                    r.add(
                        "dirent_type",
                        format!(
                            "{name:?} says type {}, inode {} is {:o}",
                            d.d_type, d.inum, i.mode
                        ),
                    );
                }
                *links.entry(d.inum).or_default() += 1;
                if i.is_dir() {
                    *subdirs.entry(d.dir).or_default() += 1;
                }
            }
        }
    }
    // Link counts and reachability only mean something when both btrees
    // were read whole; a btree that failed is already reported.
    let both = trees.contains_key(&btree_id::INODES) && trees.contains_key(&btree_id::DIRENTS);
    for (ino, i) in inodes.iter().filter(|_| both) {
        let want = if i.is_dir() {
            subdirs.get(ino).copied().unwrap_or(0) + 2
        } else {
            links.get(ino).copied().unwrap_or(0)
        };
        let unreachable = *ino != ROOT_INO && links.get(ino).copied().unwrap_or(0) == 0;
        if unreachable {
            r.add(
                "unreachable_inode",
                format!("inode {ino} has no directory entry"),
            );
        } else if i.link_count() != want {
            r.add(
                "link_count",
                format!("inode {ino} counts {} links; {want} found", i.link_count()),
            );
        }
    }

    // Extents: their inode, their bounds, their neighbours, their data.
    let have_inodes = trees.contains_key(&btree_id::INODES);
    // (inode, snapshot, end sector) of the last extent-like key seen: an
    // extent that starts before it ends overlaps it (S1's check_extents,
    // "no overlaps"; issue #60).
    let mut prev: Option<(u64, u32, u64)> = None;
    for k in trees
        .get(&btree_id::EXTENTS)
        .into_iter()
        .flatten()
        .filter(|_| have_inodes)
    {
        if k.key_type == key_type::ERROR {
            r.add(
                "data_lost",
                format!(
                    "extent {}: an `error` extent, its data permanently lost",
                    k.pos
                ),
            );
        }
        if matches!(
            k.key_type,
            key_type::EXTENT | key_type::INLINE_DATA | key_type::RESERVATION | key_type::ERROR
        ) {
            if let Some((inode, snap, end)) = prev {
                if inode == k.pos.inode && snap == k.pos.snapshot && k.start_offset() < end {
                    r.add(
                        "extent_overlap",
                        format!(
                            "extent {} starts at sector {} before the extent before it ends at {end}",
                            k.pos,
                            k.start_offset()
                        ),
                    );
                }
            }
            prev = Some((k.pos.inode, k.pos.snapshot, k.pos.offset));
        }
        match k.key_type {
            key_type::EXTENT | key_type::INLINE_DATA => {}
            _ => continue,
        }
        r.extents += 1;
        let Some(i) = inodes.get(&k.pos.inode) else {
            r.add("extent_without_inode", format!("extent {}", k.pos));
            continue;
        };
        if k.start_offset() * 512 >= i.size.div_ceil(512) * 512 {
            r.add(
                "extent_past_end_of_inode",
                format!("extent {} of inode {} (size {})", k.pos, i.ino, i.size),
            );
        }
        if k.key_type == key_type::EXTENT {
            if let Err(e) = DataExtent::from_key(k).and_then(|e| verify_data(dev, &e)) {
                r.add("extent_data", format!("extent {}: {e}", k.pos));
            }
        }
    }
    Ok(r)
}

/// Read one extent's stored bytes and verify its checksum.
fn verify_data(dev: &dyn BlockRead, e: &DataExtent) -> Result<()> {
    let Some(crc) = e.crc else { return Ok(()) };
    if !crate::csum::is_known(crc.csum_type) {
        return Err(Error::Unsupported(format!(
            "data checksum type {}",
            crc.csum_type
        )));
    }
    let mut raw = vec![0u8; crc.compressed_size as usize * 512];
    dev.read_at(e.ptr.offset * 512, &mut raw)?;
    crate::csum::verify(crc.csum_type, &raw, crc.csum_lo).map_err(|computed| Error::BadChecksum {
        what: "extent data",
        stored: crc.csum_lo,
        computed,
    })
}

/// Walk btree `id`, recording each node that cannot be read instead of
/// stopping, and the order of the keys found. `None` when not even its root
/// can be read.
fn walk_checked(
    dev: &dyn BlockRead,
    sb: &Superblock,
    id: u8,
    replay: Option<&Replay>,
    r: &mut Report,
) -> Option<Vec<Bkey>> {
    let name = journal::btree_name(id);
    match btree::walk_replayed(dev, sb, id, replay) {
        Ok(keys) => {
            let mut last: Option<Bpos> = None;
            for k in &keys {
                if last.is_some_and(|l| k.pos <= l) {
                    r.add(
                        "key_order",
                        format!("btree {name}: {} after {}", k.pos, last.unwrap()),
                    );
                }
                last = Some(k.pos);
            }
            Some(keys)
        }
        Err(e) => {
            r.add("btree_node", format!("btree {name}: {e}"));
            None
        }
    }
}
