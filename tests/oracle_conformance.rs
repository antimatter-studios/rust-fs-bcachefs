//! rust-fs-core's conformance helpers over this driver's three layers
//! (`fs_bcachefs::volume::Volume`), on the reference formatter's fixtures and
//! on copies damaged here. The expected files are the ones the formatter
//! was fed: each one's bytes are checked against the manifest's SHA-256
//! before they are handed to the helper as expected contents.

mod common;

use common::{fixture, manifest};
use fs_bcachefs::bkey::key_type;
use fs_bcachefs::btree::btree_id;
use fs_bcachefs::salvage::adapter::BcachefsSalvage;
use fs_bcachefs::volume::Volume;
use fs_bcachefs::Filesystem;
use fs_core::layers::conformance::salvage::{self as deep, Damaged, Driver, Evidence};
use fs_core::layers::conformance::{self, Expected, Limits};
use fs_core::layers::file::FileAPI;
use fs_core::layers::salvage::{DamageKind, Extent};
use fs_core::layers::volume::OpenVolume;
use fs_core::{BlockRead, FileDevice};
use sha2::{Digest, Sha256};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The aged image keeps 27,438 inode keys across its live nodes and the
/// older versions of them: every one is walked.
fn limits() -> Limits {
    Limits {
        max_nodes: 20_000,
        max_depth: 64,
        max_walks: 32_768,
        ..Limits::default()
    }
}

/// What the formatter was fed, as the helper's expectations: every
/// directory, and every file whose bytes this driver reads match the
/// manifest's hash, plus a path that is not there.
fn expected(set: &str, volume: &Volume<FileDevice>) -> Vec<Expected> {
    let mut out = vec![Expected::absent("/no-such-entry-here")];
    for e in manifest(set) {
        match e.kind.as_str() {
            "dir" => out.push(Expected::directory(&e.path)),
            "file" => {
                let node = volume.lookup(&e.path).unwrap();
                let mut bytes = Vec::new();
                volume.copy_to(&node, &mut bytes).unwrap();
                assert_eq!(
                    Some(hex(&Sha256::digest(&bytes))),
                    e.sha256,
                    "{set}: {}",
                    e.path
                );
                out.push(Expected::file(&e.path, &bytes));
            }
            _ => {}
        }
    }
    out
}

/// The clean sets the helper walks whole: `large`, 30,001 entries, is past
/// any walk the helper's limits are meant for, and is read by the oracle
/// tier's own tests.
const SETS: &[&str] = &[
    "default", "lz4", "zstd", "gzip", "nocsum", "xxhash", "crc64", "block4k", "aged",
];

#[test]
fn every_clean_set_passes_the_healthy_checks() {
    for set in SETS {
        let t = std::time::Instant::now();
        let volume = Volume::open(FileDevice::open(fixture(&format!("{set}.img"))).unwrap())
            .unwrap_or_else(|e| panic!("{set}: {e}"));
        let expected = expected(set, &volume);
        let mut report = conformance::check_healthy(&volume, &expected, &limits());
        // Every file has a live candidate whose walk's plan rebuilds it
        // byte for byte, compressed and inline runs included.
        report.merge(conformance::check_files_against_candidates(
            &volume,
            &limits(),
        ));
        assert!(report.passed(), "{set}:\n{report}");
        eprintln!("{set}: {:?}", t.elapsed());
    }
}

/// An image in memory, read-only.
struct Mem<'a>(&'a [u8]);

impl BlockRead for Mem<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let bytes = start
            .checked_add(buf.len())
            .and_then(|end| self.0.get(start..end))
            .ok_or(fs_core::Error::OutOfBounds {
                offset,
                len: buf.len() as u64,
                size: self.0.len() as u64,
            })?;
        buf.copy_from_slice(bytes);
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.0.len() as u64
    }
}

/// The device byte of the stored checksum of a bset in a live inodes leaf
/// that holds inode keys: flipping it fails that bset's checksum and leaves
/// every key as it was.
fn inode_bset_checksum(dev: &dyn BlockRead) -> u64 {
    let s = fs_bcachefs::salvage::Salvage::open(dev).unwrap();
    for (&offset, n) in &s.live_nodes().nodes {
        if n.btree_id != btree_id::INODES || n.level != 0 {
            continue;
        }
        let node = fs_bcachefs::primitive::inspect_node(dev, s.node_context(), offset)
            .unwrap()
            .unwrap();
        if let Some(k) = node
            .keys
            .iter()
            .find(|k| matches!(k.key.key_type, key_type::INODE_V3 | key_type::INODE_V2))
        {
            return offset + node.bsets[k.bset].record_offset as u64;
        }
    }
    panic!("no live inodes leaf holds an inode key")
}

/// The live leaf node of btree `id` at the lowest offset, and the node size.
fn live_leaf(img: &[u8], id: u8) -> (u64, u64) {
    let mem = Mem(img);
    let dev: &dyn BlockRead = &mem;
    let s = fs_bcachefs::salvage::Salvage::open(dev).unwrap();
    let offset = s
        .live_nodes()
        .nodes
        .iter()
        .find(|(_, n)| n.btree_id == id && n.level == 0)
        .map(|(&o, _)| o)
        .expect("a live leaf");
    (offset, s.node_context().node_bytes as u64)
}

struct Bcachefs {
    healthy: Vec<u8>,
}

impl Driver for Bcachefs {
    type Adapter = BcachefsSalvage;
    type StrictError = fs_bcachefs::Error;

    fn salvage(&self, source: &dyn BlockRead) -> BcachefsSalvage {
        BcachefsSalvage::new(source)
    }

    /// Open, then list every directory and read every file from the root.
    fn strict(&self, source: &dyn BlockRead) -> Result<(), fs_bcachefs::Error> {
        let fs = Filesystem::open(source)?;
        let mut stack = vec![fs.root()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            if !seen.insert((dir.ino, dir.snapshot)) {
                continue;
            }
            for d in fs.readdir_at(dir)? {
                let node = fs.child(dir, &d)?;
                if fs.inode_at(node)?.is_dir() {
                    stack.push(node);
                } else {
                    fs.read_at(node)?;
                }
            }
        }
        Ok(())
    }

    fn healthy(&self) -> Vec<u8> {
        self.healthy.clone()
    }

    fn damaged(&self) -> Vec<Damaged> {
        let flip = |at: u64| {
            let mut img = self.healthy.clone();
            img[at as usize] ^= 0x01;
            img
        };
        let checksum = inode_bset_checksum(&Mem(&self.healthy));
        let (dirents, node_bytes) = live_leaf(&self.healthy, btree_id::DIRENTS);
        let mut zeroed = self.healthy.clone();
        zeroed[dirents as usize..(dirents + node_bytes) as usize].fill(0);
        vec![
            Damaged::new(
                "inodes leaf checksum",
                flip(checksum),
                Evidence::Damage(DamageKind::Checksum),
            ),
            Damaged::new(
                "inodes leaf keys under a bad checksum",
                flip(checksum),
                Evidence::FailedCandidate,
            ),
            Damaged::new(
                "dirents leaf zeroed",
                zeroed,
                Evidence::Damage(DamageKind::Broken),
            ),
            Damaged::new(
                "dirents leaf unreadable",
                self.healthy.clone(),
                Evidence::Damage(DamageKind::Unreadable),
            )
            .unreadable(Extent::new(dirents, 512)),
        ]
    }
}

/// Over the image the kernel module wrote and deleted files in: its older
/// node versions hold deleted inodes, so the deleted-candidate and
/// deleted-extent capabilities are exercised, as the set requires.
#[test]
fn the_salvage_adapter_passes_the_deep_salvage_set() {
    // The shared conformance Driver requires owned buffers for its damaged
    // inputs. Bound this one intentional read to the measured 64 MiB fixture.
    let path = fixture("kernel-reuse.img");
    assert!(
        std::fs::metadata(&path).unwrap().len() <= 64 * 1024 * 1024,
        "the deep conformance fixture exceeds its 64 MiB input budget"
    );
    let healthy = std::fs::read(fixture("kernel-reuse.img")).unwrap();
    deep::run(&Bcachefs { healthy }).assert_conforms();
}

#[test]
fn a_damaged_copy_passes_the_damaged_checks() {
    let img = common::ScratchDir::new("conformance").into_image("damaged.img");
    common::copy_sparse(fixture("default.img"), &img);
    let dev = FileDevice::open(&img).unwrap();
    let at = inode_bset_checksum(&dev);
    let mut checksum = [0];
    dev.read_at(at, &mut checksum).unwrap();
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&img).unwrap();
        f.seek(SeekFrom::Start(at)).unwrap();
        f.write_all(&[checksum[0] ^ 0x01]).unwrap();
    }
    assert!(
        Volume::open(FileDevice::open(&img).unwrap()).is_err(),
        "strict open accepts it"
    );
    let volume = Volume::open_for_salvage(FileDevice::open(&img).unwrap()).unwrap();
    let report = conformance::check_damaged(&volume, &limits());
    assert!(report.passed(), "{report}");
}
