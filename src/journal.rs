//! The journal: reading its entries, and replaying the newest of them over
//! the btrees in memory when a filesystem was not shut down cleanly.

use crate::bkey::Bkey;
use crate::error::{Error, Result};
use crate::superblock::Superblock;
use fs_core::BlockRead;

/// One journal entry (`jset`) as found on the device.
#[derive(Debug, Clone)]
pub struct Jset {
    pub seq: u64,
    /// The oldest entry still needed when this one was written (0 on an
    /// entry that is not a flush).
    pub last_seq: u64,
    pub flush: bool,
    pub version: u32,
    /// Where it starts, in 512-byte sectors from the start of the device.
    pub sector: u64,
    /// Its length in bytes: the header and its sub-entries.
    pub bytes: usize,
    pub entries: Vec<JsetEntry>,
}

/// One sub-entry of a jset.
#[derive(Debug, Clone)]
pub struct JsetEntry {
    pub entry_type: u8,
    pub btree_id: u8,
    pub level: u8,
    /// The keys of a `btree_keys`, `btree_root` or `overwrite` entry.
    pub keys: Vec<Bkey>,
}

/// Every valid entry in the journal, by sequence number.
pub fn read_entries(_dev: &dyn BlockRead, _sb: &Superblock) -> Result<Vec<Jset>> {
    Err(Error::Unsupported("the journal is not read yet".into()))
}

/// Sub-entry types, in the order the Principles of Operation lists them
/// (S1, 11.2).
pub mod entry_type {
    pub const BTREE_KEYS: u8 = 0;
    pub const BTREE_ROOT: u8 = 1;
}

/// A btree's name as the reference tools print it.
pub fn btree_name(_id: u8) -> String {
    String::new()
}
