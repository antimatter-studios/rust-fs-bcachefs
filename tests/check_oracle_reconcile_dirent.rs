//! The exact reconcile_work_phys dirent regression, judged independently.
//! Runs only inside the harness guest, without the all-key-type matrix.
#![cfg(feature = "write")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::plant::{empty_root, move_root, RECONCILE_WORK_PHYS};
use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
use fs_bcachefs::btree::{self, btree_id};
use fs_bcachefs::check::check;
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::write::Writer;
use fs_core::FileDevice;

struct Case(PathBuf);

impl Drop for Case {
    fn drop(&mut self) {
        // Generated oracle inputs never survive a reference crash or timeout.
        let _ = std::fs::remove_file(&self.0);
    }
}

fn keys(img: &Path, id: u8) -> Vec<Bkey> {
    let dev = FileDevice::open(img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    btree::walk(&dev, &sb, id).unwrap()
}

fn reference(img: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new("sh")
        .args([
            "-c",
            "ulimit -c 0; exec timeout --kill-after=5 60 bcachefs-ref \"$@\"",
            "reference",
        ])
        .args(args)
        .arg(img)
        .output()
        .expect("the guest needs the reference tools and timeout (`chore test:vm`)");
    if out.status.code().is_none_or(|c| c == 124 || c >= 128) {
        // No output, image, seed or diagnostic from a crashing reference.
        std::fs::remove_file(img).unwrap();
        panic!("reference crashes or hangs: 1");
    }
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

fn clean(ok: bool, text: &str) -> bool {
    ok && !text.lines().any(|line| {
        let line = line.trim_start().to_lowercase();
        !line.starts_with("with options")
            && (line.contains("error")
                || line.contains("invalid bkey")
                || line.contains("fixing")
                || line.contains("ratelimit"))
    })
}

fn excerpt(text: &str) -> String {
    text.lines()
        .take(40)
        .map(|line| line.chars().take(240).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_exact_reconcile_dirent_verdict_matches_the_reference() {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference is required: run `chore test:vm`"
    );
    // The run's scratch root: the reference tools' chroot sees it.
    let scratch = common::ScratchDir::new("check-oracle-reconcile-dirent");
    let img = Case(scratch.join("targeted.img"));
    common::plant::write_sparse(
        &std::fs::read(common::fixture("default.img")).unwrap(),
        &img.0,
    );

    let (ok, baseline) = reference(&img.0, &["fsck", "-n"]);
    assert!(
        clean(ok, &baseline),
        "reference fixture control:\n{}",
        excerpt(&baseline)
    );

    // Identical donor selection and root move to oracle_check_key_types.
    let donor = empty_root(&img.0, RECONCILE_WORK_PHYS);
    assert!(keys(&img.0, donor).is_empty());
    let raw = common::plant::physical_keys(&img.0, donor);
    assert_eq!(
        raw, 0,
        "prerequisite missing: donor btree {donor} root holds {raw} physical keys \
         (a removed key and its whiteout); a physically empty donor node is required"
    );
    // The compacted donor, before it moves, judged by the reference alone:
    // its rewritten node and the accounting the rewrite commits must both
    // be clean, so a dirty moved-root control below is the move's, not the
    // compaction's.
    let (ok, compacted) = reference(&img.0, &["fsck", "-n"]);
    assert!(
        clean(ok, &compacted),
        "reference compacted-donor control (donor btree {donor}, not moved):\n{}",
        excerpt(&compacted)
    );
    assert!(check(&FileDevice::open(&img.0).unwrap()).unwrap().clean());
    move_root(&img.0, donor, RECONCILE_WORK_PHYS);
    let (ok, control) = reference(&img.0, &["fsck", "-n"]);
    assert!(
        clean(ok, &control),
        "reference moved-root control:\n{}",
        excerpt(&control)
    );
    assert!(check(&FileDevice::open(&img.0).unwrap()).unwrap().clean());

    // Copy the same first reference-made dirent, changing only its position.
    let pos = Bpos {
        inode: 0,
        offset: 1,
        snapshot: 0,
    };
    let mut key = keys(&img.0, btree_id::DIRENTS)
        .into_iter()
        .find(|key| key.key_type == key_type::DIRENT)
        .expect("default has a reference-made dirent");
    key.pos = pos;
    let mut writer = Writer::open(FileDevice::open_rw(&img.0).unwrap()).unwrap();
    writer.insert(RECONCILE_WORK_PHYS, vec![key]).unwrap();
    drop(writer);
    assert!(keys(&img.0, RECONCILE_WORK_PHYS)
        .iter()
        .any(|key| key.pos == pos && key.key_type == key_type::DIRENT));

    // A separate reference lister proves the checker can see the exact key.
    let (listed, listing) = reference(&img.0, &["list", "-b", "reconcile_work_phys"]);
    let position = pos.to_string();
    assert!(
        listed
            && listing.lines().any(|line| {
                line.split_whitespace().any(|word| word == position) && line.contains("type dirent")
            }),
        "reference did not list the planted dirent:\n{}",
        excerpt(&listing)
    );

    let ours = check(&FileDevice::open(&img.0).unwrap()).unwrap();
    let ours = ours.problems.iter().any(|problem| {
        problem.kind == "invalid_bkey"
            && problem.detail.contains("btree reconcile_work_phys: ")
            && problem.detail.contains(&format!("key {pos} "))
    });
    let (ok, text) = reference(&img.0, &["fsck", "-n"]);
    let lines: Vec<_> = text.lines().collect();
    let refusal = lines.iter().position(|line| {
        line.contains("invalid bkey")
            && line.contains("btree=reconcile_work_phys ")
            && line.split_whitespace().any(|word| word == position)
    });
    let theirs = match refusal {
        Some(index) => {
            let reason = lines.get(index + 1).copied().unwrap_or_default().trim();
            assert!(
                !reason.is_empty(),
                "reference refusal has no reason:\n{}",
                excerpt(&text)
            );
            println!(
                "reference exact dirent refusal: {} | {reason}",
                lines[index]
            );
            true
        }
        None => {
            assert!(
                clean(ok, &text),
                "reference verdict unresolved:\n{}",
                excerpt(&text)
            );
            println!(
                "reference accepts the exact dirent; unchanged and moved-root controls are clean"
            );
            false
        }
    };
    println!("exact dirent: reference invalid_bkey={theirs}, ours invalid_bkey={ours}");
    assert_eq!(
        ours, theirs,
        "the exact dirent verdict differs from the independent reference"
    );
}
