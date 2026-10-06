//! Decompression of extent data.

use crate::error::{Error, Result};

/// Decompress `data` (whole sectors as stored) of compression type `t`
/// (crate::extent::compression) into exactly `out_len` bytes.
pub fn decompress(t: u8, _data: &[u8], _out_len: usize) -> Result<Vec<u8>> {
    Err(Error::Unsupported(format!("compression type {t}")))
}
