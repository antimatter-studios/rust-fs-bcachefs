//! Replays every committed fuzz seed through the parser its target drives,
//! on every pull request: a seed that once crashed a parser must never
//! crash it again, and the seeds themselves (real blocks of reference-
//! formatted images) must still parse.

use std::path::PathBuf;

fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/corpus")
        .join(target);
    let mut out: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    assert!(!out.is_empty(), "fuzz/corpus/{target} holds no seeds");
    out
}

#[test]
fn every_superblock_seed_parses_and_every_truncation_is_refused_cleanly() {
    for (name, data) in seeds("superblock") {
        let sb = fs_bcachefs::superblock::Superblock::parse(&data)
            .unwrap_or_else(|e| panic!("seed {name}: {e}"));
        sb.members().unwrap();
        assert!(!sb.btree_roots().unwrap().is_empty(), "seed {name}");
        for n in (0..data.len()).step_by(7) {
            let _ = fs_bcachefs::superblock::Superblock::parse(&data[..n]);
        }
    }
}

/// The same walk the btree_node fuzz target makes (fuzz/src/lib.rs).
fn btree_node(data: &[u8]) -> usize {
    if data.len() < 24 {
        return 0;
    }
    let magic = u64::from_le_bytes(data[16..24].try_into().unwrap());
    let mut keys = 0;
    for block in [512usize, 4096] {
        if let Ok(node) = fs_bcachefs::btree::Node::parse(data, magic, block, None) {
            for k in &node.keys {
                let _ = fs_bcachefs::inode::Inode::from_key(k);
                let _ = fs_bcachefs::inode::Dirent::from_key(k);
                let _ = fs_bcachefs::extent::DataExtent::from_key(k);
                let _ = fs_bcachefs::btree::NodePtr::from_key(k);
            }
            keys += node.keys.len();
        }
    }
    keys
}

#[test]
fn every_btree_node_seed_parses_and_mutations_never_panic() {
    for (name, data) in seeds("btree_node") {
        assert!(btree_node(&data) > 0, "seed {name} yielded no keys");
        let mut d = data.clone();
        for i in (0..d.len()).step_by(97) {
            d[i] ^= 0x5a;
            let _ = btree_node(&d);
        }
        for n in (0..data.len()).step_by(131) {
            let _ = btree_node(&data[..n]);
        }
    }
}
