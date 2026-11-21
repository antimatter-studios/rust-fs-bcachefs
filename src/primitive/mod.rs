//! The primitive layer: read-only, damage-tolerant evidence about the
//! on-disk structures (docs/three-layers.md).
//!
//! The strict reader ([`crate::Filesystem`], [`crate::superblock::Superblock::read`],
//! [`crate::btree::Node::parse`]) refuses what does not validate. The
//! functions here read the same bytes with the same decoders and return
//! what they say, each structure with a [`Verdict`] beside it, so a damaged
//! superblock copy, a btree node with one bad bset or a torn journal entry
//! stays evidence instead of ending the read. They fail only when the
//! source cannot be read at all, and then say where.
//!
//! Every function takes the source as `&dyn BlockRead`, which has no write
//! path: nothing here can change the image.

mod journal;
mod nodes;
mod superblocks;

pub use journal::{survey_journal, JournalEntryEvidence, JournalRoot, JournalSurvey};
pub use nodes::{
    inspect_node, live_nodes, parse_node, scan_nodes, BsetEvidence, LiveNode, LiveNodes,
    NodeContext, NodeEvidence, NodeKey, NodeScan,
};
pub use superblocks::{
    survey_superblocks, CopyOrigin, LayoutEvidence, SuperblockCopy, SuperblockSurvey,
};

use crate::error::Error;

/// What became of reading one structure: whether it validated, and if not,
/// why. Damage is a verdict, never an early return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Magic, checksum and bounds all hold.
    Valid,
    /// The magic number is not this structure's: unwritten, overwritten or
    /// not this filesystem's.
    BadMagic,
    /// The stored checksum does not match the bytes it covers.
    BadChecksum { stored: u64, computed: u64 },
    /// The bytes are this structure's but do not hold together: a length
    /// past its end, a key that does not decode.
    Corrupt(String),
    /// A valid structure this crate does not read, such as an encrypted
    /// record without its key.
    Unsupported(String),
    /// The source failed to read these bytes.
    Unreadable(String),
}

impl Verdict {
    pub fn is_valid(&self) -> bool {
        matches!(self, Verdict::Valid)
    }
}

impl From<&Error> for Verdict {
    fn from(e: &Error) -> Self {
        match e {
            Error::BadMagic { .. } => Verdict::BadMagic,
            Error::BadChecksum {
                stored, computed, ..
            } => Verdict::BadChecksum {
                stored: *stored,
                computed: *computed,
            },
            Error::Io(m) => Verdict::Unreadable(m.clone()),
            Error::Unsupported(m) => Verdict::Unsupported(m.clone()),
            Error::Corrupt(m) | Error::NotFound(m) => Verdict::Corrupt(m.clone()),
        }
    }
}

impl From<Error> for Verdict {
    fn from(e: Error) -> Self {
        Verdict::from(&e)
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Valid => write!(f, "valid"),
            Verdict::BadMagic => write!(f, "bad magic"),
            Verdict::BadChecksum { stored, computed } => {
                write!(
                    f,
                    "bad checksum: stored {stored:#x}, computed {computed:#x}"
                )
            }
            Verdict::Corrupt(m) => write!(f, "corrupt: {m}"),
            Verdict::Unsupported(m) => write!(f, "unsupported: {m}"),
            Verdict::Unreadable(m) => write!(f, "unreadable: {m}"),
        }
    }
}
