//! The reference kernel module as an oracle (#110). The FUSE mount cannot
//! make reflinks, snapshots, casefolded directories or per-inode options,
//! and stalls on a removal followed by a sync (#94). The harness guest
//! boots a kernel that carries the reference module (scripts/vm-setup.sh),
//! and the fixture build mounts an image through it, writes a tree, unmounts
//! cleanly and records what happened (scripts/guest-build-fixtures.sh).
//! Here that image is read back and compared with what the kernel's own
//! mount reported, entry by entry.

mod common;

use common::{fixture, manifest, read_text};
use fs_bcachefs::Filesystem;
use fs_core::FileDevice;
use sha2::{Digest, Sha256};

/// `key: value` lines of kernel.txt.
fn record() -> std::collections::BTreeMap<String, String> {
    read_text("kernel.txt")
        .lines()
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// The guest ran a kernel with the reference module loaded, mounted an
/// image through it, unmounted it cleanly, and the reference checker passed
/// the result.
#[test]
fn the_reference_kernel_module_mounted_and_unmounted_an_image() {
    let r = record();
    let text = read_text("kernel.txt");
    assert!(
        r.get("kernel").is_some_and(|k| !k.is_empty()),
        "kernel.txt names no kernel:\n{text}"
    );
    assert_eq!(
        r.get("module").map(String::as_str),
        Some("loaded"),
        "{text}"
    );
    assert_eq!(r.get("mount").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("reflink").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("snapshot").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("unmount").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(r.get("fsck").map(String::as_str), Some("clean"), "{text}");
}

/// Every file, directory and symlink the kernel wrote reads back here at the
/// inode number, mode, size and contents its mount reported: the reflinked
/// pair (#7), big/random.bin and its clone, both read through a `reflink_p`
/// into the reflink btree; and the subvolume as it is now, and its snapshot
/// as it was (#12).
#[test]
fn what_the_kernel_wrote_reads_back_byte_for_byte() {
    let fs = Filesystem::open(FileDevice::open(fixture("kernel.img")).unwrap())
        .unwrap_or_else(|e| panic!("kernel.img: {e}"));
    let mut files = 0;
    for e in manifest("kernel") {
        let node = fs
            .resolve(&e.path)
            .unwrap_or_else(|err| panic!("kernel {}: {err}", e.path));
        assert_eq!(Some(node.ino), e.ino, "kernel {}: inode number", e.path);
        let inode = fs.inode_at(node).unwrap();
        assert_eq!(inode.mode & 0o7777, e.mode, "kernel {}: mode", e.path);
        match e.kind.as_str() {
            "file" => {
                // A hash that is not 64 hex digits is a garbled record,
                // not a misread file (#134).
                assert!(
                    e.sha256
                        .as_ref()
                        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())),
                    "kernel {}: the record's hash {:?} is garbled",
                    e.path,
                    e.sha256
                );
                let data = fs.read_at(node).unwrap();
                assert_eq!(Some(data.len() as u64), e.size, "kernel {}: size", e.path);
                assert_eq!(
                    Some(format!("{:x}", Sha256::digest(&data))),
                    e.sha256,
                    "kernel {}: contents",
                    e.path
                );
                files += 1;
            }
            "symlink" => {
                let target = String::from_utf8(fs.read_at(node).unwrap()).unwrap();
                assert_eq!(Some(target), e.target, "kernel {}: target", e.path);
            }
            "dir" => assert!(inode.is_dir(), "kernel {}: not a directory", e.path),
            other => panic!("kernel {}: entry type {other}", e.path),
        }
    }
    assert!(files >= 10, "kernel: only {files} files compared");
    let paths: Vec<String> = manifest("kernel").into_iter().map(|e| e.path).collect();
    for p in ["/sv/new.txt", "/snap/gone.txt", "/snap/inner/file.txt"] {
        assert!(
            paths.iter().any(|q| q == p),
            "the kernel's manifest has no {p}"
        );
    }
    assert!(
        manifest("kernel")
            .iter()
            .any(|e| e.path == "/big/random-clone.bin"),
        "the kernel's manifest has no reflinked clone"
    );
}

/// Every `reflink_p` the kernel wrote (#7) is laid out as the reference
/// lister prints it: the index into the reflink btree in the low 56 bits
/// of the first word, `may_update_opts` at bit 57, and both pads 0 in the
/// second word (no image has shown a pad that is not, so where each sits
/// in it is not known).
#[test]
fn every_reflink_pointer_holds_the_index_the_lister_printed() {
    use fs_bcachefs::btree::{self, btree_id};
    let listed: Vec<(u64, u64, bool)> = read_text("kernel.extents.txt")
        .lines()
        .filter(|l| l.contains("type reflink_p "))
        .map(|l| {
            let pos = l.split_whitespace().nth(4).unwrap();
            let offset: u64 = pos.split(':').nth(1).unwrap().parse().unwrap();
            let idx: u64 = l
                .split(" idx ")
                .nth(1)
                .and_then(|r| r.split_whitespace().next())
                .unwrap()
                .parse()
                .unwrap();
            assert!(l.contains("front_pad 0 back_pad 0"), "{l}");
            (offset, idx, l.contains("may_update_opts"))
        })
        .collect();
    let dev = FileDevice::open(fixture("kernel.img")).unwrap();
    let sb = fs_bcachefs::superblock::Superblock::read(&dev).unwrap();
    let ours: Vec<(u64, u64, bool)> = btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .into_iter()
        .filter(|k| k.key_type == fs_bcachefs::bkey::key_type::REFLINK_P)
        .map(|k| {
            let w0 = u64::from_le_bytes(k.value[..8].try_into().unwrap());
            let w1 = u64::from_le_bytes(k.value[8..16].try_into().unwrap());
            assert_eq!(w1, 0, "{}: the pads' word", k.pos);
            (
                k.pos.offset,
                fs_bcachefs::extent::reflink_p_idx(&k.value).unwrap(),
                w0 >> 57 & 1 == 1,
            )
        })
        .collect();
    assert!(listed.len() >= 40, "only {} reflink_p listed", listed.len());
    assert_eq!(ours, listed);
}

/// Per-file options (#81): the reference tool set each option on a file of
/// `/opts` (and one on a directory, which its new file inherits), and
/// every option it read back from a file reads back here, by the name and
/// value it printed. The tool lists only the options set on the file itself,
/// not inherited ones.
#[test]
fn every_per_file_option_reads_back_as_the_reference_tool_reports_it() {
    let r = record();
    let set: Vec<_> = r.iter().filter(|(k, _)| k.starts_with("option.")).collect();
    assert!(
        set.len() >= 9,
        "kernel.txt records {} options set",
        set.len()
    );
    for (k, v) in &set {
        assert_eq!(v.as_str(), "ok", "{k}");
    }
    let fs = Filesystem::open(FileDevice::open(fixture("kernel.img")).unwrap()).unwrap();
    let text = read_text("kernel.options.txt");
    let mut compared = 0;
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let (Some(path), Some(said)) = (parts.next(), parts.next()) else {
            panic!("kernel.options.txt: {line:?}");
        };
        // The reference tool prints one option per line, `name<TAB>value`.
        let Some((name, value)) = said.split_once('\t') else {
            panic!("kernel.options.txt: {line:?}");
        };
        let (name, value) = (name.trim(), value.trim());
        if !fs_bcachefs::inode::FIELD_NAMES.contains(&name) {
            continue;
        }
        let ino = fs.lookup(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let got = fs.inode(ino).unwrap().option(name);
        assert_eq!(got.as_deref(), Some(value), "{path}: {name}\n{text}");
        compared += 1;
    }
    assert!(compared >= 9, "only {compared} options compared:\n{text}");
}

/// What a compression option means is what the kernel did with the data
/// (#81): every data extent of a file whose inode carries `lz4`, `gzip` or
/// `zstd`, set on the file or inherited from its directory, is stored
/// compressed with that codec, by the crc entry the kernel wrote.
#[test]
fn every_file_option_is_what_the_kernel_wrote_its_data_with() {
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::extent::{compression, DataExtent};
    let fs = Filesystem::open(FileDevice::open(fixture("kernel.img")).unwrap()).unwrap();
    let dev = FileDevice::open(fixture("kernel.img")).unwrap();
    let sb = fs_bcachefs::superblock::Superblock::read(&dev).unwrap();
    let extents = btree::walk(&dev, &sb, btree_id::EXTENTS).unwrap();
    let crcs = |ino: u64| -> Vec<(u8, u8)> {
        extents
            .iter()
            .filter(|k| k.pos.inode == ino && k.key_type == fs_bcachefs::bkey::key_type::EXTENT)
            .map(|k| {
                let c = DataExtent::from_key(k).unwrap().crc.expect("a crc entry");
                (c.compression_type, c.csum_type)
            })
            .collect()
    };
    for (path, option, codec) in [
        ("/opts/compression-lz4", "lz4", compression::LZ4),
        ("/opts/compression-gzip", "gzip", compression::GZIP),
        ("/opts/compression-zstd", "zstd", compression::ZSTD),
        ("/opts/dir/inherited", "zstd", compression::ZSTD),
    ] {
        let ino = fs.lookup(path).unwrap();
        assert_eq!(
            fs.inode(ino).unwrap().option("compression").as_deref(),
            Some(option),
            "{path}"
        );
        let got = crcs(ino);
        assert!(!got.is_empty(), "{path}: no data extents");
        assert!(
            got.iter().all(|&(c, _)| c == codec),
            "{path}: {option} stored as {got:?}"
        );
    }
}

/// Data this crate wrote on a gzip filesystem, judged by the reference
/// kernel module (#141): its tools put a compressible file on a gzip
/// filesystem the module had mounted, and a second boot of the module read
/// it back. The file is stored gzip-compressed, the module reads the bytes
/// that were written, and the reference checker passes the image.
#[test]
fn a_file_this_crate_compressed_with_gzip_reads_back_through_the_kernel() {
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::extent::{compression, DataExtent};
    use fs_bcachefs::superblock::Superblock;
    let r = record();
    let text = read_text("kernel.txt");
    assert_eq!(
        r.get("written-format").map(String::as_str),
        Some("ok"),
        "{text}"
    );
    assert_eq!(
        r.get("written-seed").map(String::as_str),
        Some("ok"),
        "{text}"
    );
    assert_eq!(r.get("written").map(String::as_str), Some("ok"), "{text}");
    assert_eq!(
        r.get("written-fsck").map(String::as_str),
        Some("clean"),
        "{text}"
    );
    let want = r
        .get("written-sha256")
        .expect("the record names no written-sha256");
    assert_eq!(
        r.get("judge.read"),
        Some(want),
        "the module read back other bytes than were written:\n{text}"
    );
    let img = fixture("kernel-written.img");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let ino = fs.lookup("/written.txt").unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(fs.read(ino).unwrap())),
        *want
    );
    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let kinds: Vec<Option<u8>> = btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .iter()
        .filter(|k| k.pos.inode == ino && k.key_type == fs_bcachefs::bkey::key_type::EXTENT)
        .map(|k| {
            DataExtent::from_key(k)
                .unwrap()
                .crc
                .map(|c| c.compression_type)
        })
        .collect();
    assert!(!kinds.is_empty(), "/written.txt has no data extents");
    assert!(
        kinds.iter().all(|&c| c == Some(compression::GZIP)),
        "/written.txt is not stored gzip-compressed: {kinds:?}"
    );
}

/// A file this crate writes in a directory with options takes them as the
/// module's own file there did (#142). The reference tool set
/// `compression=zstd` and `data_checksum=xxhash` on the directory, the module
/// wrote a file in it, and this crate's tools wrote one beside it: the
/// reference lister shows the same option fields and flags on both inodes,
/// this crate's data is stored as those options say, and the module reads
/// it back.
#[test]
fn a_file_written_in_a_directory_with_options_takes_them_as_the_kernels_does() {
    use fs_bcachefs::btree::{self, btree_id};
    use fs_bcachefs::extent::{compression, DataExtent};
    use fs_bcachefs::superblock::Superblock;
    let r = record();
    let text = read_text("kernel.txt");
    for k in [
        "written-option.compression=zstd",
        "written-option.data_checksum=xxhash",
        "written-opt",
    ] {
        assert_eq!(r.get(k).map(String::as_str), Some("ok"), "{k}:\n{text}");
    }
    assert_eq!(
        r.get("judge.optread"),
        r.get("written-sha256"),
        "the module read back other bytes than were written:\n{text}"
    );
    assert_eq!(
        r.get("written-fsck").map(String::as_str),
        Some("clean"),
        "{text}"
    );

    // The lister's inodes, by number: `name=value` lines under each key.
    let listed = read_text("kernel-written.inodes.txt");
    let mut inodes: std::collections::BTreeMap<u64, Vec<(String, String)>> = Default::default();
    let mut at = None;
    for line in listed.lines() {
        if let Some(rest) = line.split(" type inode_v3 0:").nth(1) {
            at = rest.split(':').next().and_then(|n| n.parse().ok());
            continue;
        }
        if let (Some(ino), Some((k, v))) = (at, line.trim().split_once('=')) {
            inodes
                .entry(ino)
                .or_default()
                .push((k.to_string(), v.to_string()));
        }
    }
    let img = fixture("kernel-written.img");
    let fs = Filesystem::open(FileDevice::open(&img).unwrap()).unwrap();
    let theirs = fs.lookup("/optdir/kernel.txt").unwrap();
    let ours = fs.lookup("/optdir/ours.txt").unwrap();
    let options = |ino: u64| -> Vec<(String, String)> {
        inodes
            .get(&ino)
            .unwrap_or_else(|| panic!("the lister shows no inode {ino}:\n{listed}"))
            .iter()
            .filter(|(k, _)| {
                k == "flags"
                    || [
                        "bi_data_checksum",
                        "bi_compression",
                        "bi_background_compression",
                        "bi_data_replicas",
                        "bi_promote_target",
                        "bi_foreground_target",
                        "bi_background_target",
                        "bi_erasure_code",
                        "bi_fields_set",
                    ]
                    .contains(&k.as_str())
            })
            .cloned()
            .collect()
    };
    assert_eq!(
        options(ours),
        options(theirs),
        "this crate's file (left) and the module's (right) differ in their options"
    );

    let dev = FileDevice::open(&img).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let entries: Vec<(u8, u8)> = btree::walk(&dev, &sb, btree_id::EXTENTS)
        .unwrap()
        .iter()
        .filter(|k| k.pos.inode == ours && k.key_type == fs_bcachefs::bkey::key_type::EXTENT)
        .map(|k| {
            let c = DataExtent::from_key(k).unwrap().crc.expect("a crc entry");
            (c.compression_type, c.csum_type)
        })
        .collect();
    assert!(!entries.is_empty(), "/optdir/ours.txt has no data extents");
    // xxhash data carries checksum type 7 (src/write_alloc.rs, #105).
    assert!(
        entries.iter().all(|&e| e == (compression::ZSTD, 7)),
        "/optdir/ours.txt is not stored as zstd with xxhash: {entries:?}"
    );
}
