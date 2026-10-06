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
