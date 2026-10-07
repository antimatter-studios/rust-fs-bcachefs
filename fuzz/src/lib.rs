//! The decoders each fuzz target drives, shared with tests/fuzz_decoders.rs
//! through being trivially small: a node's keys go on through every value
//! decoder that could see them.

/// Parse a btree node (accepting whatever magic it carries, so the fuzzer
/// gets past the magic check) and decode every key's value.
pub fn btree_node(data: &[u8]) {
    if data.len() < 24 {
        return;
    }
    let magic = u64::from_le_bytes(data[16..24].try_into().unwrap());
    for block in [512usize, 4096] {
        if let Ok(node) = fs_bcachefs::btree::Node::parse(data, magic, block, None) {
            for k in &node.keys {
                let _ = fs_bcachefs::inode::Inode::from_key(k);
                let _ = fs_bcachefs::inode::Dirent::from_key(k);
                let _ = fs_bcachefs::extent::DataExtent::from_key(k);
                let _ = fs_bcachefs::btree::NodePtr::from_key(k);
            }
        }
    }
}

/// Parse a journal entry (accepting whatever magic it carries) and decode
/// every key it holds through every value decoder.
pub fn jset(data: &[u8]) {
    if data.len() < 24 {
        return;
    }
    let magic = u64::from_le_bytes(data[16..24].try_into().unwrap());
    if let Ok(Some(j)) = fs_bcachefs::journal::parse_jset(data, magic) {
        for e in &j.entries {
            for k in &e.keys {
                values(k);
            }
        }
    }
}

/// Every value decoder over one key.
pub fn values(k: &fs_bcachefs::bkey::Bkey) {
    let _ = fs_bcachefs::inode::Inode::from_key(k);
    let _ = fs_bcachefs::inode::Dirent::from_key(k);
    let _ = fs_bcachefs::extent::DataExtent::from_key(k);
    let _ = fs_bcachefs::btree::NodePtr::from_key(k);
    let _ = fs_bcachefs::xattr::Xattr::from_key(k);
}

/// An unpacked key from raw bytes, every key type over the same value.
pub fn key_values(data: &[u8]) {
    let fmt = fs_bcachefs::bkey::BkeyFormat {
        key_u64s: 5,
        nr_fields: 6,
        bits: [0; 6],
        field_offset: [0; 6],
    };
    if let Ok(mut k) = fs_bcachefs::bkey::decode(data, &fmt) {
        for t in [6u8, 10, 11, 17, 18, 23, 29] {
            k.key_type = t;
            values(&k);
        }
    }
}
