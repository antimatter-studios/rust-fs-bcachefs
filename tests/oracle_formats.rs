//! The packed key formats the reference writes (#82). Every format the
//! reference lister printed for every fixture (`*.formats.txt`) is read
//! back node for node by this crate, and every field width in them is a
//! whole number of bytes: the reference rounds a field's width up to bytes,
//! so a format with a width such as 13 bits is not one it writes.

mod common;

use common::fixture;
use fs_bcachefs::bkey::{self, BkeyFormat};
use fs_bcachefs::btree::{self, NodePtr};
use fs_bcachefs::superblock::Superblock;
use fs_core::{BlockRead, FileDevice};

/// Every `<image>.<btree>.formats.txt` under the fixtures, as
/// `(path relative to the fixtures, image, btree)`.
fn dumps() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for sub in ["", "write-study/"] {
        for e in std::fs::read_dir(fixture(sub)).unwrap() {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".formats.txt") {
                let (image, btree) = stem.rsplit_once('.').unwrap();
                out.push((
                    format!("{sub}{name}"),
                    format!("{sub}{image}"),
                    btree.into(),
                ));
            }
        }
    }
    out.sort();
    out
}

fn show(f: &BkeyFormat) -> String {
    (0..6)
        .map(|i| format!("{}:{}", f.bits[i], f.field_offset[i]))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `(level, fields)` of every node the lister printed, sorted.
fn lister_formats(dump: &str) -> Vec<(u8, String)> {
    let mut out = Vec::new();
    let mut level = None;
    for l in common::read_text(dump).lines() {
        if let Some(rest) = l.strip_prefix("l ") {
            level = rest.split_whitespace().next().and_then(|n| n.parse().ok());
        } else if let Some(rest) = l.trim().strip_prefix("format: ") {
            // u64s N fields B:O, B:O, ... unpack fn len: 0
            let fields = rest
                .split_once("fields ")
                .unwrap()
                .1
                .split("unpack")
                .next()
                .unwrap()
                .trim()
                .to_string();
            out.push((level.take().expect("a format line before its node"), fields));
        }
    }
    out.sort();
    out
}

/// `(level, fields)` of every node this crate reads, from the root down.
fn our_formats(image: &str, btree: &str) -> Vec<(u8, String)> {
    let dev = FileDevice::open(fixture(&format!("{image}.img"))).unwrap();
    let sb = Superblock::read(&dev).unwrap();
    let id = match btree {
        "extents" => btree::btree_id::EXTENTS,
        "inodes" => btree::btree_id::INODES,
        "dirents" => btree::btree_id::DIRENTS,
        _ => panic!("no btree named {btree} here"),
    };
    // A btree with no root is empty, and the lister prints no node for it.
    let Some(root) = sb
        .btree_roots()
        .unwrap()
        .into_iter()
        .find(|r| r.btree_id == id)
    else {
        return Vec::new();
    };
    let unpacked = BkeyFormat {
        key_u64s: 5,
        nr_fields: 6,
        bits: [0; 6],
        field_offset: [0; 6],
    };
    let mut out = Vec::new();
    let mut todo = vec![(
        root.level,
        NodePtr::from_key(&bkey::decode(&root.key, &unpacked).unwrap()).unwrap(),
    )];
    while let Some((level, ptr)) = todo.pop() {
        let node = btree::read_node(&dev as &dyn BlockRead, &sb, &ptr)
            .unwrap_or_else(|e| panic!("{image} {btree}: {e}"));
        out.push((level, show(&node.format)));
        if level > 0 {
            for k in node
                .keys
                .iter()
                .filter(|k| k.key_type == bkey::key_type::BTREE_PTR_V2)
            {
                todo.push((level - 1, NodePtr::from_key(k).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_node_format_reads_as_the_lister_printed_it() {
    let all = dumps();
    assert!(all.len() >= 5, "only {} format dumps", all.len());
    for (dump, image, btree) in &all {
        assert_eq!(our_formats(image, btree), lister_formats(dump), "{dump}");
    }
}

#[test]
fn every_field_width_the_reference_wrote_is_whole_bytes() {
    let mut fields = 0;
    let mut narrowed = 0;
    for (dump, _, _) in dumps() {
        for (_, f) in lister_formats(&dump) {
            for field in f.split(", ") {
                let bits: u32 = field.split(':').next().unwrap().parse().unwrap();
                assert_eq!(bits % 8, 0, "{dump}: a {bits}-bit field in {f}");
                fields += 1;
                narrowed += usize::from(bits != 0 && bits < 64);
            }
        }
    }
    // Enough formats, narrowed enough, for the rounding to be a finding.
    assert!(
        fields >= 500 && narrowed >= 100,
        "{fields} fields, {narrowed} narrowed"
    );
    eprintln!("{fields} fields, {narrowed} narrowed below 64 bits, all whole bytes");
}
