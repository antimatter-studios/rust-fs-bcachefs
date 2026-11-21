//! Every superblock copy, each with its own verdict.

use super::Verdict;
use crate::superblock::{Layout, Superblock, Version, LAYOUT_OFFSET, SB_OFFSET};
use fs_core::BlockRead;

/// Which layout named a copy. A copy several layouts name is listed once,
/// with the first origin that named it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOrigin {
    /// The primary copy, at sector 8, which is read whatever any layout says.
    Primary,
    /// Named by the layout embedded in the primary copy's header.
    PrimaryLayout,
    /// Named by the standalone layout at sector 7.
    StandaloneLayout,
}

/// One superblock copy and what reading it found.
#[derive(Debug, Clone)]
pub struct SuperblockCopy {
    /// The sector it was read from.
    pub sector: u64,
    pub origin: CopyOrigin,
    pub verdict: Verdict,
    /// Header facts, whenever the header parsed, even under a bad checksum.
    pub seq: Option<u64>,
    pub uuid: Option<[u8; 16]>,
    pub version: Option<Version>,
    /// The sector the copy records as its own, which damage can make differ
    /// from where it was found.
    pub recorded_sector: Option<u64>,
}

/// The standalone layout at sector 7.
#[derive(Debug, Clone)]
pub struct LayoutEvidence {
    pub verdict: Verdict,
    pub layout: Option<Layout>,
}

/// Every superblock copy the device holds, and which one a reader believes.
#[derive(Debug, Clone)]
pub struct SuperblockSurvey {
    pub standalone_layout: LayoutEvidence,
    pub copies: Vec<SuperblockCopy>,
    /// The sector of the copy chosen: the highest `seq` among valid copies,
    /// the later-listed one winning a tie, as [`Superblock::read`] chooses.
    pub chosen: Option<u64>,
    /// The chosen copy, fully parsed.
    pub superblock: Option<Superblock>,
    /// Where valid copies disagree: `seq` or internal UUID.
    pub disagreements: Vec<String>,
}

impl SuperblockSurvey {
    /// Copies whose verdict is not [`Verdict::Valid`].
    pub fn damaged(&self) -> impl Iterator<Item = &SuperblockCopy> {
        self.copies.iter().filter(|c| !c.verdict.is_valid())
    }
}

/// Read every superblock copy any layout names. Never fails: a copy that
/// does not read is a copy with a verdict, and a device with no valid copy
/// is a survey whose `chosen` is `None`.
///
/// The copies are the primary at sector 8, every sector the primary's
/// header layout names (read even when the primary's checksum fails, since
/// the layout is still what the bytes say), and every sector the standalone
/// layout at sector 7 names.
pub fn survey_superblocks(dev: &dyn BlockRead) -> SuperblockSurvey {
    let primary_sector = SB_OFFSET / 512;
    let mut copies: Vec<SuperblockCopy> = Vec::new();
    let mut primary_layout: Option<Layout> = None;

    let primary = read_copy(
        dev,
        primary_sector,
        CopyOrigin::Primary,
        &mut primary_layout,
    );
    copies.push(primary);

    let standalone = {
        let mut b = vec![0u8; Layout::BYTES];
        match dev.read_at(LAYOUT_OFFSET, &mut b) {
            Err(e) => LayoutEvidence {
                verdict: Verdict::Unreadable(e.to_string()),
                layout: None,
            },
            Ok(()) => match Layout::parse(&b) {
                Ok(l) => LayoutEvidence {
                    verdict: Verdict::Valid,
                    layout: Some(l),
                },
                Err(e) => LayoutEvidence {
                    verdict: Verdict::from(e),
                    layout: None,
                },
            },
        }
    };

    let named = primary_layout
        .iter()
        .flat_map(|l| l.sb_offsets.iter().map(|&s| (s, CopyOrigin::PrimaryLayout)))
        .chain(standalone.layout.iter().flat_map(|l| {
            l.sb_offsets
                .iter()
                .map(|&s| (s, CopyOrigin::StandaloneLayout))
        }))
        .collect::<Vec<_>>();
    for (sector, origin) in named {
        if copies.iter().any(|c| c.sector == sector) {
            continue;
        }
        let mut ignored = None;
        copies.push(read_copy(dev, sector, origin, &mut ignored));
    }

    let mut chosen: Option<&SuperblockCopy> = None;
    for c in copies.iter().filter(|c| c.verdict.is_valid()) {
        if chosen.is_none_or(|b| c.seq >= b.seq) {
            chosen = Some(c);
        }
    }
    let chosen = chosen.map(|c| c.sector);
    let superblock = chosen.and_then(|s| Superblock::read_at(dev, s).ok());

    let mut disagreements = Vec::new();
    let valid: Vec<&SuperblockCopy> = copies.iter().filter(|c| c.verdict.is_valid()).collect();
    if let Some(first) = valid.first() {
        for c in &valid[1..] {
            if c.seq != first.seq {
                disagreements.push(format!(
                    "copy at sector {} has seq {}, copy at sector {} has seq {}",
                    c.sector,
                    c.seq.unwrap_or_default(),
                    first.sector,
                    first.seq.unwrap_or_default()
                ));
            }
            if c.uuid != first.uuid {
                disagreements.push(format!(
                    "copy at sector {} has another internal UUID than the copy at sector {}",
                    c.sector, first.sector
                ));
            }
        }
    }

    SuperblockSurvey {
        standalone_layout: standalone,
        copies,
        chosen,
        superblock,
        disagreements,
    }
}

/// One copy: its bytes, its header, its checksum, and its fields. The
/// header's layout goes to `layout` whenever the header parses.
fn read_copy(
    dev: &dyn BlockRead,
    sector: u64,
    origin: CopyOrigin,
    layout: &mut Option<Layout>,
) -> SuperblockCopy {
    let mut copy = SuperblockCopy {
        sector,
        origin,
        verdict: Verdict::Valid,
        seq: None,
        uuid: None,
        version: None,
        recorded_sector: None,
    };
    let bytes = match Superblock::read_bytes_at(dev, sector) {
        Ok(b) => b,
        Err(e) => {
            copy.verdict = Verdict::from(e);
            return copy;
        }
    };
    let header = match Superblock::parse_header(&bytes) {
        Ok(h) => h,
        Err(e) => {
            copy.verdict = Verdict::from(e);
            return copy;
        }
    };
    copy.seq = Some(header.seq);
    copy.uuid = Some(header.uuid);
    copy.version = Some(header.version);
    copy.recorded_sector = Some(header.offset);
    *layout = Some(header.layout.clone());
    copy.verdict = match header.verify_checksum(&bytes) {
        Err(e) => Verdict::from(e),
        Ok(()) => match Superblock::parse(&bytes) {
            Ok(_) => Verdict::Valid,
            Err(e) => Verdict::from(e),
        },
    };
    copy
}
