//! The volume, primitive and file layers on rust-fs-core's shared traits
//! (`fs_core::layers`, docs/three-layers.md).
//!
//! [`Volume::open`] is the strict open, ready for the file API;
//! [`Volume::open_for_salvage`] needs only a superblock copy whose header
//! parses, and offers the primitive layer without a namespace. Everything
//! reads the source as a `BlockRead`.

use crate::error::{Error, Result};
use crate::fs::{Filesystem, Node};
use crate::primitive::{self, SuperblockSurvey, Verdict};
use crate::salvage::adapter::{shared_damage, BcachefsSalvage};
use crate::salvage::{DamageSource, Salvage};
use crate::superblock::{Superblock, LAYOUT_OFFSET, SB_HEADER_BYTES};
use fs_core::layers::file::{DirEntry, FileAPI, Metadata, NodeKind, Timestamp};
use fs_core::layers::primitive::{PrimitiveAPI, Structure, StructureRole};
use fs_core::layers::salvage::{Damage, DamageKind, Extent, Integrity, SalvageError, Severity};
use fs_core::layers::volume::{
    check_bounds, ApiError, Geometry, OpenVolume, Probe, Recognition, SuperblockCopy, VolumeAPI,
    VolumeHealth, VolumeSupport,
};
use fs_core::BlockRead;

/// The format's name in the shared layers.
pub const FORMAT: &str = "bcachefs";

impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::NotFound(p) => Error::NotFound(p),
            ApiError::Unsupported(what) => Error::Unsupported(what.into()),
            ApiError::Bounds {
                offset,
                length,
                size,
            } => Error::Corrupt(format!(
                "{length} byte(s) at {offset} lie outside the {size}-byte volume"
            )),
            other => Error::Corrupt(other.to_string()),
        }
    }
}

/// Directory listings by `(inode, snapshot)`.
type Listings = std::collections::HashMap<(u64, u32), Vec<DirEntry<Node>>>;

enum Inner<D: BlockRead> {
    /// Opened strictly: the file API reads through it.
    Strict(Box<Filesystem<D>>),
    /// Opened for salvage only.
    Raw(D),
}

/// A bcachefs volume on the shared layers.
pub struct Volume<D: BlockRead> {
    inner: Inner<D>,
    survey: SuperblockSurvey,
    /// The chosen valid copy, or for a salvage open with none valid, the
    /// newest copy whose header parses.
    sb: Superblock,
    adapter: BcachefsSalvage,
    /// Directory listings already made: a path lookup lists every directory
    /// on its way, and nothing here changes the volume.
    listings: std::sync::Mutex<Listings>,
}

impl<D: BlockRead> Volume<D> {
    fn dev(&self) -> &dyn BlockRead {
        match &self.inner {
            Inner::Strict(fs) => fs.device(),
            Inner::Raw(d) => d,
        }
    }

    /// The strict reader, when the volume was opened strictly.
    pub fn filesystem(&self) -> Option<&Filesystem<D>> {
        match &self.inner {
            Inner::Strict(fs) => Some(fs),
            Inner::Raw(_) => None,
        }
    }

    /// The superblock survey made at open: every copy with its verdict.
    pub fn superblock_survey(&self) -> &SuperblockSurvey {
        &self.survey
    }

    /// The bcachefs salvage adapter, with its format-specific verbs.
    pub fn bcachefs_salvage(&self) -> &BcachefsSalvage {
        &self.adapter
    }

    fn fs(&self) -> Result<&Filesystem<D>> {
        self.filesystem().ok_or_else(|| {
            Error::Unsupported("the volume was opened for salvage: it has no namespace".into())
        })
    }

    fn bucket_bytes(&self) -> u64 {
        self.member()
            .map(|m| u64::from(m.bucket_size) * 512)
            .unwrap_or(0)
    }

    fn member(&self) -> Option<crate::superblock::Member> {
        self.sb
            .members()
            .ok()?
            .into_iter()
            .nth(self.sb.dev_idx as usize)
    }

    fn build(source: D, strict: bool) -> Result<Self> {
        let survey = primitive::survey_superblocks(&source);
        let sb = match &survey.superblock {
            Some(sb) => sb.clone(),
            None => newest_parsed(&source, &survey)
                .ok_or_else(|| Error::Corrupt("no superblock copy on the device parses".into()))?,
        };
        refuse(&sb)?;
        let adapter = match Salvage::with_superblock(&source, sb.clone()) {
            Ok(s) => BcachefsSalvage::from_native(s),
            Err(e) => return Err(e),
        };
        let inner = if strict {
            Inner::Strict(Box::new(Filesystem::open(source)?))
        } else {
            Inner::Raw(source)
        };
        Ok(Volume {
            inner,
            survey,
            sb,
            adapter,
            listings: Default::default(),
        })
    }
}

/// The newest copy whose header parses, its checksum failing: enough to
/// know the node magic and geometry for salvage.
fn newest_parsed(dev: &dyn BlockRead, survey: &SuperblockSurvey) -> Option<Superblock> {
    survey
        .copies
        .iter()
        .filter(|c| c.seq.is_some())
        .max_by_key(|c| c.seq)
        .and_then(|c| {
            let bytes = Superblock::read_bytes_at(dev, c.sector).ok()?;
            Superblock::parse_unchecked(&bytes).ok()
        })
}

/// Encrypted and multi-device volumes are recognised and refused, naming
/// which.
fn refuse(sb: &Superblock) -> Result<()> {
    if let Some(crypt) = sb.crypt()? {
        return Err(Error::Unsupported(if crypt.is_sealed() {
            "the volume is encrypted, its master key sealed by a passphrase".into()
        } else {
            "the volume is encrypted (its master key stored unsealed)".into()
        }));
    }
    if sb.nr_devices != 1 {
        return Err(Error::Unsupported(format!(
            "the volume spans {} devices: only single-device volumes are read",
            sb.nr_devices
        )));
    }
    Ok(())
}

fn integrity(v: &Verdict) -> Integrity {
    if v.is_valid() {
        Integrity::Verified
    } else {
        Integrity::Failed
    }
}

impl<D: BlockRead> VolumeAPI for Volume<D> {
    type Error = Error;
    type Superblock = Superblock;

    fn format(&self) -> &'static str {
        FORMAT
    }

    fn size_bytes(&self) -> u64 {
        self.dev().size_bytes()
    }

    fn read_volume_bytes(&self, offset: u64, output: &mut [u8]) -> Result<()> {
        check_bounds(offset, output.len(), self.size_bytes())?;
        self.dev().read_at(offset, output)?;
        Ok(())
    }

    /// Buckets are the allocation unit; bcachefs maps them through its
    /// btrees, so there is no linear cluster layout.
    fn geometry(&self) -> Result<Geometry> {
        let m = self.member();
        let bucket = self.bucket_bytes();
        let label = self.sb.label_str();
        Ok(Geometry {
            sector_size: 512,
            allocation_unit: bucket.max(512),
            allocation_units: m.as_ref().map(|m| m.nbuckets).unwrap_or(0),
            declared_bytes: m.map(|m| m.nbuckets.saturating_mul(bucket)).unwrap_or(0),
            layout: None,
            label: (!label.is_empty()).then_some(label),
            serial: Some(crate::superblock::format_uuid(&self.sb.user_uuid)),
        })
    }

    fn superblock(&self) -> Result<Superblock> {
        Ok(self.sb.clone())
    }

    /// The primary at sector 8 is copy 0; the others follow in the order
    /// the layouts name them.
    fn superblocks(&self) -> Result<Vec<SuperblockCopy<Superblock>>> {
        let dev = self.dev();
        Ok(self
            .survey
            .copies
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let decoded = Superblock::read_bytes_at(dev, c.sector)
                    .ok()
                    .and_then(|b| Superblock::parse_unchecked(&b).ok());
                let length = decoded
                    .as_ref()
                    .map(|s| SB_HEADER_BYTES as u64 + u64::from(s.u64s) * 8)
                    .unwrap_or(SB_HEADER_BYTES as u64);
                SuperblockCopy {
                    copy: i as u32,
                    location: Extent::new(c.sector.saturating_mul(512), length),
                    superblock: decoded,
                    integrity: integrity(&c.verdict),
                }
            })
            .collect())
    }

    fn support(&self) -> Result<VolumeSupport> {
        Ok(VolumeSupport {
            files: self.filesystem().is_some(),
            salvage: crate::salvage::adapter::capabilities(),
            unsupported: Vec::new(),
        })
    }

    /// From what open already read: superblock copies, the live tree's
    /// breaks and the clean flag. A full damage report is the adapter's.
    fn health(&self) -> Result<VolumeHealth> {
        let mut findings = Vec::new();
        for c in self.survey.damaged() {
            findings.push(Damage {
                kind: match c.verdict {
                    Verdict::BadMagic => DamageKind::Signature,
                    Verdict::BadChecksum { .. } => DamageKind::Checksum,
                    Verdict::Unreadable(_) => DamageKind::Unreadable,
                    _ => DamageKind::Malformed,
                },
                severity: if self.survey.chosen.is_some() {
                    Severity::Warning
                } else {
                    Severity::Error
                },
                location: Extent::new(c.sector.saturating_mul(512), SB_HEADER_BYTES as u64),
                structure: "superblock".into(),
                detail: format!("the copy at sector {}: {}", c.sector, c.verdict),
            });
        }
        if let Some(s) = self.adapter.native() {
            for (offset, why) in &s.live_nodes().breaks {
                findings.push(shared_damage(&crate::salvage::Damage {
                    source: DamageSource::LiveTree,
                    offset: *offset,
                    length: 0,
                    verdict: Verdict::Corrupt(why.clone()),
                    detail: format!("a live-tree pointer: {why}"),
                }));
            }
        }
        if !self.sb.is_clean() {
            findings.push(Damage {
                kind: DamageKind::Recorded,
                severity: Severity::Notice,
                location: Extent::new(crate::superblock::SB_OFFSET, SB_HEADER_BYTES as u64),
                structure: "superblock".into(),
                detail: "not shut down cleanly: the journal must be replayed".into(),
            });
        }
        Ok(VolumeHealth::from_findings(findings))
    }
}

impl<D: BlockRead> OpenVolume for Volume<D> {
    type Source = D;

    fn probe(source: &dyn BlockRead) -> std::result::Result<Probe, fs_core::Error> {
        let survey = primitive::survey_superblocks(source);
        let recognition = if survey.chosen.is_some() {
            Recognition::Yes
        } else if survey.copies.iter().any(|c| c.seq.is_some())
            || survey.standalone_layout.verdict.is_valid()
        {
            Recognition::Damaged
        } else {
            Recognition::No
        };
        let declared_bytes = survey.superblock.as_ref().and_then(|sb| {
            let m = sb.members().ok()?.into_iter().nth(sb.dev_idx as usize)?;
            Some(m.nbuckets.saturating_mul(u64::from(m.bucket_size) * 512))
        });
        let detail = match recognition {
            Recognition::Yes => format!("superblock copy at sector {}", survey.chosen.unwrap_or(0)),
            Recognition::Damaged => {
                "no superblock copy validates; a header or layout parses".into()
            }
            Recognition::No => "no bcachefs superblock".into(),
        };
        Ok(Probe {
            format: FORMAT,
            recognition,
            declared_bytes,
            detail,
        })
    }

    fn open(source: D) -> Result<Self> {
        Self::build(source, true)
    }

    fn open_for_salvage(source: D) -> Result<Self> {
        Self::build(source, false)
    }
}

impl<D: BlockRead> PrimitiveAPI for Volume<D> {
    type Adapter = BcachefsSalvage;
    /// No hypotheses: a salvage open tries every superblock copy itself.
    type Hypothesis = ();

    fn source(&self) -> &dyn BlockRead {
        self.dev()
    }

    fn adapter(&self) -> std::result::Result<BcachefsSalvage, SalvageError> {
        Ok(self.adapter.clone())
    }

    /// The superblock copies, the layout at sector 7, the journal buckets
    /// and the live btree roots.
    fn structures(&self) -> std::result::Result<Vec<Structure>, SalvageError> {
        let mut out = Vec::new();
        for (i, c) in self
            .superblocks()
            .map_err(|_| SalvageError::NoGeometry)?
            .iter()
            .enumerate()
        {
            out.push(Structure {
                name: format!("superblock copy at sector {}", c.location.offset / 512),
                role: StructureRole::Superblock,
                location: c.location,
                copy: i as u32,
                integrity: c.integrity,
            });
        }
        out.push(Structure {
            name: "superblock layout".into(),
            role: StructureRole::Superblock,
            location: Extent::new(LAYOUT_OFFSET, crate::superblock::Layout::BYTES as u64),
            copy: 0,
            integrity: integrity(&self.survey.standalone_layout.verdict),
        });
        let journal = primitive::survey_journal(self.dev(), &self.sb);
        let bucket = self.bucket_bytes();
        for (i, &b) in journal.buckets.iter().enumerate() {
            let bad = journal.unreadable.contains(&b)
                || journal.entries.iter().any(|e| {
                    e.sector * 512 >= b && e.sector * 512 < b + bucket && !e.verdict.is_valid()
                });
            out.push(Structure {
                name: format!("journal bucket {i}"),
                role: StructureRole::Journal,
                location: Extent::new(b, bucket),
                copy: 0,
                integrity: if bad {
                    Integrity::Failed
                } else {
                    Integrity::Verified
                },
            });
        }
        if let Some(s) = self.adapter.native() {
            let ctx = s.node_context();
            let mut roots: Vec<(u8, u8, u64)> = Vec::new();
            for (&offset, n) in &s.live_nodes().nodes {
                let top = s
                    .live_nodes()
                    .nodes
                    .values()
                    .filter(|m| m.btree_id == n.btree_id)
                    .map(|m| m.level)
                    .max();
                if Some(n.level) == top {
                    roots.push((n.btree_id, n.level, offset));
                }
            }
            for (id, level, offset) in roots {
                let ok = primitive::inspect_node(self.dev(), ctx, offset)
                    .ok()
                    .flatten()
                    .is_some_and(|n| n.is_valid());
                out.push(Structure {
                    name: format!(
                        "btree root: {} (level {level})",
                        crate::journal::btree_name(id)
                    ),
                    role: StructureRole::RecordTable,
                    location: Extent::new(offset, ctx.node_bytes as u64),
                    copy: 0,
                    integrity: if ok {
                        Integrity::Verified
                    } else {
                        Integrity::Failed
                    },
                });
            }
        }
        let size = self.size_bytes();
        for s in &mut out {
            if s.location.end().is_some_and(|e| e > size) {
                s.location.length = size.saturating_sub(s.location.offset);
            }
        }
        Ok(out)
    }
}

fn kind(mode: u32) -> NodeKind {
    match mode & 0o170000 {
        0o040000 => NodeKind::Directory,
        0o100000 => NodeKind::File,
        0o120000 => NodeKind::Symlink,
        _ => NodeKind::Special,
    }
}

/// A stored inode time, in the superblock's time units from its time base.
fn time(sb: &Superblock, raw: u64) -> Timestamp {
    let (seconds, nanoseconds) = crate::capi::epoch_time(sb, raw);
    Timestamp {
        seconds,
        nanoseconds,
    }
}

impl<D: BlockRead> FileAPI for Volume<D> {
    type Node = Node;

    fn root(&self) -> Result<Node> {
        Ok(self.fs()?.root())
    }

    fn read_dir(&self, directory: &Node) -> Result<Vec<DirEntry<Node>>> {
        let key = (directory.ino, directory.snapshot);
        if let Some(hit) = self.listings.lock().ok().and_then(|l| l.get(&key).cloned()) {
            return Ok(hit);
        }
        let fs = self.fs()?;
        let inode = fs.inode_at(*directory)?;
        if !inode.is_dir() {
            return Err(ApiError::NotADirectory(format!("inode {}", directory.ino)).into());
        }
        let mut out = Vec::new();
        for d in fs.readdir_at(*directory)? {
            let node = fs.child(*directory, &d)?;
            let child = fs.inode_at(node)?;
            let (name, lossy) = match String::from_utf8(d.name.clone()) {
                Ok(n) => (n, false),
                Err(_) => (String::from_utf8_lossy(&d.name).into_owned(), true),
            };
            out.push(DirEntry {
                bytes: d.name.clone(),
                name,
                lossy,
                kind: kind(child.mode),
                size: if child.is_dir() { 0 } else { child.size },
                node,
            });
        }
        if let Ok(mut l) = self.listings.lock() {
            l.insert(key, out.clone());
        }
        Ok(out)
    }

    fn metadata(&self, node: &Node) -> Result<Metadata> {
        let i = self.fs()?.inode_at(*node)?;
        let mut m = Metadata::new(kind(i.mode), if i.is_dir() { 0 } else { i.size });
        m.allocated = Some(i.sectors.saturating_mul(512));
        m.modified = Some(time(&self.sb, i.mtime));
        m.accessed = Some(time(&self.sb, i.atime));
        m.mode = Some(i.mode);
        m.links = Some(u64::from(i.link_count()));
        Ok(m)
    }

    fn read_at(&self, file: &Node, offset: u64, output: &mut [u8]) -> Result<usize> {
        let fs = self.fs()?;
        if fs.inode_at(*file)?.is_dir() {
            return Err(ApiError::IsADirectory(format!("inode {}", file.ino)).into());
        }
        let data = fs.read_range_at(*file, offset, output.len())?;
        output[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    fn read_link(&self, link: &Node) -> Result<Vec<u8>> {
        let fs = self.fs()?;
        if !fs.inode_at(*link)?.is_symlink() {
            return Err(ApiError::Unsupported("not a symbolic link").into());
        }
        fs.read_at(*link)
    }
}
