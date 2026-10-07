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

/// The fuzzing switch: built with the `fuzzing` feature (the fuzz targets
/// only), a checksum that does not match is not a reason to stop, so
/// mutated input reaches the decoders behind it. In every other build a
/// corrupted superblock is refused for its checksum. Both builds run this:
/// the unit tier once each way.
#[test]
fn checksums_are_verified_unless_built_for_fuzzing() {
    for (name, mut data) in seeds("superblock") {
        // A byte of the label: inside the checksummed range, harmless to the
        // parse otherwise.
        data[0x48] ^= 0x5a;
        let r = fs_bcachefs::superblock::Superblock::parse(&data);
        if cfg!(feature = "fuzzing") {
            assert!(
                r.is_ok(),
                "seed {name}: the fuzzing build refused it: {:?}",
                r.err()
            );
        } else {
            assert!(
                matches!(r, Err(fs_bcachefs::Error::BadChecksum { .. })),
                "seed {name}: a corrupted superblock was not refused for its checksum: {:?}",
                r.map(|_| ())
            );
        }
    }
}

/// The jset target's seeds: real journal entries of the uncleanly unmounted
/// fixture, which must parse, and whose truncations must be refused cleanly.
#[test]
fn every_jset_seed_parses_and_every_truncation_is_refused_cleanly() {
    for (name, data) in seeds("jset") {
        let magic = u64::from_le_bytes(data[16..24].try_into().unwrap());
        let j = fs_bcachefs::journal::parse_jset(&data, magic)
            .unwrap_or_else(|e| panic!("seed {name}: {e}"))
            .unwrap_or_else(|| panic!("seed {name}: not a jset"));
        assert!(!j.entries.is_empty(), "seed {name}");
        for n in (0..data.len()).step_by(13) {
            let _ = fs_bcachefs::journal::parse_jset(&data[..n], magic);
        }
    }
}

/// The key_values target's seeds: one unpacked key of each kind from the
/// aged fixture, which must decode as that kind.
#[test]
fn every_key_seed_decodes() {
    let fmt = fs_bcachefs::bkey::BkeyFormat {
        key_u64s: 5,
        nr_fields: 6,
        bits: [0; 6],
        field_offset: [0; 6],
    };
    for (name, data) in seeds("key_values") {
        let k =
            fs_bcachefs::bkey::decode(&data, &fmt).unwrap_or_else(|e| panic!("seed {name}: {e}"));
        let ok = match k.key_type {
            6 => fs_bcachefs::extent::DataExtent::from_key(&k).is_ok(),
            // Inline data: the value is the bytes themselves.
            17 => !k.value.is_empty(),
            10 => fs_bcachefs::inode::Dirent::from_key(&k).is_ok(),
            11 => fs_bcachefs::xattr::Xattr::from_key(&k).is_ok(),
            29 => fs_bcachefs::inode::Inode::from_key(&k).is_ok(),
            t => panic!("seed {name}: key type {t}"),
        };
        assert!(ok, "seed {name} does not decode");
    }
}
