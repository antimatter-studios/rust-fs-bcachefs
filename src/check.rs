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
//! that every key is at a snapshot the snapshots btree has; every xattr
//! against its inode (it exists, the xattr is reachable from its name's
//! hash slot); every inode's back-reference to the entry naming it; every
//! inode's has-options flag against its option fields. Entries, inodes
//! and link counts are judged as each subvolume sees them through its
//! snapshot's chain, and an entry naming a subvolume against the
//! subvolumes btree. Every reflinked extent is followed into the reflink
//! btree: its range is covered, each indirect extent's refcount is the
//! number of pointers into it, and its data is checked as an extent's is.
//! A filesystem not shut down cleanly is checked through the replay of its
//! journal, as it would be read.
//!
//! Of the allocation btrees, each bucket's generation in bucket_gens is
//! checked against its alloc key.
//!
//! It is a subset of what the reference checker checks (the rest of
//! allocation, accounting, backpointers and lru are not), and the tests
//! hold it to that: wherever it reports a problem, so does the reference
//! (tests in the guest), and on every clean fixture it reports none.

use std::collections::BTreeMap;

use crate::bkey::{key_type, Bkey, Bpos};
use crate::btree::{self, btree_id};
use crate::error::{Error, Result};
use crate::extent::DataExtent;
use crate::inode::{Dirent, Inode, InodeV3Raw, INODE_HAS_OPTIONS, OPTION_FIELDS, ROOT_INO};
use crate::journal::{self, Replay};
use crate::superblock::Superblock;
use crate::util::le64;
use fs_core::BlockRead;

/// The snapshots and subvolumes btrees (S1 11.3) and their key types (S1
/// 11.5), as the reader reads them (src/fs.rs).
const SUBVOLUMES: u8 = 8;
const SNAPSHOTS: u8 = 9;
const KEY_SUBVOLUME: u8 = 21;
const KEY_SNAPSHOT: u8 = 22;
/// The bucket_gens btree, and its key type: 256 buckets' generations a
/// key, one byte each, at `dev:bucket >> 8` (S8).
const BUCKET_GENS: u8 = 14;
const KEY_BUCKET_GENS: u8 = 30;
/// A dirent's type when it names a subvolume (the lister's `type subvol`).
const DT_SUBVOL: u8 = 16;
/// Deeper than any snapshot tree; a parent cycle stops here.
const MAX_SNAPSHOT_DEPTH: usize = 1024;

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
    // Through the journal after an unclean shutdown, and after a clean one
    // whose superblock records a root that will not read (#166).
    let replay = match btree::journal_for(dev, &sb) {
        Ok(rp) => {
            r.replayed = rp.is_some();
            rp
        }
        Err(e) => {
            r.add("journal", e.to_string());
            return Ok(r);
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

    // Snapshots (#139): each snapshot's parent, from the snapshots btree,
    // and each subvolume's root inode and snapshot. A key is seen by a
    // snapshot when it sits on that snapshot's chain, the nearest one
    // winning and a whiteout hiding; everything below is judged as each
    // subvolume sees it.
    let parents: BTreeMap<u32, u32> = trees
        .get(&SNAPSHOTS)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == KEY_SNAPSHOT && k.value.len() >= 8)
        .map(|k| (k.pos.offset as u32, (le64(&k.value, 0) >> 32) as u32))
        .collect();
    let subvolumes: BTreeMap<u32, (u64, u32)> = trees
        .get(&SUBVOLUMES)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == KEY_SUBVOLUME && k.value.len() >= 16)
        .map(|k| {
            (
                k.pos.offset as u32,
                (le64(&k.value, 8), (le64(&k.value, 0) >> 32) as u32),
            )
        })
        .collect();
    let root_snapshot = trees
        .get(&btree_id::INODES)
        .into_iter()
        .flatten()
        .find(|k| k.pos.inode == 0 && k.pos.offset == ROOT_INO && k.key_type == key_type::INODE_V3)
        .map(|k| k.pos.snapshot);
    // A key at a snapshot the snapshots btree does not have cannot be
    // placed; without a snapshots btree, every key is at the root's.
    let known = |snap: u32| {
        if parents.is_empty() {
            root_snapshot.is_none_or(|r| r == snap)
        } else {
            parents.contains_key(&snap)
        }
    };
    let unplaced = [
        btree_id::EXTENTS,
        btree_id::INODES,
        btree_id::DIRENTS,
        btree_id::XATTRS,
    ]
    .iter()
    .filter_map(|id| trees.get(id).map(|keys| (*id, keys)))
    .flat_map(|(id, keys)| keys.iter().map(move |k| (id, k)))
    .find(|(_, k)| !known(k.pos.snapshot));
    if let Some((id, k)) = unplaced {
        r.add(
            "snapshots",
            format!(
                "btree {}: key {} is at snapshot {}, which the snapshots btree does not have",
                journal::btree_name(id),
                k.pos,
                k.pos.snapshot
            ),
        );
    }
    let chain = |snap: u32| {
        let mut out = vec![snap];
        let mut at = snap;
        while let Some(&p) = parents.get(&at) {
            if p == 0 || out.len() >= MAX_SNAPSHOT_DEPTH || out.contains(&p) {
                break;
            }
            out.push(p);
            at = p;
        }
        out
    };
    // Each subvolume's snapshot, and the root directories it holds; with no
    // subvolumes btree, the root inode's snapshot alone.
    let mut views: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    for (root, snap) in subvolumes.values() {
        views.entry(*snap).or_default().push(*root);
    }
    if views.is_empty() {
        views.insert(root_snapshot.unwrap_or(u32::MAX), vec![ROOT_INO]);
    }

    // Every version of each inode and each dirent slot, by position.
    let mut inode_keys: BTreeMap<u64, Vec<&Bkey>> = BTreeMap::new();
    for k in trees.get(&btree_id::INODES).into_iter().flatten() {
        if k.pos.inode == 0 {
            inode_keys.entry(k.pos.offset).or_default().push(k);
        }
    }
    let mut dirent_keys: BTreeMap<(u64, u64), Vec<&Bkey>> = BTreeMap::new();
    for k in trees.get(&btree_id::DIRENTS).into_iter().flatten() {
        dirent_keys
            .entry((k.pos.inode, k.pos.offset))
            .or_default()
            .push(k);
    }
    let mut xattr_keys: BTreeMap<(u64, u64), Vec<&Bkey>> = BTreeMap::new();
    for k in trees.get(&btree_id::XATTRS).into_iter().flatten() {
        xattr_keys
            .entry((k.pos.inode, k.pos.offset))
            .or_default()
            .push(k);
    }
    // A subvolume's root is named by an entry in its parent that names
    // the subvolume, not the inode.
    let subvolume_roots: std::collections::BTreeSet<u64> =
        subvolumes.values().map(|(root, _)| *root).collect();
    // Every inode key decoded once; a key that does not decode is named.
    let mut decoded: BTreeMap<Bpos, Inode> = BTreeMap::new();
    for k in trees.get(&btree_id::INODES).into_iter().flatten() {
        if k.key_type != key_type::INODE_V3 {
            continue;
        }
        match Inode::from_key(k) {
            Ok(i) => {
                decoded.insert(k.pos, i);
            }
            Err(e) => r.add("inode", format!("{}: {e}", k.pos)),
        }
    }
    r.inodes = decoded.len() as u64;
    // The has-options flag (#163): bit 12 of bi_flags is set exactly when
    // the inode carries an option, set on it or inherited (S3 + S4, #142).
    // A nocow inode may carry it with none of the option fields set.
    for k in trees.get(&btree_id::INODES).into_iter().flatten() {
        if k.key_type != key_type::INODE_V3 || !decoded.contains_key(&k.pos) {
            continue;
        }
        let Ok(raw) = InodeV3Raw::parse(&k.value) else {
            continue;
        };
        let has = OPTION_FIELDS.iter().any(|f| raw.field(f).unwrap_or(0) != 0);
        let flagged = raw.flags & INODE_HAS_OPTIONS != 0;
        let nocow = raw.field("nocow").unwrap_or(0) != 0;
        if has != flagged && !(flagged && nocow) {
            r.add(
                "inode_has_inode_opts_flag_wrong",
                format!(
                    "inode {}: the has-options flag is {}, and it {} options",
                    k.pos,
                    if flagged { "set" } else { "clear" },
                    if has { "has" } else { "has no" }
                ),
            );
        }
    }
    // The inode a snapshot sees at `ino`.
    let inode_seen = |chain: &[u32], ino: u64| -> Option<&Inode> {
        let k = nearest(chain, inode_keys.get(&ino)?)?;
        decoded.get(&k.pos)
    };
    if root_snapshot.is_none() {
        r.add("root_inode", "the root directory's inode is missing");
    }

    // Inodes, dirents and link counts, as each subvolume sees them.
    let both = trees.contains_key(&btree_id::INODES) && trees.contains_key(&btree_id::DIRENTS);
    let mut found: Vec<Problem> = Vec::new();
    let mut dirents_seen = std::collections::BTreeSet::new();
    for (snap, roots) in &views {
        let c = chain(*snap);
        let mut add = |kind: &'static str, detail: String| {
            let p = Problem { kind, detail };
            if !found.contains(&p) {
                found.push(p);
            }
        };
        let mut links: BTreeMap<u64, u32> = BTreeMap::new();
        let mut subdirs: BTreeMap<u64, u32> = BTreeMap::new();
        for (slot, keys) in &dirent_keys {
            let Some(k) = nearest(&c, keys) else { continue };
            if k.key_type != key_type::DIRENT {
                continue;
            }
            if dirents_seen.insert(k.pos) {
                r.dirents += 1;
            }
            let d = match Dirent::from_key(k) {
                Ok(d) => d,
                Err(e) => {
                    add("dirent", format!("{}: {e}", k.pos));
                    continue;
                }
            };
            let name = String::from_utf8_lossy(&d.name);
            match inode_seen(&c, slot.0) {
                Some(dir) if dir.is_dir() => {}
                Some(_) => add(
                    "dirent_in_non_directory",
                    format!("{name:?} in inode {}", d.dir),
                ),
                None => add(
                    "dirent_in_missing_directory",
                    format!("{name:?} in inode {}", d.dir),
                ),
            }
            // An entry naming a subvolume: its id in the low half of the
            // target (S3 + S4: the kernel oracle's `sv` and `snap`).
            if d.d_type == DT_SUBVOL {
                let id = d.inum as u32;
                if !subvolumes.contains_key(&id) {
                    add(
                        "dirent_to_missing_subvolume",
                        format!("{name:?} in {} names subvolume {id}", d.dir),
                    );
                }
                continue;
            }
            match inode_seen(&c, d.inum) {
                None => add(
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
                        add(
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
        // Link counts and reachability only mean something when both
        // btrees were read whole; a btree that failed is already reported.
        for ino in inode_keys.keys().filter(|_| both) {
            let Some(i) = inode_seen(&c, *ino) else {
                continue;
            };
            let want = if i.is_dir() {
                subdirs.get(ino).copied().unwrap_or(0) + 2
            } else {
                links.get(ino).copied().unwrap_or(0)
            };
            let root = roots.contains(ino);
            if !root && links.get(ino).copied().unwrap_or(0) == 0 {
                add(
                    "unreachable_inode",
                    format!("inode {ino} has no directory entry"),
                );
            } else if i.link_count() != want {
                add(
                    "link_count",
                    format!("inode {ino} counts {} links; {want} found", i.link_count()),
                );
            }
        }

        // Back-references (#163): an inode one entry names says which, in
        // bi_dir and bi_dir_offset, and that entry names it back. A file
        // with several names, a subvolume's root and an inode with no
        // back-reference at all are not judged.
        for (ino, ks) in inode_keys.iter().filter(|_| both) {
            if subvolume_roots.contains(ino) || roots.contains(ino) {
                continue;
            }
            let Some(k) = nearest(&c, ks) else { continue };
            let Some(i) = decoded.get(&k.pos) else {
                continue;
            };
            if !i.is_dir() && i.link_count() != 1 {
                continue;
            }
            let Ok(raw) = InodeV3Raw::parse(&k.value) else {
                continue;
            };
            let dir = raw.field("dir").unwrap_or(0);
            let off = raw.field("dir_offset").unwrap_or(0);
            if dir == 0 {
                continue;
            }
            let entry = dirent_keys
                .get(&(dir, off))
                .and_then(|ks| nearest(&c, ks))
                .filter(|d| d.key_type == key_type::DIRENT)
                .and_then(|d| Dirent::from_key(d).ok());
            match entry {
                None => add(
                    "inode_points_to_missing_dirent",
                    format!("inode {ino} names entry {off} of directory {dir}, which is not there"),
                ),
                Some(d) if d.d_type != DT_SUBVOL && d.inum != *ino => add(
                    "inode_points_to_wrong_dirent",
                    format!(
                        "inode {ino} names entry {off} of directory {dir}, which names inode {}",
                        d.inum
                    ),
                ),
                Some(_) => {}
            }
        }

        // Xattrs (#163): each one's inode is there, and the xattr is
        // reachable from its name's hash slot along a run of taken slots,
        // as a lookup walks it (S8, src/xattr.rs).
        for ((owner_ino, off), ks) in &xattr_keys {
            let Some(k) = nearest(&c, ks) else { continue };
            if k.key_type != key_type::XATTR {
                continue;
            }
            let Some(owner) = inode_seen(&c, *owner_ino) else {
                add(
                    "key_in_missing_inode",
                    format!("xattr {} of inode {owner_ino}, which is not there", k.pos),
                );
                continue;
            };
            let v = &k.value;
            if v.len() < 4 + v.get(1).copied().unwrap_or(0) as usize {
                continue;
            }
            let short = &v[4..4 + v[1] as usize];
            let Some(slot) = crate::xattr::slot(owner.hash_type(), owner.hash_seed, v[0], short)
            else {
                continue;
            };
            let taken = |o: u64| {
                xattr_keys
                    .get(&(*owner_ino, o))
                    .and_then(|ks| nearest(&c, ks))
                    .is_some_and(|k| {
                        matches!(k.key_type, key_type::XATTR | key_type::HASH_WHITEOUT)
                    })
            };
            let reachable =
                slot <= *off && *off - slot <= xattr_keys.len() as u64 && (slot..*off).all(taken);
            if !reachable {
                add(
                    "hash_table_key_wrong_offset",
                    format!(
                        "xattr {} is at offset {off}, and its name hashes to {slot} with an \
                         empty slot between",
                        k.pos
                    ),
                );
            }
        }
    }
    r.problems.extend(found);

    // Pointers are judged against the allocator: the bucket's generation
    // from its alloc_v4 key, this device's index, and no flags.
    let bucket_size = sb
        .members()
        .ok()
        .and_then(|m| m.get(sb.dev_idx as usize).map(|d| u64::from(d.bucket_size)))
        .filter(|&b| b > 0);
    let gens: BTreeMap<u64, u8> = trees
        .get(&btree_id::ALLOC)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == key_type::ALLOC_V4 && k.pos.inode == u64::from(sb.dev_idx))
        .filter_map(|k| crate::extent::alloc_v4_gen(&k.value).map(|g| (k.pos.offset, g)))
        .collect();
    let have_alloc = trees.contains_key(&btree_id::ALLOC) && bucket_size.is_some();

    // Every bucket's generation as bucket_gens records it, against its
    // alloc key's; a bucket with no alloc key is at generation 0 (#163: the
    // reference's "incorrect gen in bucket_gens").
    if trees.contains_key(&btree_id::ALLOC) {
        for k in trees
            .get(&BUCKET_GENS)
            .into_iter()
            .flatten()
            .filter(|k| k.key_type == KEY_BUCKET_GENS && k.pos.inode == u64::from(sb.dev_idx))
        {
            for (i, &recorded) in k.value.iter().take(256).enumerate() {
                let bucket = k.pos.offset << 8 | i as u64;
                let alloc = gens.get(&bucket).copied().unwrap_or(0);
                if recorded != alloc {
                    r.add(
                        "incorrect_gen_in_bucket_gens",
                        format!(
                            "bucket {bucket}: bucket_gens says generation {recorded}, its alloc \
                             key {alloc}"
                        ),
                    );
                }
            }
        }
    }

    // Extents: their inode, their bounds, their neighbours, their data.
    let have_inodes = trees.contains_key(&btree_id::INODES);
    // (inode, snapshot, end sector) of the last extent-like key seen: an
    // extent that starts before it ends overlaps it (S1's check_extents,
    // "no overlaps"; issue #60).
    let mut prev: Option<(u64, u32, u64)> = None;
    // Every `reflink_p`: where it is, and the reflink btree sectors
    // `idx..idx + size` it shows (#139).
    let mut pointers: Vec<(Bpos, u64, u64)> = Vec::new();
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
        if k.key_type == key_type::REFLINK_P {
            match crate::extent::reflink_p_idx(&k.value) {
                Ok(idx) => pointers.push((k.pos, idx, idx + u64::from(k.size))),
                Err(e) => r.add("reflink", format!("extent {}: {e}", k.pos)),
            }
        }
        if matches!(
            k.key_type,
            key_type::EXTENT
                | key_type::INLINE_DATA
                | key_type::RESERVATION
                | key_type::ERROR
                | key_type::REFLINK_P
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
        let Some(i) = inode_seen(&chain(k.pos.snapshot), k.pos.inode) else {
            r.add("extent_without_inode", format!("extent {}", k.pos));
            continue;
        };
        // In sectors: in bytes an extent starting 2^55 sectors in overflows
        // (#150), and a wrapped product would judge it inside its file.
        if k.start_offset() >= i.size.div_ceil(512) {
            r.add(
                "extent_past_end_of_inode",
                format!("extent {} of inode {} (size {})", k.pos, i.ino, i.size),
            );
        }
        if k.key_type == key_type::EXTENT {
            let e = match DataExtent::from_key(k) {
                Ok(e) => e,
                Err(e) => {
                    r.add("extent_data", format!("extent {}: {e}", k.pos));
                    continue;
                }
            };
            if have_alloc {
                let gen = bucket_size.and_then(|b| gens.get(&(e.ptr.offset / b)).copied());
                if let Err(err) = e.ptr.check(sb.dev_idx, gen) {
                    r.add("extent_pointer", format!("extent {}: {err}", k.pos));
                    continue;
                }
            }
            // A poisoned extent's data is known to be bad, and the
            // filesystem says so: the reference checker passes an image
            // holding one (S8, the `poison` image), and so does this one.
            // An unwritten one has no data to check (#158).
            if matches!(crate::extent::poisoned(&k.value), Ok(true)) || e.ptr.is_unwritten() {
                continue;
            }
            if let Err(e) = verify_data(dev, &e) {
                r.add("extent_data", format!("extent {}: {e}", k.pos));
            }
        }
    }

    // Reflinks (#139): every `reflink_p` lands on indirect extents that
    // cover its whole range, and each indirect extent's refcount is the
    // number of pointers into it (S1 9.1.6: the count of references; the
    // kernel oracle's clone pair, two pointers each, passes the reference
    // checker). Each indirect extent's data is checked as an extent's is.
    let indirect: Vec<&Bkey> = trees
        .get(&btree_id::REFLINK)
        .into_iter()
        .flatten()
        .filter(|k| k.key_type == key_type::REFLINK_V)
        .collect();
    let mut refs = vec![0u64; indirect.len()];
    for (pos, idx, end) in &pointers {
        let mut at = *idx;
        for (n, v) in indirect.iter().enumerate() {
            if v.start_offset() < *end && v.pos.offset > *idx {
                refs[n] += 1;
                if v.start_offset() <= at {
                    at = at.max(v.pos.offset);
                }
            }
        }
        if at < *end {
            r.add(
                "reflink_missing",
                format!(
                    "extent {pos}: the reflink btree has nothing at sector {at}, which it points \
                     into"
                ),
            );
        }
    }
    for (v, n) in indirect.iter().zip(refs) {
        if v.value.len() < 8 {
            r.add("reflink", format!("reflink {}: no refcount", v.pos));
            continue;
        }
        let count = le64(&v.value, 0);
        if count != n {
            r.add(
                "reflink_refcount",
                format!(
                    "reflink {}: refcount {count}; {n} pointers into it found",
                    v.pos
                ),
            );
        }
        let e = match DataExtent::from_key(&Bkey {
            key_type: key_type::EXTENT,
            value: v.value[8..].to_vec(),
            ..(*v).clone()
        }) {
            Ok(e) => e,
            Err(e) => {
                r.add("reflink", format!("reflink {}: {e}", v.pos));
                continue;
            }
        };
        if have_alloc {
            let gen = bucket_size.and_then(|b| gens.get(&(e.ptr.offset / b)).copied());
            if let Err(err) = e.ptr.check(sb.dev_idx, gen) {
                r.add("extent_pointer", format!("reflink {}: {err}", v.pos));
                continue;
            }
        }
        if let Err(e) = verify_data(dev, &e) {
            r.add("extent_data", format!("reflink {}: {e}", v.pos));
        }
    }
    Ok(r)
}

/// Of the versions of one key, the one a snapshot whose chain is `chain`
/// sees: the one at the snapshot nearest it, a whiteout included (the
/// caller reads a whiteout as nothing there).
fn nearest<'a>(chain: &[u32], keys: &[&'a Bkey]) -> Option<&'a Bkey> {
    keys.iter()
        .filter_map(|k| Some((chain.iter().position(|&s| s == k.pos.snapshot)?, *k)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, k)| k)
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
