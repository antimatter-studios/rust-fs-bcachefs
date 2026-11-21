//! Every journal entry in the journal buckets, torn and stale ones too.

use super::Verdict;
use crate::bkey::Bkey;
use crate::journal::{
    bucket_ranges, entry_type, jset_magic, parse_entries, FLAG_NO_FLUSH, JSET_HEADER,
};
use crate::superblock::Superblock;
use crate::util::{le32, le64};
use fs_core::BlockRead;

/// A btree root a journal entry records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRoot {
    pub btree_id: u8,
    pub level: u8,
    pub key: Bkey,
}

/// One journal entry (`jset`) as found in its bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryEvidence {
    /// Where it starts, in 512-byte sectors from the start of the device.
    pub sector: u64,
    pub seq: u64,
    /// The oldest entry still needed when this one was written.
    pub last_seq: u64,
    pub flush: bool,
    pub version: u32,
    /// Its length in bytes, as its header claims.
    pub bytes: usize,
    pub verdict: Verdict,
    /// The btree roots it records; empty unless its checksum holds.
    pub roots: Vec<JournalRoot>,
    /// How many btree keys its `btree_keys` and `overwrite` sub-entries carry.
    pub keys: usize,
}

/// The journal, entry by entry, as the buckets hold it.
#[derive(Debug, Clone, Default)]
pub struct JournalSurvey {
    /// Byte offset of every journal bucket the superblock names.
    pub buckets: Vec<u64>,
    /// Buckets the source failed to read, by byte offset.
    pub unreadable: Vec<u64>,
    /// Every entry, in bucket order.
    pub entries: Vec<JournalEntryEvidence>,
    /// Why the journal could not be found at all: no `journal_v2` field, no
    /// member entry, a zero bucket size.
    pub error: Option<String>,
}

impl JournalSurvey {
    /// The newest entry whose checksum holds and that is a flush: the one a
    /// replay starts from.
    pub fn newest_flush(&self) -> Option<&JournalEntryEvidence> {
        self.entries
            .iter()
            .filter(|e| e.flush && e.verdict.is_valid())
            .max_by_key(|e| e.seq)
    }
}

/// Read every journal bucket the superblock's `journal_v2` field names and
/// list every entry in each. An entry is recognised by its magic; one whose
/// checksum fails, or whose sub-entries do not parse, is listed with that
/// verdict, and the bucket is read on past it by the length its header
/// claims. A bucket ends at the first block that does not carry the magic,
/// or at an entry whose claimed length runs past the bucket.
///
/// Never fails: a journal that cannot be located is a survey with `error`.
pub fn survey_journal(dev: &dyn BlockRead, sb: &Superblock) -> JournalSurvey {
    let mut survey = JournalSurvey::default();
    let bucket_bytes = match sb.members() {
        Ok(m) => match m.get(sb.dev_idx as usize) {
            Some(m) if m.bucket_size > 0 => u64::from(m.bucket_size) * 512,
            Some(_) => {
                survey.error = Some("bucket size 0".into());
                return survey;
            }
            None => {
                survey.error = Some("no member entry for this device".into());
                return survey;
            }
        },
        Err(e) => {
            survey.error = Some(format!("members: {e}"));
            return survey;
        }
    };
    let ranges = match bucket_ranges(sb) {
        Ok(r) => r,
        Err(e) => {
            survey.error = Some(e.to_string());
            return survey;
        }
    };
    let block = (sb.block_size as usize * 512).max(512);
    let magic = jset_magic(&sb.uuid);
    for (start, nr) in ranges {
        for bucket in start..start.saturating_add(nr) {
            let Some(base) = bucket.checked_mul(bucket_bytes) else {
                survey.error = Some(format!("journal bucket {bucket} overflows"));
                continue;
            };
            survey.buckets.push(base);
            let mut buf = vec![0u8; bucket_bytes as usize];
            if base.saturating_add(bucket_bytes) > dev.size_bytes()
                || dev.read_at(base, &mut buf).is_err()
            {
                survey.unreadable.push(base);
                continue;
            }
            read_bucket(&buf, base, magic, block, &mut survey.entries);
        }
    }
    survey
}

fn read_bucket(
    buf: &[u8],
    base: u64,
    magic: u64,
    block: usize,
    out: &mut Vec<JournalEntryEvidence>,
) {
    let mut at = 0usize;
    while at + JSET_HEADER <= buf.len() && le64(buf, at + 16) == magic {
        let b = &buf[at..];
        let flags = le32(b, 36);
        let bytes = u64::from(le32(b, 40)) * 8 + JSET_HEADER as u64;
        let mut e = JournalEntryEvidence {
            sector: (base + at as u64) / 512,
            seq: le64(b, 24),
            last_seq: le64(b, 48),
            flush: flags & FLAG_NO_FLUSH == 0,
            version: le32(b, 32),
            bytes: bytes.min(usize::MAX as u64) as usize,
            verdict: Verdict::Valid,
            roots: Vec::new(),
            keys: 0,
        };
        if bytes > b.len() as u64 {
            e.verdict = Verdict::Corrupt(format!(
                "the entry claims {bytes} bytes, and {} remain in its bucket",
                b.len()
            ));
            out.push(e);
            return;
        }
        let end = bytes as usize;
        let csum_type = (flags & 0xf) as u8;
        e.verdict = if csum_type == crate::crypt::CSUM_CHACHA20_POLY1305_128 {
            Verdict::Unsupported("an encrypted entry: its MAC needs the master key".into())
        } else if !crate::csum::is_known(csum_type) {
            Verdict::Unsupported(format!("checksum type {csum_type}"))
        } else {
            let stored = le64(b, 0) & crate::btree::csum_mask(csum_type);
            match crate::csum::verify(csum_type, &b[16..end], stored) {
                Ok(()) => Verdict::Valid,
                Err(computed) => Verdict::BadChecksum { stored, computed },
            }
        };
        if e.verdict.is_valid() {
            match parse_entries(b, end) {
                Ok(entries) => {
                    for entry in entries {
                        match entry.entry_type {
                            entry_type::BTREE_ROOT => {
                                e.roots
                                    .extend(entry.keys.into_iter().map(|key| JournalRoot {
                                        btree_id: entry.btree_id,
                                        level: entry.level,
                                        key,
                                    }))
                            }
                            entry_type::BTREE_KEYS | entry_type::OVERWRITE => {
                                e.keys += entry.keys.len()
                            }
                            _ => {}
                        }
                    }
                }
                Err(err) => e.verdict = Verdict::from(err),
            }
        }
        out.push(e);
        at += end.div_ceil(block).max(1) * block;
    }
}
