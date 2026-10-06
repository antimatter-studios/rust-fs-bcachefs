//! Little-endian field readers. bcachefs is little-endian on disk.
//!
//! Every reader takes a slice and an offset the caller has bounds-checked;
//! they panic on a short slice, so parsers check lengths first.

pub(crate) fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"))
}

pub(crate) fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().expect("8 bytes"))
}

pub(crate) fn uuid_at(b: &[u8], o: usize) -> [u8; 16] {
    b[o..o + 16].try_into().expect("16 bytes")
}
