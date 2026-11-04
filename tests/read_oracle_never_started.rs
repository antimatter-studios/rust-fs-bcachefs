//! The read tools against the reference tools on images the reference formatter
//! made and nothing has started, INSIDE the harness guest only (`chore test:vm`,
//! with the `cli` feature); anywhere else it fails naming that. Nothing skips
//! (#534).
//!
//! What the image's superblock says is held to the reference printer. For a
//! directory, what the reference lister shows is held to be something it makes
//! at that start and does not keep: so a listing here would be an invention, and
//! these tools refuse by name until a start has written one, after which they
//! agree with the lister. docs/clean-room.md, "Images formatted and never
//! started". A crash, a hang or an internal fault of the reference stays
//! private: it fails with no output.

mod common;

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::process::{Command, Output};

use common::{ScratchDir, ScratchImage};

const DEVICE_BYTES: u64 = 256 << 20;
const EXIT_REFUSED: i32 = 4;
const REFUSAL: &str = "has not been started";

/// The reference tool's printed text, which must exit 0. Nothing of a fault is
/// kept.
fn reference(args: &[&str], image: &Path) -> String {
    let out = common::reference_runs::output(args, image);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let faulted = [
        "BUG",
        "ssertion",
        "acktrace",
        "Call Trace",
        "panicked at",
        "Kernel panic",
    ]
    .iter()
    .any(|fault| text.contains(fault));
    assert!(
        out.status.success() && !faulted,
        "reference fault or refusal: 1; no case details retained"
    );
    text
}

fn format(args: &[&str]) -> ScratchImage {
    let image = ScratchDir::new("read-never-started").into_image("image.img");
    std::fs::File::create(&image)
        .unwrap()
        .set_len(DEVICE_BYTES)
        .unwrap();
    let mut command = vec!["format", "-q"];
    command.extend_from_slice(args);
    let out = common::reference_runs::output(&command, image.as_path());
    assert!(out.status.success(), "format {args:?} failed");
    image
}

fn fs_bcachefs(image: &ScratchImage, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rust-fs-bcachefs"))
        .arg("fs")
        .arg(image.as_path())
        .args(args)
        .output()
        .unwrap()
}

fn text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).trim().to_owned()
}

fn property(image: &ScratchImage, key: &str) -> String {
    let out = fs_bcachefs(image, &["get", key, "--text"]);
    assert!(out.status.success(), "get {key}: {}", text(&out.stderr));
    text(&out.stdout)
}

/// What the reference printer says, keyed as it prints them; the first device's
/// lines under `dev.`.
fn printed(image: &ScratchImage) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let mut device = "";
    for line in reference(&["show-super"], image.as_path()).lines() {
        if line.starts_with("Device 0:") {
            device = "dev.";
        } else if let Some((key, value)) = line.split_once(':') {
            fields
                .entry(format!("{device}{}", key.trim()))
                .or_insert_with(|| value.trim().to_owned());
        }
    }
    fields
}

/// A size the reference printer gives with a unit.
fn size(printed: &str) -> u64 {
    let (number, unit) = match printed.strip_suffix(['k', 'M', 'G']) {
        Some(number) => (number, &printed[number.len()..]),
        None => (printed, ""),
    };
    let scale = match unit {
        "k" => 1 << 10,
        "M" => 1 << 20,
        "G" => 1 << 30,
        _ => 1,
    };
    (number.parse::<f64>().unwrap() * f64::from(scale)).round() as u64
}

/// The version in `name (1.38)`.
fn version(printed: &str) -> &str {
    let open = printed.rfind('(').unwrap();
    printed[open + 1..].trim_end_matches(')')
}

fn assert_properties_are_the_printers(image: &ScratchImage, what: &str) {
    let f = printed(image);
    let field = |key: &str| f.get(key).unwrap_or_else(|| panic!("{what}: no {key}"));
    let buckets: u64 = field("dev.Buckets").parse().unwrap();
    let first: u64 = field("dev.First bucket").parse().unwrap();
    let total = (buckets - first) * size(field("dev.Bucket size"));
    let label = field("Label");
    for (key, want) in [
        ("bcachefs.version", version(field("Version")).to_owned()),
        ("bcachefs.uuid", field("External UUID").clone()),
        ("bcachefs.devices", field("Devices").clone()),
        (
            "bcachefs.btree_node_size",
            size(field("btree_node_size")).to_string(),
        ),
        ("block_size", size(field("block_size")).to_string()),
        ("total_bytes", total.to_string()),
        ("bcachefs.clean", (field("Clean") == "1").to_string()),
        ("dirty", (field("Clean") != "1").to_string()),
        (
            "label",
            if label == "(none)" {
                String::new()
            } else {
                label.clone()
            },
        ),
    ] {
        assert_eq!(property(image, key), want, "{what}: {key}");
    }
}

/// The formatter leaves all of these to the first start, and its superblock
/// shows it: sequence 0, and no journal or clean section.
fn assert_never_started_by_the_printer(image: &ScratchImage, what: &str) {
    let f = printed(image);
    assert_eq!(f["Sequence number"], "0", "{what}");
    for section in f["Sections"].split(',') {
        assert!(
            !["journal", "journal_v2", "clean"].contains(&section),
            "{what}: {section}"
        );
    }
    assert_eq!(f["Clean"], "0", "{what}");
}

fn assert_refused(image: &ScratchImage, what: &str) {
    for args in [
        &["ls"][..],
        &["ls", "-R"],
        &["tree"],
        &["stat", "/"],
        &["read", "/lost+found"],
    ] {
        let out = fs_bcachefs(image, args);
        let error = text(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(EXIT_REFUSED),
            "{what}: {args:?}: {error}"
        );
        assert!(error.contains(REFUSAL), "{what}: {args:?}: {error}");
        assert!(!error.contains("not implemented"), "{what}: {error}");
        assert!(!error.contains("journal"), "{what}: {error}");
        assert!(out.stdout.is_empty(), "{what}: {args:?}");
    }
}

#[test]
fn every_version_the_formatter_leaves_to_its_first_start_reads_as_the_printer_says() {
    for minor in (0..=38).chain([40]) {
        let what = format!("1.{minor}");
        let image = format(&[&format!("--version=1.{minor}")]);
        assert_never_started_by_the_printer(&image, &what);
        assert_properties_are_the_printers(&image, &what);
        assert_eq!(property(&image, "bcachefs.started"), "false", "{what}");
        assert_refused(&image, &what);
    }
}

#[test]
fn what_the_formatter_was_told_reads_as_the_printer_says() {
    for args in [
        &["-L", "name"][..],
        &["--label=device"],
        &["--block_size=4096"],
        &["--btree_node_size=64k"],
        &["--bucket_size=64k", "--btree_node_size=32k"],
    ] {
        let what = format!("{args:?}");
        let mut all = vec!["--version=1.38"];
        all.extend_from_slice(args);
        let image = format(&all);
        assert_never_started_by_the_printer(&image, &what);
        assert_properties_are_the_printers(&image, &what);
        assert_refused(&image, &what);
    }
}

/// The lister exits 0 and shows a root and a `lost+found` on the image. It made
/// them: it starts the filesystem in memory, keeps nothing, and the next run
/// makes others. There is nothing on the image for a listing to agree with.
#[test]
fn what_the_reference_lists_it_makes_at_that_start_and_does_not_keep() {
    let image = format(&["--version=1.38"]);
    let bytes = std::fs::read(&image).unwrap();
    let mut seeds = BTreeSet::new();
    let mut entries = BTreeSet::new();
    for _ in 0..3 {
        let dirents = reference(&["list", "-b", "dirents"], image.as_path());
        assert!(dirents.contains("initializing new filesystem"));
        let line = dirents
            .lines()
            .find(|line| line.contains("type dirent"))
            .expect("the lister shows an entry");
        assert!(line.contains(" : lost+found -> "), "{line}");
        entries.insert(line.to_owned());
        let inodes = reference(&["list", "-b", "inodes"], image.as_path());
        let seed = inodes
            .lines()
            .find(|line| line.trim_start().starts_with("hash_seed="))
            .expect("the lister shows the root's seed");
        seeds.insert(seed.to_owned());
        assert!(
            std::fs::read(&image).unwrap() == bytes,
            "the lister changed the image"
        );
    }
    assert_eq!(seeds.len(), 3, "each start makes its own root");
    assert_eq!(entries.len(), 3, "each start hashes its own entry");
    assert_refused(&image, "1.38");
    assert!(
        std::fs::read(&image).unwrap() == bytes,
        "the read tools changed the image"
    );
}

/// The lister's `NAME -> INODE type dir`, for each entry in the root.
fn lister_root(image: &ScratchImage) -> Vec<(String, u64, String)> {
    let dirents = reference(&["list", "-b", "dirents"], image.as_path());
    dirents
        .lines()
        .filter(|line| line.contains("type dirent 4096:"))
        .map(|line| {
            let (_, entry) = line.rsplit_once(" : ").unwrap();
            let (name, target) = entry.split_once(" -> ").unwrap();
            let (inode, kind) = target.split_once(" type ").unwrap();
            (name.to_owned(), inode.parse().unwrap(), kind.to_owned())
        })
        .collect()
}

/// Once a start has written it, the same image lists, and as the lister does.
#[test]
fn after_the_reference_starts_it_the_listing_is_the_listers() {
    let image = format(&["--version=1.38"]);
    assert_refused(&image, "before the start");
    let started = reference(&["fsck", "-y"], image.as_path());
    assert!(started.contains("initializing new filesystem"));

    let f = printed(&image);
    assert_eq!(f["Clean"], "1");
    assert!(f["Sections"].contains("journal_v2"), "{}", f["Sections"]);
    assert_properties_are_the_printers(&image, "after the start");
    assert_eq!(property(&image, "bcachefs.started"), "true");

    let root = lister_root(&image);
    assert_eq!(root.len(), 1, "{root:?}");
    let (name, inode, kind) = &root[0];
    assert_eq!(name, "lost+found");
    let out = fs_bcachefs(&image, &["ls"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let listed = text(&out.stdout);
    for want in [
        format!("\"name\":\"{name}\""),
        format!("\"type\":\"{kind}\""),
        format!("\"ino\":{inode}"),
    ] {
        assert!(
            listed.replace(": ", ":").contains(&want),
            "{want} in {listed}"
        );
    }
}

/// The formatter starts the current version itself: it is no such image.
#[test]
fn the_current_version_is_started_by_the_formatter_and_lists() {
    let image = format(&["--version=1.39"]);
    assert_eq!(property(&image, "bcachefs.started"), "true");
    assert_properties_are_the_printers(&image, "1.39");
    let out = fs_bcachefs(&image, &["ls"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("lost+found"));
}
