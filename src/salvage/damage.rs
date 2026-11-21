//! Report damage: every structure that does not validate, and every range
//! that does not read.

use super::Salvage;
use crate::primitive::{self, Verdict};
use fs_core::BlockRead;
use std::ops::ControlFlow;

/// Which structure a damage record is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DamageSource {
    SuperblockCopy,
    /// The standalone superblock layout at sector 7.
    SuperblockLayout,
    /// Valid superblock copies that disagree.
    SuperblockDisagreement,
    JournalEntry,
    JournalBucket,
    BtreeNode,
    /// A pointer of the live tree that leads to no node, or to another node
    /// than it expects.
    LiveTree,
    /// A range the source failed to read.
    Unreadable,
}

/// One damage record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Damage {
    pub source: DamageSource,
    /// Device byte offset and length of what is damaged, where known.
    pub offset: u64,
    pub length: u64,
    pub verdict: Verdict,
    /// What and where, in words.
    pub detail: String,
}

/// What a damage report covered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DamageReport {
    pub reported: u64,
    /// Whether the sink stopped the report before its end.
    pub stopped: bool,
}

impl Salvage {
    /// Every superblock copy, journal entry and btree node on the device
    /// that does not validate, every live-tree pointer that leads nowhere,
    /// and every range that does not read, passed to `sink`, which may stop
    /// the report. A device with no damage reports nothing.
    pub fn report_damage(
        &self,
        dev: &dyn BlockRead,
        mut sink: impl FnMut(Damage) -> ControlFlow<()>,
    ) -> DamageReport {
        let mut report = DamageReport::default();
        let mut emit = |d: Damage, report: &mut DamageReport| {
            report.reported += 1;
            let flow = sink(d);
            report.stopped = flow.is_break();
            flow
        };
        macro_rules! emit {
            ($d:expr) => {
                if emit($d, &mut report).is_break() {
                    return report;
                }
            };
        }

        let survey = primitive::survey_superblocks(dev);
        if !survey.standalone_layout.verdict.is_valid() {
            emit!(Damage {
                source: DamageSource::SuperblockLayout,
                offset: crate::superblock::LAYOUT_OFFSET,
                length: crate::superblock::Layout::BYTES as u64,
                verdict: survey.standalone_layout.verdict.clone(),
                detail: "the superblock layout at sector 7".into(),
            });
        }
        for c in survey.damaged() {
            emit!(Damage {
                source: DamageSource::SuperblockCopy,
                offset: c.sector.saturating_mul(512),
                length: crate::superblock::SB_HEADER_BYTES as u64,
                verdict: c.verdict.clone(),
                detail: format!("the superblock copy at sector {}", c.sector),
            });
        }
        for d in &survey.disagreements {
            emit!(Damage {
                source: DamageSource::SuperblockDisagreement,
                offset: 0,
                length: 0,
                verdict: Verdict::Corrupt(d.clone()),
                detail: d.clone(),
            });
        }

        let journal = primitive::survey_journal(dev, self.superblock());
        if let Some(e) = &journal.error {
            emit!(Damage {
                source: DamageSource::JournalBucket,
                offset: 0,
                length: 0,
                verdict: Verdict::Corrupt(e.clone()),
                detail: format!("the journal cannot be located: {e}"),
            });
        }
        let bucket_bytes = self.bucket_bytes();
        for &b in &journal.unreadable {
            emit!(Damage {
                source: DamageSource::JournalBucket,
                offset: b,
                length: bucket_bytes,
                verdict: Verdict::Unreadable("the journal bucket does not read".into()),
                detail: format!("the journal bucket at byte {b}"),
            });
        }
        for e in journal.entries.iter().filter(|e| !e.verdict.is_valid()) {
            emit!(Damage {
                source: DamageSource::JournalEntry,
                offset: e.sector.saturating_mul(512),
                length: e.bytes as u64,
                verdict: e.verdict.clone(),
                detail: format!("journal entry {} at sector {}", e.seq, e.sector),
            });
        }

        for (offset, why) in &self.live_nodes().breaks {
            emit!(Damage {
                source: DamageSource::LiveTree,
                offset: *offset,
                length: 0,
                verdict: Verdict::Corrupt(why.clone()),
                detail: format!("a live-tree pointer: {why}"),
            });
        }

        let mut damaged_nodes = Vec::new();
        let scan = primitive::scan_nodes(dev, self.node_context(), 0, u64::MAX, |n| {
            if !n.is_valid() {
                damaged_nodes.push(n);
            }
            ControlFlow::Continue(())
        });
        for n in damaged_nodes {
            let bad = n
                .bsets
                .iter()
                .find(|b| !b.verdict.is_valid() || b.key_error.is_some());
            let verdict = match bad {
                Some(b) if !b.verdict.is_valid() => b.verdict.clone(),
                Some(b) => Verdict::Corrupt(b.key_error.clone().unwrap_or_default()),
                None => Verdict::Corrupt(n.issues.join("; ")),
            };
            emit!(Damage {
                source: DamageSource::BtreeNode,
                offset: n.offset,
                length: n.written_bytes.max(512) as u64,
                verdict,
                detail: format!(
                    "the btree {} node at level {} at byte {} ({})",
                    crate::journal::btree_name(n.btree_id),
                    n.level,
                    n.offset,
                    if self
                        .live_nodes()
                        .nodes
                        .get(&n.offset)
                        .is_some_and(|l| l.seq == n.seq)
                    {
                        "live"
                    } else {
                        "an older version"
                    }
                ),
            });
        }
        for (offset, length) in scan.unreadable {
            emit!(Damage {
                source: DamageSource::Unreadable,
                offset,
                length,
                verdict: Verdict::Unreadable("the source failed to read".into()),
                detail: format!("{length} bytes at byte {offset}"),
            });
        }
        report
    }

    pub(crate) fn bucket_bytes(&self) -> u64 {
        self.superblock()
            .members()
            .ok()
            .and_then(|m| m.get(self.superblock().dev_idx as usize).cloned())
            .map(|m| u64::from(m.bucket_size) * 512)
            .unwrap_or(0)
    }
}
