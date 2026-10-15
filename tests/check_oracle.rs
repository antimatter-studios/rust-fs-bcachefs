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
    // Any line naming an error, but not the options line, whose
    // `errors=`/`fix_errors=` is no finding.
    let named = text.lines().any(|l| {
        !l.trim_start().starts_with("with options") && (l.contains("error") || l.contains("fixing"))
    });
    (!out.status.success() || named, text)
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
    // And on an undamaged copy, both are clean. A copy under /share: the
    // reference tools run in a chroot that sees /share and not /repo.
    let clean = damaged("undamaged", |_, _| {});
    let (theirs, text) = reference_finds_a_problem(&clean);
    assert!(
        !theirs,
        "the reference finds a problem in the fixture:\n{text}"
    );
    assert!(check(&FileDevice::open(&clean).unwrap()).unwrap().clean());
}

/// The inode damage `tests/oracle_check_inode_metadata.rs` makes (#221),
/// judged by the reference checker itself: a directory of nonzero size,
/// an inode journal sequence in the future, and sector counts that are
/// not what the extents add up to. The reference finds each, and this
/// checker names each.
#[cfg(feature = "write")]
#[test]
fn where_the_reference_finds_inode_damage_this_checker_names_it() {
    use fs_bcachefs::bkey::key_type;
    use fs_bcachefs::btree;
    use fs_bcachefs::inode::InodeV3Raw;
    use fs_bcachefs::write::Writer;
    use fs_bcachefs::Filesystem;

    /// A copy of fixture `src` under /share, with the inode at `path`
    /// changed by `change` through the writer. By path: inode numbers
    /// differ from one build of the fixtures to the next.
    fn rewritten(
        name: &str,
        src: &str,
        path: &str,
        change: impl FnOnce(&mut InodeV3Raw),
    ) -> PathBuf {
        assert!(
            Path::new("/usr/local/bin/bcachefs-ref").exists(),
            "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
        );
        std::fs::create_dir_all("/share/check-oracle").unwrap();
        let p = PathBuf::from(format!("/share/check-oracle/{name}.img"));
        std::fs::copy(fixture(src), &p).unwrap();
        let ino = Filesystem::open(FileDevice::open(&p).unwrap())
            .unwrap()
            .lookup(path)
            .unwrap();
        let dev = FileDevice::open(&p).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut k = btree::walk(&dev, &sb, btree_id::INODES)
            .unwrap()
            .into_iter()
            .find(|k| k.pos.inode == 0 && k.pos.offset == ino && k.key_type == key_type::INODE_V3)
            .unwrap_or_else(|| panic!("{src}: no inode {ino} at {path}"));
        let mut raw = InodeV3Raw::parse(&k.value).unwrap();
        change(&mut raw);
        k.value = raw.encode();
        let mut w = Writer::open(FileDevice::open_rw(&p).unwrap()).unwrap();
        w.insert(btree_id::INODES, vec![k]).unwrap();
        p
    }

    let cases = [
        (
            rewritten("inode-dir-size", "kernel-written.img", "/optdir", |i| {
                i.size = u64::MAX
            }),
            "directory_nonzero_i_size",
        ),
        (
            rewritten("inode-seq-future", "lz4.img", "/hello.txt", |i| {
                i.journal_seq = 16_711_691
            }),
            "inode_journal_seq_in_future",
        ),
        (
            rewritten("inode-sectors-max", "aged.img", "/many/f02467", |i| {
                i.sectors = u64::MAX
            }),
            "inode_i_sectors_wrong",
        ),
        (
            rewritten("inode-sectors-48", "lz4.img", "/many/file-0271.txt", |i| {
                i.sectors = (1 << 48) - 1
            }),
            "inode_i_sectors_wrong",
        ),
    ];
    for (img, kind) in &cases {
        let (theirs, text) = reference_finds_a_problem(img);
        assert!(
            theirs,
            "{}: the reference found nothing, so this case proves nothing:\n{text}",
            img.display()
        );
        let ours = check(&FileDevice::open(img).unwrap()).unwrap();
        assert!(
            ours.problems.iter().any(|p| p.kind == *kind),
            "{}: the reference found damage this checker did not name as {kind}: {:?}\n{text}",
            img.display(),
            ours.problems
        );
    }
}

/// The dirent damage `tests/oracle_check_dirent_hash.rs` makes (#221),
/// judged by the reference checker itself. Differential run 38752704728
/// (job 116482951023, kernel seed 45) grounds the class: the reference
/// reported "hash table key at wrong offset" and counted
/// `hash_table_key_wrong_offset` where this crate called the image clean.
/// A dirent five slots past its hash with empty slots between is that
/// damage to both checkers; one slot past a whiteout left in its own slot
/// is reachable, and neither reports it.
#[cfg(feature = "write")]
#[test]
fn where_the_reference_judges_a_displaced_dirent_this_checker_agrees() {
    use fs_bcachefs::bkey::{key_type, Bkey, Bpos};
    use fs_bcachefs::btree;
    use fs_bcachefs::inode::{Dirent, InodeV3Raw, FIELD_NAMES};
    use fs_bcachefs::write::Writer;

    /// A copy under /share, removed when dropped, pass or panic.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A copy of `default` under /share with `hello.txt`'s dirent moved
    /// `by` slots on and a key of type `left` in its own slot.
    fn displaced(name: &str, by: u64, left: u8) -> Scratch {
        assert!(
            Path::new("/usr/local/bin/bcachefs-ref").exists(),
            "the reference checker is not here: this runs inside the harness guest (`chore test:vm`)"
        );
        std::fs::create_dir_all("/share/check-oracle").unwrap();
        let img = Scratch(PathBuf::from(format!("/share/check-oracle/{name}.img")));
        std::fs::copy(fixture("default.img"), &img.0).unwrap();
        let d = {
            let dev = FileDevice::open(&img.0).unwrap();
            let sb = Superblock::read(&dev).unwrap();
            btree::walk(&dev, &sb, btree_id::DIRENTS)
                .unwrap()
                .into_iter()
                .find(|k| Dirent::from_key(k).is_ok_and(|d| d.name == b"hello.txt"))
                .expect("the dirent's key")
        };
        let at_slot = Bkey {
            key_type: left,
            value: Vec::new(),
            ..d.clone()
        };
        // The file's inode names the slot its dirent sits in; it follows
        // the move, so the only damage is where the dirent now hashes.
        let target = Dirent::from_key(&d).unwrap().inum;
        let mut ino = {
            let dev = FileDevice::open(&img.0).unwrap();
            let sb = Superblock::read(&dev).unwrap();
            btree::walk(&dev, &sb, btree_id::INODES)
                .unwrap()
                .into_iter()
                .find(|k| {
                    k.pos.inode == 0 && k.pos.offset == target && k.key_type == key_type::INODE_V3
                })
                .expect("hello.txt's inode")
        };
        let mut raw = InodeV3Raw::parse(&ino.value).unwrap();
        let at = 2 * 4 + FIELD_NAMES.iter().position(|&n| n == "dir_offset").unwrap();
        assert_eq!(
            raw.varints.get(at).copied(),
            Some(d.pos.offset),
            "hello.txt's inode does not name its dirent's slot"
        );
        raw.varints[at] = d.pos.offset + by;
        ino.value = raw.encode();
        let moved = Bkey {
            pos: Bpos {
                offset: d.pos.offset + by,
                ..d.pos
            },
            ..d
        };
        let mut w = Writer::open(FileDevice::open_rw(&img.0).unwrap()).unwrap();
        w.insert(btree_id::DIRENTS, vec![at_slot, moved]).unwrap();
        w.insert(btree_id::INODES, vec![ino]).unwrap();
        drop(w);
        img
    }

    fn wrong_offset(img: &Path) -> bool {
        check(&FileDevice::open(img).unwrap())
            .unwrap()
            .problems
            .iter()
            .any(|p| p.kind == "hash_table_key_wrong_offset")
    }

    let away = displaced("dirent-wrong-offset", 5, key_type::DELETED);
    let (theirs, text) = reference_finds_a_problem(&away.0);
    assert!(
        theirs && text.contains("hash table key at wrong offset"),
        "{}: the reference did not report the dirent at a wrong offset, so this \
         case proves nothing:\n{text}",
        away.0.display()
    );
    assert!(
        wrong_offset(&away.0),
        "{}: the reference found a dirent at a wrong offset this checker did not \
         name as hash_table_key_wrong_offset:\n{text}",
        away.0.display()
    );

    let past = displaced("dirent-past-whiteout", 1, key_type::HASH_WHITEOUT);
    let (theirs, text) = reference_finds_a_problem(&past.0);
    assert!(
        !theirs,
        "{}: the reference found a problem in a dirent reachable over a \
         whiteout:\n{text}",
        past.0.display()
    );
    assert!(
        !wrong_offset(&past.0),
        "{}: the reference calls a dirent reachable over a whiteout clean, and \
         this checker reported it at a wrong offset",
        past.0.display()
    );
}

/// The inode cases damage their images through the writer: without it
/// they would vanish, and a case that does not run fails.
#[cfg(not(feature = "write"))]
#[test]
fn the_inode_cases_need_the_write_feature() {
    panic!("the inode cases need --features write, as scripts/guest-suite.sh runs them");
}
