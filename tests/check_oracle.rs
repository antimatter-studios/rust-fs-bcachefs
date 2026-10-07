//! The checker against the reference checker, on damaged copies of a
//! fixture, INSIDE the harness guest only (`chore test:vm`); anywhere else
//! it fails naming that. Nothing skips. Wherever the reference finds a
//! problem, this crate's checker must find one too.

use std::path::{Path, PathBuf};
use std::process::Command;

use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{btree_id, NodePtr};
use fs_bcachefs::check::check;
use fs_bcachefs::superblock::Superblock;
use fs_core::FileDevice;

fn fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".vm-share/fixtures")
        .join(name);
    assert!(
        p.exists(),
        "{} is missing: build the fixtures with `chore fixtures`",
        p.display()
    );
    p
}

fn damaged(name: &str, damage: impl FnOnce(&mut Vec<u8>, u64)) -> PathBuf {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
    );
    let src = fixture("default.img");
    let sb = Superblock::read(&FileDevice::open(&src).unwrap()).unwrap();
    let r = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == btree_id::DIRENTS)
        .unwrap();
    let k = bkey::decode(
        &r.key,
        &BkeyFormat {
            key_u64s: 5,
            nr_fields: 6,
            bits: [0; 6],
            field_offset: [0; 6],
        },
    )
    .unwrap();
    let node = NodePtr::from_key(&k).unwrap().ptrs[0].offset * 512;
    let mut b = std::fs::read(&src).unwrap();
    damage(&mut b, node);
    std::fs::create_dir_all("/share/check-oracle").unwrap();
    let p = PathBuf::from(format!("/share/check-oracle/{name}.img"));
    std::fs::write(&p, &b).unwrap();
    p
}

/// Whether the reference checker found anything: a failing status, or an
/// error it reported and fixed nothing about (`-n`).
fn reference_finds_a_problem(img: &Path) -> (bool, String) {
    let out = Command::new("bcachefs-ref")
        .args(["fsck", "-n", img.to_str().unwrap()])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (!out.status.success() || text.contains("error"), text)
}

#[test]
fn where_the_reference_finds_damage_this_checker_does_too() {
    let cases = [
        damaged("node-header", |b, node| b[(node + 40) as usize] ^= 0xff),
        damaged("node-first-bset", |b, node| {
            // Find the first bset that holds keys and damage its first key.
            let at = node as usize;
            let mut off = 512;
            while u16::from_le_bytes([b[at + off + 16 + 22], b[at + off + 16 + 23]]) == 0 {
                off += 512;
            }
            b[at + off + 16 + 24 + 8] ^= 0xff;
        }),
    ];
    for img in &cases {
        let (theirs, text) = reference_finds_a_problem(img);
        assert!(
            theirs,
            "{}: the reference found nothing, so this case proves nothing:\n{text}",
            img.display()
        );
        let ours = check(&FileDevice::open(img).unwrap()).unwrap();
        assert!(
            !ours.clean(),
            "{}: the reference found damage this checker did not:\n{text}",
            img.display()
        );
    }
    // And on the undamaged fixture, both are clean.
    let (theirs, text) = reference_finds_a_problem(&fixture("default.img"));
    assert!(
        !theirs,
        "the reference finds a problem in the fixture:\n{text}"
    );
    assert!(check(&FileDevice::open(fixture("default.img")).unwrap())
        .unwrap()
        .clean());
}
