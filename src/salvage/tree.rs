//! Rebuild the tree: every directory entry found, as parent claims.

use super::{Candidate, Salvage};
use crate::bkey::key_type;
use crate::btree::btree_id;
use crate::inode::{Dirent, ROOT_INO};
use fs_core::BlockRead;
use std::collections::BTreeMap;

/// A directory entry that names an inode as its child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentClaim {
    /// The directory's inode number.
    pub dir: u64,
    pub name: Vec<u8>,
    /// The entry's snapshot.
    pub snapshot: u32,
    /// Whether the live tree holds this entry.
    pub live: bool,
    pub checksum_ok: bool,
    pub node_offset: u64,
}

/// One candidate and every entry that claims it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub ino: u64,
    pub snapshot: u32,
    /// Whether the candidate is a directory, and its link count: a file may
    /// rightly have as many entries as links, a directory only one.
    pub is_dir: bool,
    pub nlink: u32,
    pub parents: Vec<ParentClaim>,
}

impl TreeNode {
    /// No entry names it, and it is not the root directory.
    pub fn is_orphan(&self) -> bool {
        self.parents.is_empty() && self.ino != ROOT_INO
    }

    /// The entries that name it disagree: more distinct `(directory, name)`
    /// pairs than it can have, among the live ones or, with none live, among
    /// all. A directory can have one; a file as many as its link count.
    pub fn is_conflict(&self) -> bool {
        let live: Vec<_> = self.parents.iter().filter(|p| p.live).collect();
        let pool: Vec<&ParentClaim> = if live.is_empty() {
            self.parents.iter().collect()
        } else {
            live
        };
        let mut distinct: Vec<(u64, &[u8])> =
            pool.iter().map(|p| (p.dir, p.name.as_slice())).collect();
        distinct.sort();
        distinct.dedup();
        let allowed = if self.is_dir {
            1
        } else {
            self.nlink.max(1) as usize
        };
        distinct.len() > allowed
    }
}

/// Every candidate with the parent claims found for it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    pub nodes: Vec<TreeNode>,
    /// Device ranges that failed to read while entries were gathered.
    pub unreadable: Vec<(u64, u64)>,
}

impl Tree {
    pub fn orphans(&self) -> impl Iterator<Item = &TreeNode> {
        self.nodes.iter().filter(|n| n.is_orphan())
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &TreeNode> {
        self.nodes.iter().filter(|n| n.is_conflict())
    }
}

impl Salvage {
    /// Every directory entry on the device, from every node version, as
    /// claims on the candidates it names. An entry claims a candidate when
    /// it names its inode number and its snapshot is the candidate's or one
    /// of its ancestors. Entries naming a subvolume rather than an inode are
    /// left out: their number is a subvolume's.
    pub fn rebuild_tree(&self, dev: &dyn BlockRead, candidates: &[Candidate]) -> Tree {
        let (found, unreadable) = self.all_keys(dev, btree_id::DIRENTS);
        let mut by_child: BTreeMap<u64, Vec<ParentClaim>> = BTreeMap::new();
        for k in found {
            if k.key.key_type != key_type::DIRENT {
                continue;
            }
            let Ok(d) = Dirent::from_key(&k.key) else {
                continue;
            };
            if d.d_type == DT_SUBVOL {
                continue;
            }
            let claim = ParentClaim {
                dir: d.dir,
                name: d.name,
                snapshot: k.key.pos.snapshot,
                live: k.live,
                checksum_ok: k.checksum_ok,
                node_offset: k.node_offset,
            };
            let claims = by_child.entry(d.inum).or_default();
            if !claims.contains(&claim) {
                claims.push(claim);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut nodes = Vec::new();
        for c in candidates {
            if !seen.insert((c.ino, c.snapshot)) {
                continue;
            }
            let chain = self.snapshot_chain(c.snapshot);
            let parents = by_child
                .get(&c.ino)
                .map(|claims| {
                    claims
                        .iter()
                        .filter(|p| {
                            chain.contains(&p.snapshot)
                                || self.snapshot_chain(p.snapshot).contains(&c.snapshot)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            nodes.push(TreeNode {
                ino: c.ino,
                snapshot: c.snapshot,
                is_dir: c.is_dir(),
                nlink: c.nlink,
                parents,
            });
        }
        Tree { nodes, unreadable }
    }
}

/// The directory-entry type of an entry naming a subvolume.
const DT_SUBVOL: u8 = 16;
