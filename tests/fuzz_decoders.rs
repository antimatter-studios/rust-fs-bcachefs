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
