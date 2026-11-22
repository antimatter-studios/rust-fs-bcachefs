//! The salvage verbs on rust-fs-core's shared `SalvageAdapter` trait.

use super::{Candidate as NativeCandidate, CandidateState as NativeState, PieceKind, Salvage};
use crate::inode::ROOT_INO;
use crate::primitive::{self, Verdict};
use fs_core::layers::salvage::{
    Candidate, CandidateKind, CandidateState, Capabilities, Damage, DamageKind, DamageReport,
    Encoding, Extent, ExtentWalk, Feature, Integrity, Parent, ParentClaim, SalvageAdapter,
    SalvageError, ScanReport, Severity, Sink, Tree, TreeNode, WalkEnd, WalkExtent,
};
use fs_core::BlockRead;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a bcachefs candidate carries beyond the shared fields: which inode,
/// at which snapshot, and where its key was found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InodeDetail {
    pub ino: u64,
    pub snapshot: u32,
    /// Byte offset of the btree node holding the key, its sequence number,
    /// the bset within it and that bset's journal sequence.
    pub node_offset: u64,
    pub node_seq: u64,
    pub bset: u32,
    pub journal_seq: u64,
    pub mode: u32,
    pub nlink: u32,
    /// Modification time as stored (the filesystem's time units).
    pub mtime: u64,
    /// The key is not the live tree's current one: an older version of a
    /// live inode (state `Unknown`), or of one since deleted (`Deleted`).
    pub superseded: bool,
}

/// The salvage adapter for one bcachefs volume: the native [`Salvage`] when
/// the source held a usable superblock, or why it did not.
#[derive(Debug, Clone)]
pub struct BcachefsSalvage {
    inner: Result<Salvage, String>,
}

impl BcachefsSalvage {
    /// Build the adapter from whatever the source holds. Never fails: a
    /// source with no usable superblock, or an encrypted or multi-device
    /// one, gives an adapter whose verbs refuse with `NoGeometry`, and
    /// [`BcachefsSalvage::refusal`] says why.
    pub fn new(source: &dyn BlockRead) -> Self {
        BcachefsSalvage {
            inner: Salvage::open(source).map_err(|e| e.to_string()),
        }
    }

    pub fn from_native(salvage: Salvage) -> Self {
        BcachefsSalvage { inner: Ok(salvage) }
    }

    /// The native adapter, with the bcachefs-specific verbs.
    pub fn native(&self) -> Option<&Salvage> {
        self.inner.as_ref().ok()
    }

    /// Why the verbs refuse, when they do.
    pub fn refusal(&self) -> Option<&str> {
        self.inner.as_ref().err().map(String::as_str)
    }

    fn salvage(&self) -> Result<&Salvage, SalvageError> {
        self.inner.as_ref().map_err(|_| SalvageError::NoGeometry)
    }
}

/// The source range `range` names, cut to the source; `Bounds` when it
/// starts past the end or is not representable.
fn clip(range: Extent, size: u64) -> Result<(u64, u64), SalvageError> {
    let end = range.end().ok_or(SalvageError::Bounds)?;
    if range.offset > size || (range.offset == size && range.length > 0) {
        return Err(SalvageError::Bounds);
    }
    Ok((range.offset, end.min(size)))
}

fn shared(c: &NativeCandidate) -> Candidate<InodeDetail> {
    Candidate {
        location: Extent::new(c.key_offset, c.key_bytes),
        kind: if c.is_dir() {
            CandidateKind::Directory
        } else if c.is_file() {
            CandidateKind::File
        } else {
            CandidateKind::Other
        },
        state: match c.state {
            NativeState::Live => CandidateState::Live,
            NativeState::Superseded => CandidateState::Unknown,
            NativeState::Deleted => CandidateState::Deleted,
        },
        integrity: if c.checksum_ok {
            Integrity::Verified
        } else {
            Integrity::Failed
        },
        name: None,
        size: Some(c.size),
        detail: InodeDetail {
            ino: c.ino,
            snapshot: c.snapshot,
            node_offset: c.node_offset,
            node_seq: c.node_seq,
            bset: c.bset as u32,
            journal_seq: c.journal_seq,
            mode: c.mode,
            nlink: c.nlink,
            mtime: c.mtime,
            superseded: c.state != NativeState::Live,
        },
    }
}

fn native(c: &Candidate<InodeDetail>) -> NativeCandidate {
    let d = &c.detail;
    NativeCandidate {
        ino: d.ino,
        snapshot: d.snapshot,
        state: match c.state {
            CandidateState::Live => NativeState::Live,
            CandidateState::Deleted => NativeState::Deleted,
            _ => NativeState::Superseded,
        },
        checksum_ok: c.integrity == Integrity::Verified,
        node_offset: d.node_offset,
        node_seq: d.node_seq,
        bset: d.bset as usize,
        journal_seq: d.journal_seq,
        key_offset: c.location.offset,
        key_bytes: c.location.length,
        mode: d.mode,
        size: c.size.unwrap_or(0),
        nlink: d.nlink,
        mtime: d.mtime,
    }
}

fn damage_kind(v: &Verdict) -> DamageKind {
    match v {
        Verdict::Valid => DamageKind::Recorded,
        Verdict::BadMagic => DamageKind::Signature,
        Verdict::BadChecksum { .. } => DamageKind::Checksum,
        Verdict::Corrupt(_) | Verdict::Unsupported(_) => DamageKind::Malformed,
        Verdict::Unreadable(_) => DamageKind::Unreadable,
    }
}

/// The shared form of a native damage record. A torn journal entry, a
/// damaged older node version and a damaged superblock copy beside valid
/// ones lose nothing live: notices and warnings. What the live tree needs
/// is an error.
pub(crate) fn shared_damage(d: &super::Damage) -> Damage {
    use super::DamageSource as S;
    let (kind, severity) = match d.source {
        S::SuperblockCopy | S::SuperblockLayout => (damage_kind(&d.verdict), Severity::Warning),
        S::SuperblockDisagreement => (DamageKind::Mismatch, Severity::Warning),
        S::JournalEntry => (damage_kind(&d.verdict), Severity::Notice),
        S::JournalBucket => (damage_kind(&d.verdict), Severity::Warning),
        S::BtreeNode if d.detail.ends_with("(live)") => (damage_kind(&d.verdict), Severity::Error),
        S::BtreeNode => (damage_kind(&d.verdict), Severity::Notice),
        S::LiveTree => (DamageKind::Broken, Severity::Error),
        S::Unreadable => (DamageKind::Unreadable, Severity::Error),
    };
    Damage {
        kind,
        severity,
        location: Extent::new(d.offset, d.length),
        structure: format!("{:?}", d.source),
        detail: format!("{}: {}", d.detail, d.verdict),
    }
}

impl BcachefsSalvage {
    /// The damage of a source with no usable superblock: every copy that
    /// does not validate, and what does not read, each an error, since
    /// without a superblock nothing else can be found.
    fn report_no_geometry(
        &self,
        source: &dyn BlockRead,
        sink: &mut dyn Sink<Damage>,
    ) -> DamageReport {
        let mut report = DamageReport::default();
        let survey = primitive::survey_superblocks(source);
        let why = self
            .refusal()
            .unwrap_or("no superblock copy validates")
            .to_owned();
        let mut findings: Vec<Damage> = survey
            .copies
            .iter()
            .filter(|c| !c.verdict.is_valid())
            .map(|c| Damage {
                kind: damage_kind(&c.verdict),
                severity: Severity::Error,
                location: Extent::new(
                    c.sector.saturating_mul(512),
                    crate::superblock::SB_HEADER_BYTES as u64,
                ),
                structure: "SuperblockCopy".into(),
                detail: format!(
                    "the superblock copy at sector {}: {}; {why}",
                    c.sector, c.verdict
                ),
            })
            .collect();
        if findings.is_empty() {
            findings.push(Damage {
                kind: DamageKind::Malformed,
                severity: Severity::Error,
                location: Extent::new(crate::superblock::SB_OFFSET, 0),
                structure: "SuperblockCopy".into(),
                detail: why,
            });
        }
        findings.extend(
            unreadable_spans(source, 0, source.size_bytes())
                .into_iter()
                .map(|e| Damage {
                    kind: DamageKind::Unreadable,
                    severity: Severity::Error,
                    location: e,
                    structure: "Unreadable".into(),
                    detail: format!("{} bytes at byte {} do not read", e.length, e.offset),
                }),
        );
        for d in findings {
            report.reported += 1;
            if sink.accept(d).is_break() {
                report.stopped = true;
                break;
            }
        }
        report
    }
}

/// The ranges between `start` and `end` the source fails to read, found a
/// mebibyte at a time and then sector by sector inside a failing one.
fn unreadable_spans(source: &dyn BlockRead, start: u64, end: u64) -> Vec<Extent> {
    const CHUNK: u64 = 1 << 20;
    let mut out: Vec<Extent> = Vec::new();
    let mut push = |o: u64, l: u64| match out.last_mut() {
        Some(last) if last.end() == Some(o) => last.length += l,
        _ => out.push(Extent::new(o, l)),
    };
    let mut at = start;
    let mut buf = vec![0u8; CHUNK as usize];
    while at < end {
        let n = (end - at).min(CHUNK);
        if source.read_at(at, &mut buf[..n as usize]).is_err() {
            let mut o = at;
            while o < at + n {
                let l = (at + n - o).min(512);
                if source.read_at(o, &mut buf[..l as usize]).is_err() {
                    push(o, l);
                }
                o += l;
            }
        }
        at += n;
    }
    out
}

impl SalvageAdapter for BcachefsSalvage {
    type Detail = InodeDetail;

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    fn find_candidates(
        &self,
        source: &dyn BlockRead,
        range: Extent,
        sink: &mut dyn Sink<Candidate<InodeDetail>>,
    ) -> Result<ScanReport, SalvageError> {
        let (start, end) = clip(range, source.size_bytes())?;
        let Ok(s) = self.salvage() else {
            // Without a superblock there is no node magic to look for: no
            // candidate can be told apart, and what does not read is named.
            return Ok(ScanReport {
                candidates: 0,
                unreadable: unreadable_spans(source, start, end),
                stopped_at: None,
            });
        };
        let native = s.find_candidates(source, start, end, |c| sink.accept(shared(&c)));
        Ok(ScanReport {
            candidates: native.candidates,
            unreadable: native
                .unreadable
                .iter()
                .map(|&(o, l)| Extent::new(o, l))
                .collect(),
            stopped_at: native.resume_at,
        })
    }

    /// The candidate's extents in file order, each as the file bytes it
    /// holds from its offset: stored plain, compressed (an [`Encoding`]
    /// [`BcachefsSalvage::read_encoded`] decodes), allocated and unwritten,
    /// or inline in its key's own bytes. A range no key covers is a hole.
    /// `claimed` is the inode's size. A piece no extent can name -- data the
    /// filesystem records as lost, a reflink the reflink btree has nothing
    /// for, an encrypted extent -- ends the walk `Broken`.
    fn walk_extents(
        &self,
        source: &dyn BlockRead,
        candidate: &Candidate<InodeDetail>,
    ) -> Result<ExtentWalk, SalvageError> {
        let s = self.salvage()?;
        let walk = s.walk_extents(source, &native(candidate));
        let claimed = walk.size;
        let device = source.size_bytes();
        let mut extents: Vec<WalkExtent> = Vec::new();
        let mut whole = walk.conflicts.is_empty();
        for p in &walk.pieces {
            if p.file_offset >= claimed {
                break;
            }
            let length = p.length.min(claimed - p.file_offset);
            let next = match &p.kind {
                PieceKind::Data {
                    poisoned: false,
                    csum_type,
                    ..
                } if is_mac(*csum_type) => None,
                PieceKind::Data {
                    extent,
                    data_skip,
                    compression,
                    unwritten,
                    poisoned: false,
                    ..
                } => {
                    let live = extent.crc.map(|c| u64::from(c.offset)).unwrap_or(0) * 512;
                    let at = extent.ptr.offset.saturating_mul(512);
                    match extent.crc {
                        Some(crc)
                            if !*unwritten
                                && !matches!(
                                    *compression,
                                    0 | crate::extent::compression::INCOMPRESSIBLE
                                ) =>
                        {
                            Some(WalkExtent {
                                offset: p.file_offset,
                                source: Extent::new(
                                    at,
                                    u64::from(crc.compressed_size).saturating_mul(512),
                                ),
                                uninitialized: false,
                                encoded: Some(Encoding {
                                    codec: codec(&crc),
                                    decoded: u64::from(crc.uncompressed_size).saturating_mul(512),
                                    offset: live + data_skip,
                                    length,
                                }),
                            })
                        }
                        _ => Some(WalkExtent {
                            offset: p.file_offset,
                            source: Extent::new(at + live + data_skip, length),
                            uninitialized: *unwritten,
                            encoded: None,
                        }),
                    }
                }
                PieceKind::Inline(_) => p.inline_at.map(|at| WalkExtent {
                    offset: p.file_offset,
                    source: Extent::new(at, length),
                    uninitialized: false,
                    encoded: None,
                }),
                // Reads as zeros: a hole, which the walk leaves as a gap.
                PieceKind::Zeros => continue,
                PieceKind::Data { .. }
                | PieceKind::Lost
                | PieceKind::Reflink { .. }
                | PieceKind::Undecodable(_) => None,
            };
            match next.filter(|e| e.source.end().is_some_and(|end| end <= device)) {
                Some(e) => push_merged(&mut extents, e),
                None => whole = false,
            }
        }
        let end = if let Some(&(o, l)) = walk.unreadable.first() {
            WalkEnd::Unreadable(Extent::new(o, l))
        } else if whole {
            WalkEnd::Complete
        } else {
            WalkEnd::Broken
        };
        Ok(ExtentWalk {
            extents,
            claimed,
            end,
        })
    }

    /// The bytes of a compressed extent: read as stored, its checksum
    /// checked, decompressed, and the run's part of them copied out.
    fn read_encoded(
        &self,
        source: &dyn BlockRead,
        stored: Extent,
        encoding: &Encoding,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, SalvageError> {
        let end = stored.end().ok_or(SalvageError::Bounds)?;
        if end > source.size_bytes() || offset > encoding.length {
            return Err(SalvageError::Bounds);
        }
        let n = (encoding.length - offset).min(output.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        let crc = parse_codec(&encoding.codec, stored, encoding.decoded)
            .ok_or(SalvageError::Unsupported(Feature::EncodedExtents))?;
        let extent = crate::extent::DataExtent {
            file_start: 0,
            len: u64::from(crc.uncompressed_size),
            ptr: crate::extent::Ptr {
                dev: 0,
                offset: stored.offset / 512,
                gen: 0,
                flags: 0,
            },
            crc: Some(crc),
            skip: 0,
        };
        let decoded = crate::fs::extent_bytes(&extent, None, source.size_bytes(), |at, buf| {
            Ok(source.read_at(at, buf)?)
        })
        .map_err(|e| match e {
            crate::Error::Io(m) => SalvageError::Device(fs_core::Error::Custom(m)),
            other => SalvageError::Device(fs_core::Error::Custom(other.to_string())),
        })?;
        let from = encoding
            .offset
            .checked_add(offset)
            .and_then(|f| usize::try_from(f).ok())
            .ok_or(SalvageError::Bounds)?;
        let bytes = decoded.get(from..from + n).ok_or(SalvageError::Bounds)?;
        output[..n].copy_from_slice(bytes);
        Ok(n)
    }

    fn rebuild_tree(
        &self,
        source: &dyn BlockRead,
        candidates: &[Candidate<InodeDetail>],
    ) -> Result<Tree, SalvageError> {
        let Ok(s) = self.salvage() else {
            // No directory entry can be found without a superblock: every
            // candidate is an orphan, the root excepted.
            return Ok(Tree {
                nodes: candidates
                    .iter()
                    .map(|c| TreeNode {
                        candidate: c.location.offset,
                        parents: if c.detail.ino == ROOT_INO {
                            vec![ParentClaim::live(Parent::Root)]
                        } else {
                            Vec::new()
                        },
                    })
                    .collect(),
            });
        };
        let natives: Vec<NativeCandidate> = candidates.iter().map(native).collect();
        let native_tree = s.rebuild_tree(source, &natives);
        let claims: BTreeMap<(u64, u32), &super::TreeNode> = native_tree
            .nodes
            .iter()
            .map(|n| ((n.ino, n.snapshot), n))
            .collect();
        // Each directory inode's candidate: the live one, else the newest.
        let mut dirs: BTreeMap<u64, &Candidate<InodeDetail>> = BTreeMap::new();
        for c in candidates
            .iter()
            .filter(|c| c.kind == CandidateKind::Directory)
        {
            let better = match dirs.get(&c.detail.ino) {
                None => true,
                Some(b) => {
                    (c.state == CandidateState::Live, c.detail.journal_seq)
                        > (b.state == CandidateState::Live, b.detail.journal_seq)
                }
            };
            if better {
                dirs.insert(c.detail.ino, c);
            }
        }
        let nodes = candidates
            .iter()
            .map(|c| {
                let mut parents: Vec<ParentClaim> = Vec::new();
                if c.detail.ino == ROOT_INO {
                    parents.push(ParentClaim::live(Parent::Root));
                } else if let Some(n) = claims.get(&(c.detail.ino, c.detail.snapshot)) {
                    // Each entry is its own claim: a hard link is two.
                    for p in &n.parents {
                        if let Some(d) = dirs.get(&p.dir) {
                            let claim = ParentClaim {
                                parent: Parent::Candidate(d.location.offset),
                                name: Some(p.name.clone()),
                                state: if p.live {
                                    CandidateState::Live
                                } else {
                                    CandidateState::Deleted
                                },
                            };
                            if !parents.contains(&claim) {
                                parents.push(claim);
                            }
                        }
                    }
                }
                TreeNode {
                    candidate: c.location.offset,
                    parents,
                }
            })
            .collect();
        Ok(Tree { nodes })
    }

    fn report_damage(
        &self,
        source: &dyn BlockRead,
        sink: &mut dyn Sink<Damage>,
    ) -> Result<DamageReport, SalvageError> {
        let Ok(s) = self.salvage() else {
            return Ok(self.report_no_geometry(source, sink));
        };
        let native = s.report_damage(source, |d| sink.accept(shared_damage(&d)));
        Ok(DamageReport {
            reported: native.reported,
            stopped: native.stopped,
        })
    }

    fn read_extent(
        &self,
        source: &dyn BlockRead,
        extent: Extent,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, SalvageError> {
        let end = extent.end().ok_or(SalvageError::Bounds)?;
        if end > source.size_bytes() || offset > extent.length {
            return Err(SalvageError::Bounds);
        }
        let n = (extent.length - offset).min(output.len() as u64) as usize;
        source
            .read_at(extent.offset + offset, &mut output[..n])
            .map_err(SalvageError::Device)?;
        Ok(n)
    }
}

/// What the adapter lacks: cluster plan steps (bcachefs maps buckets
/// through its btrees, with no linear numbering of allocation units), and
/// geometry hypotheses (a salvage open needs a superblock copy whose header
/// parses, and every copy is tried).
pub fn capabilities() -> Capabilities {
    Capabilities {
        missing: vec![Feature::ClusterSteps, Feature::GeometryHypotheses],
    }
}

/// Merge a stored extent into the one before it when the file and the
/// source both continue and neither is encoded.
fn push_merged(extents: &mut Vec<WalkExtent>, e: WalkExtent) {
    if let Some(last) = extents.last_mut() {
        if last.encoded.is_none()
            && e.encoded.is_none()
            && last.uninitialized == e.uninitialized
            && last.offset.checked_add(last.source.length) == Some(e.offset)
            && last.source.end() == Some(e.source.offset)
        {
            last.source.length += e.source.length;
            return;
        }
    }
    extents.push(e);
}

/// The codec token of a compressed extent: what decoding it needs beyond
/// its stored bytes and decoded size. `bcachefs-<algorithm>;csum=<type>;
/// lo=<hex>;hi=<hex>`.
fn codec(crc: &crate::extent::Crc) -> String {
    use crate::extent::compression;
    let alg = match crc.compression_type {
        compression::GZIP => "gzip".to_owned(),
        compression::LZ4 => "lz4".to_owned(),
        compression::ZSTD => "zstd".to_owned(),
        t => format!("type{t}"),
    };
    format!(
        "bcachefs-{alg};compression={};csum={};lo={:x};hi={:x}",
        crc.compression_type, crc.csum_type, crc.csum_lo, crc.csum_hi
    )
}

/// The checksum and compression entry a codec token describes, for an
/// extent stored at `stored` that decodes to `decoded` bytes.
fn parse_codec(codec: &str, stored: Extent, decoded: u64) -> Option<crate::extent::Crc> {
    let rest = codec.strip_prefix("bcachefs-")?;
    let mut fields = std::collections::BTreeMap::new();
    for part in rest.split(';').skip(1) {
        let (k, v) = part.split_once('=')?;
        fields.insert(k, v);
    }
    Some(crate::extent::Crc {
        compressed_size: u32::try_from(stored.length / 512).ok()?,
        uncompressed_size: u32::try_from(decoded / 512).ok()?,
        offset: 0,
        nonce: 0,
        csum_type: fields.get("csum")?.parse().ok()?,
        compression_type: fields.get("compression")?.parse().ok()?,
        csum_hi: u64::from_str_radix(fields.get("hi")?, 16).ok()?,
        csum_lo: u64::from_str_radix(fields.get("lo")?, 16).ok()?,
    })
}

fn is_mac(csum_type: u8) -> bool {
    csum_type == crate::crypt::CSUM_CHACHA20_POLY1305_80
        || csum_type == crate::crypt::CSUM_CHACHA20_POLY1305_128
}
