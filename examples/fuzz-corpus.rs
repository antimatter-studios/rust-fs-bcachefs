//! Cut the fuzz seeds out of the fixtures the reference tools made:
//! `scripts/make-fuzz-corpus.sh` runs this.
//!
//!     cargo run --release --example fuzz-corpus -- .vm-share/fixtures fuzz/corpus
//!
//! Each seed is a real structure at a known place in a real image, found
//! with this crate's own reader: superblocks, btree nodes (the root and a
//! leaf of each btree this reader walks), journal entries, one unpacked
//! key of each kind, inode and extent values, and the stored sectors of
//! an lz4-compressed extent. Only the names written here are written;
//! anything else under the corpus -- a committed reproducer above all --
//! is left where it is.

use std::path::{Path, PathBuf};

use fs_bcachefs::bkey::{self, key_type, Bkey, BkeyFormat};
use fs_bcachefs::btree::{self, NodePtr};
use fs_bcachefs::extent::{compression, DataExtent};
use fs_bcachefs::journal;
use fs_bcachefs::superblock::{Superblock, SB_HEADER_BYTES, SB_OFFSET};
use fs_core::{BlockRead, FileDevice};

const UNPACKED: BkeyFormat = BkeyFormat {
    key_u64s: 5,
    nr_fields: 6,
    bits: [0; 6],
    field_offset: [0; 6],
};

struct Out {
    root: PathBuf,
    written: usize,
}

impl Out {
    fn put(&mut self, dir: &str, name: &str, bytes: &[u8]) {
        let d = self.root.join(dir);
        std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display()));
        let p = d.join(name);
        std::fs::write(&p, bytes).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        println!("{} ({} bytes)", p.display(), bytes.len());
        self.written += 1;
    }
}

fn read(dev: &FileDevice, at: u64, len: usize) -> Vec<u8> {
    let mut b = vec![0u8; len];
    dev.read_at(at, &mut b).expect("read");
    b
}

/// The written part of a node, as the parent's pointer describes it.
fn node_bytes(dev: &FileDevice, ptr: &NodePtr) -> Vec<u8> {
    read(
        dev,
        ptr.ptrs[0].offset * 512,
        ptr.sectors_written as usize * 512,
    )
}

/// An unpacked key's bytes: the layout `bkey.rs` documents.
fn encode_key(k: &Bkey) -> Vec<u8> {
    let u64s = 5 + k.value.len() / 8;
    let mut b = vec![u64s as u8, 1, k.key_type, 0];
    b.extend_from_slice(&k.version_hi.to_le_bytes());
    b.extend_from_slice(&k.version_lo.to_le_bytes());
    b.extend_from_slice(&k.size.to_le_bytes());
    b.extend_from_slice(&k.pos.snapshot.to_le_bytes());
    b.extend_from_slice(&k.pos.offset.to_le_bytes());
    b.extend_from_slice(&k.pos.inode.to_le_bytes());
    b.extend_from_slice(&k.value);
    b
}

fn open(fixtures: &Path, set: &str) -> (FileDevice, Superblock) {
    let img = fixtures.join(format!("{set}.img"));
    let dev = FileDevice::open(&img).unwrap_or_else(|e| panic!("{}: {e}", img.display()));
    let sb = Superblock::read(&dev).unwrap_or_else(|e| panic!("{}: {e}", img.display()));
    (dev, sb)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: fuzz-corpus <fixtures-dir> <corpus-dir>");
        std::process::exit(2);
    }
    let fixtures = Path::new(&args[1]);
    let mut out = Out {
        root: PathBuf::from(&args[2]),
        written: 0,
    };

    // Superblocks: one per checksum type the fixtures cover.
    for set in ["default", "crc64", "xxhash"] {
        let (dev, sb) = open(fixtures, set);
        let bytes = read(&dev, SB_OFFSET, SB_HEADER_BYTES + sb.u64s as usize * 8);
        out.put("superblock", set, &bytes);
    }

    // Nodes: the root and the first leaf of every btree this reader walks,
    // from the formatter's image and from the aged one.
    let btrees = [
        (0u8, "extents"),
        (1, "inodes"),
        (2, "dirents"),
        (3, "xattrs"),
    ];
    for set in ["default", "aged"] {
        let (dev, sb) = open(fixtures, set);
        let roots = sb.btree_roots().expect("roots");
        for (id, name) in btrees {
            let Some(root) = roots.iter().find(|r| r.btree_id == id) else {
                continue;
            };
            let key = bkey::decode(&root.key, &UNPACKED).expect("root key");
            let mut ptr = NodePtr::from_key(&key).expect("root pointer");
            let mut level = root.level;
            if level > 0 {
                out.put(
                    "btree_node",
                    &format!("{set}-{name}-interior"),
                    &node_bytes(&dev, &ptr),
                );
            }
            while level > 0 {
                let node = btree::read_node(&dev, &sb, &ptr).expect("interior node");
                let child = node
                    .keys
                    .iter()
                    .find(|k| k.key_type == key_type::BTREE_PTR_V2)
                    .expect("a child pointer");
                ptr = NodePtr::from_key(child).expect("child pointer");
                level -= 1;
            }
            out.put(
                "btree_node",
                &format!("{set}-{name}"),
                &node_bytes(&dev, &ptr),
            );
        }

        if set == "aged" {
            // One unpacked key of each kind, and the values behind two of them.
            let kinds = [
                (0u8, key_type::EXTENT, "extent"),
                (0, key_type::INLINE_DATA, "inline"),
                (1, key_type::INODE_V3, "inode"),
                (2, key_type::DIRENT, "dirent"),
                (3, key_type::XATTR, "xattr"),
            ];
            for (id, ty, name) in kinds {
                let keys = btree::walk(&dev, &sb, id).expect("walk");
                let Some(k) = keys.iter().find(|k| k.key_type == ty) else {
                    continue;
                };
                out.put("key_values", &format!("{set}-{name}"), &encode_key(k));
                match ty {
                    key_type::INODE_V3 => out.put("inode_v3", &format!("{set}-inode"), &k.value),
                    key_type::EXTENT => {
                        out.put("extent_entries", &format!("{set}-extent"), &k.value)
                    }
                    _ => {}
                }
            }
        }
    }

    // Journal entries: the two of the unclean image with the most sub-entries.
    {
        let (dev, sb) = open(fixtures, "aged-unclean");
        let mut entries = journal::read_entries(&dev, &sb).expect("journal");
        entries.sort_by_key(|j| std::cmp::Reverse(j.entries.len()));
        for j in entries.iter().take(2) {
            out.put(
                "jset",
                &format!("aged-unclean-{}", j.seq),
                &read(&dev, j.sector * 512, j.bytes),
            );
        }
    }

    // LZ4: the stored sectors of the first compressed extent, and the
    // formatter's first extent value.
    {
        let (dev, sb) = open(fixtures, "lz4");
        let keys = btree::walk(&dev, &sb, 0).expect("extents");
        if let Some(k) = keys.iter().find(|k| k.key_type == key_type::EXTENT) {
            out.put("extent_entries", "lz4-extent", &k.value);
        }
        let first = keys
            .iter()
            .filter(|k| k.key_type == key_type::EXTENT)
            .filter_map(|k| DataExtent::from_key(k).ok())
            .find(|e| {
                e.crc
                    .is_some_and(|c| c.compression_type == compression::LZ4)
            });
        if let Some(e) = first {
            let crc = e.crc.expect("crc");
            let bytes = read(&dev, e.ptr.offset * 512, crc.compressed_size as usize * 512);
            out.put(
                "lz4_block",
                &format!("lz4-{}-{}", e.file_start, e.len),
                &bytes,
            );
        }
    }

    println!("{} seeds written under {}", out.written, out.root.display());
}
